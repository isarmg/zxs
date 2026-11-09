//! Immutable publication-time partition. Later saves may reconnect files, and
//! later retirements may split them again; neither rewrites this original fact.
use super::*;
use crate::shared_file_topology::{Member, Scan, VersionIndex, partition};
use storage_contract::history_window::HistoryWindowCancellation;
use storage_contract::shared_history::MAX_SHARED_HISTORY_FILES;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Separation {
    /// Complete live component at the admitted source root, not only touched files.
    files: Vec<String>,
    cohort_version_ids: Vec<String>,
    cohort_digest: String,
    file_components: Vec<Vec<String>>,
}

fn cohort<'a>(
    state: &'a State,
    request: &Request,
    ids: &[String],
) -> Result<Vec<&'a crate::FileVersion>> {
    if ids.is_empty() || ids.len() > MAX_SELECTED || ids.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(invalid());
    }
    ids.iter()
        .map(|id| {
            HistoryVersionId::parse(id)?;
            state
                .versions
                .get(id)
                .filter(|version| version.history_tree_id == request.history_tree_id)
                .ok_or_else(invalid)
        })
        .collect()
}

fn digest(versions: &[&crate::FileVersion]) -> Result<String> {
    // Only fixed-size identity/content commitments determine connectivity. Do
    // not copy names, full source lists, commits, body bytes or mutable notes.
    let commitments = versions
        .iter()
        .map(|version| {
            (
                &version.version_id,
                &version.file_id,
                &version.history_tree_id,
                version.logical_size,
                version.content_hash,
            )
        })
        .collect::<Vec<_>>();
    Ok(crate::object_hash(
        "retired-component-cohort",
        &commitments,
    )?)
}

impl Separation {
    pub(super) fn capture(state: &State, request: &Request) -> Result<Self> {
        let cancellation = HistoryWindowCancellation::new();
        let mut scan = Scan::new(&cancellation);
        let index = VersionIndex::new(state, &request.history_tree_id, &mut scan)?;
        let files = index.component(&request.origin_file_id, &mut scan)?;
        let mut versions = files
            .iter()
            .flat_map(|file| index.files[file].iter().copied())
            .collect::<Vec<_>>();
        versions.sort_by(|a, b| a.version_id.cmp(&b.version_id));
        let selected = request
            .version_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let cohort_ids = versions
            .iter()
            .map(|v| v.version_id.as_str())
            .collect::<BTreeSet<_>>();
        if !selected.is_subset(&cohort_ids) {
            return Err(invalid());
        }
        let file_components = partition(
            &files,
            versions.iter().copied().map(Member::from),
            &selected,
            || scan.record(),
        )?;
        Ok(Self {
            files: files.iter().map(|id| (*id).to_owned()).collect(),
            cohort_version_ids: versions.iter().map(|v| v.version_id.clone()).collect(),
            cohort_digest: digest(&versions)?,
            file_components,
        })
    }

    pub(super) fn validate(&self, state: &State, request: &Request) -> Result<()> {
        if self.files.is_empty()
            || self.files.len() > MAX_SHARED_HISTORY_FILES
            || self.files.windows(2).any(|pair| pair[0] >= pair[1])
            || self.files.binary_search(&request.origin_file_id).is_err()
        {
            return Err(invalid());
        }
        for file in &self.files {
            HistoryFileId::parse(file)?;
        }
        let versions = cohort(state, request, &self.cohort_version_ids)?;
        let files = versions
            .iter()
            .map(|v| v.file_id.as_str())
            .collect::<BTreeSet<_>>();
        let selected = request
            .version_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if files
            .iter()
            .copied()
            .ne(self.files.iter().map(String::as_str))
            || selected.iter().any(|id| {
                self.cohort_version_ids
                    .binary_search_by(|v| v.as_str().cmp(id))
                    .is_err()
            })
            || digest(&versions)? != self.cohort_digest
        {
            return Err(invalid());
        }
        // Do not filter today's Retired tokens here. The original cohort is
        // frozen evidence, including versions retired by a later valid decision.
        let cancellation = HistoryWindowCancellation::new();
        let mut scan = Scan::new(&cancellation);
        if partition(
            &files,
            versions.iter().copied().map(Member::from),
            &BTreeSet::new(),
            || scan.record(),
        )? != vec![self.files.clone()]
            || partition(
                &files,
                versions.iter().copied().map(Member::from),
                &selected,
                || scan.record(),
            )? != self.file_components
        {
            return Err(invalid());
        }
        Ok(())
    }
}

/// Exact grouping published by one retirement decision, not the live component
/// today and not a claim about physical body cleanup or metadata erasure.
#[derive(Clone, Debug)]
pub struct PermanentNodeSeparation {
    source_history_root: HistoryRoot,
    original_files: Vec<HistoryFileId>,
    terminated_files: Vec<HistoryFileId>,
    file_components: Vec<Vec<HistoryFileId>>,
}
impl PermanentNodeSeparation {
    pub fn source_history_root(&self) -> HistoryRoot {
        self.source_history_root
    }
    pub fn original_files(&self) -> &[HistoryFileId] {
        &self.original_files
    }
    pub fn terminated_files(&self) -> &[HistoryFileId] {
        &self.terminated_files
    }
    pub fn file_components(&self) -> &[Vec<HistoryFileId>] {
        &self.file_components
    }
}

impl Decision {
    pub(crate) fn separation_observation(&self, state: &State) -> Result<PermanentNodeSeparation> {
        self.separation.validate(state, &self.request)?;
        self.file_terminations.validate(state, &self.request)?;
        Ok(PermanentNodeSeparation {
            source_history_root: HistoryRoot::parse(&self.request.history_root)?,
            original_files: self
                .separation
                .files
                .iter()
                .map(|id| HistoryFileId::parse(id))
                .collect::<storage_contract::Result<_>>()?,
            terminated_files: self
                .file_terminations
                .files
                .iter()
                .map(|file| HistoryFileId::parse(&file.file_id))
                .collect::<storage_contract::Result<_>>()?,
            file_components: self
                .separation
                .file_components
                .iter()
                .map(|component| {
                    component
                        .iter()
                        .map(|id| HistoryFileId::parse(id))
                        .collect::<storage_contract::Result<_>>()
                })
                .collect::<storage_contract::Result<_>>()?,
        })
    }
}
