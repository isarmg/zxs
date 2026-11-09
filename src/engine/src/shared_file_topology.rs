//! Shared nonempty-content topology over stable file identities. Every parallel
//! equality bucket remains live until its last cross-file membership is removed.
use crate::{Error, FileVersion, Result, State};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use storage_contract::history_window::HistoryWindowCancellation;
use storage_contract::shared_history::{MAX_SHARED_HISTORY_FILES, MAX_SHARED_HISTORY_NODES};

const MAX_SCAN_RECORDS: usize = 1_048_576;
const MAX_INDEX_VERSIONS: usize = 65_536;
pub(crate) type ContentKey = (storage_contract::ContentHash, u64);

pub(crate) fn exhausted() -> Error {
    Error::new(
        storage_contract::ErrorCode::ResourceLimit,
        "shared history exceeds its complete-component lookup budget",
    )
}

pub(crate) struct Scan<'a> {
    remaining: usize,
    cancellation: &'a HistoryWindowCancellation,
}
impl<'a> Scan<'a> {
    pub(crate) fn new(cancellation: &'a HistoryWindowCancellation) -> Self {
        Self {
            remaining: MAX_SCAN_RECORDS,
            cancellation,
        }
    }
    pub(crate) fn record(&mut self) -> Result<()> {
        if self.cancellation.is_cancelled() {
            return Err(Error::new(
                storage_contract::ErrorCode::OperationAborted,
                "shared topology query cancelled",
            ));
        }
        if self.remaining == 0 {
            return Err(exhausted());
        }
        self.remaining -= 1;
        Ok(())
    }
}

pub(crate) struct VersionIndex<'a> {
    pub(crate) files: BTreeMap<&'a str, Vec<&'a FileVersion>>,
    pub(crate) contents: BTreeMap<ContentKey, Vec<&'a FileVersion>>,
}
impl<'a> VersionIndex<'a> {
    pub(crate) fn new(state: &'a State, history: &str, scan: &mut Scan<'_>) -> Result<Self> {
        let mut index = Self {
            files: BTreeMap::new(),
            contents: BTreeMap::new(),
        };
        let mut count = 0;
        for version in state.versions.values() {
            scan.record()?;
            if version.history_tree_id != history
                || crate::node_retirement_engine::is_retired(state, &version.version_id)
            {
                continue;
            }
            if count == MAX_INDEX_VERSIONS {
                return Err(exhausted());
            }
            count += 1;
            index
                .files
                .entry(&version.file_id)
                .or_default()
                .push(version);
            if version.logical_size != 0 {
                index
                    .contents
                    .entry(content_key(version))
                    .or_default()
                    .push(version);
            }
        }
        Ok(index)
    }

    pub(crate) fn component(&self, origin: &str, scan: &mut Scan<'_>) -> Result<BTreeSet<&'a str>> {
        let origin = self
            .files
            .get_key_value(origin)
            .ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::InvalidRequest,
                    "shared component origin has no live version",
                )
            })?
            .0;
        let mut files = BTreeSet::from([*origin]);
        let mut queue = VecDeque::from([*origin]);
        let mut seen = BTreeSet::new();
        let mut versions = 0;
        while let Some(file) = queue.pop_front() {
            for version in &self.files[file] {
                scan.record()?;
                if versions == MAX_SHARED_HISTORY_NODES {
                    return Err(exhausted());
                }
                versions += 1;
                if version.logical_size == 0 || !seen.insert(content_key(version)) {
                    continue;
                }
                for related in &self.contents[&content_key(version)] {
                    scan.record()?;
                    if !files.contains(related.file_id.as_str()) {
                        if files.len() == MAX_SHARED_HISTORY_FILES {
                            return Err(exhausted());
                        }
                        files.insert(&related.file_id);
                        queue.push_back(&related.file_id);
                    }
                }
            }
        }
        Ok(files)
    }
}

pub(crate) fn content_key(version: &FileVersion) -> ContentKey {
    (version.content_hash, version.logical_size)
}

pub(crate) struct Member<'a> {
    pub(crate) file: &'a str,
    pub(crate) version: &'a str,
    pub(crate) content: ContentKey,
}
impl<'a> From<&'a FileVersion> for Member<'a> {
    fn from(version: &'a FileVersion) -> Self {
        Self {
            file: &version.file_id,
            version: &version.version_id,
            content: content_key(version),
        }
    }
}

/// Partition a fixed, complete cohort. The exclusion is a body identity, not a
/// content hash: another version of equal bytes must continue to join its files.
/// Files without a remaining equality edge are singletons, never split internally.
pub(crate) fn partition<'a>(
    files: &BTreeSet<&'a str>,
    versions: impl IntoIterator<Item = Member<'a>>,
    excluded: &BTreeSet<&str>,
    mut step: impl FnMut() -> Result<()>,
) -> Result<Vec<Vec<String>>> {
    if files.is_empty() || files.len() > MAX_SHARED_HISTORY_FILES {
        return Err(exhausted());
    }
    let mut groups = BTreeMap::<ContentKey, BTreeSet<&str>>::new();
    for (count, version) in versions.into_iter().enumerate() {
        step()?;
        if count == MAX_SHARED_HISTORY_NODES || !files.contains(version.file) {
            return Err(exhausted());
        }
        if version.content.1 != 0 && !excluded.contains(version.version) {
            groups
                .entry(version.content)
                .or_default()
                .insert(version.file);
        }
    }
    let mut memberships = BTreeMap::<&str, Vec<ContentKey>>::new();
    for (key, members) in &groups {
        for file in members {
            step()?;
            memberships.entry(*file).or_default().push(*key);
        }
    }
    let mut seen = BTreeSet::new();
    let mut expanded = BTreeSet::new();
    let mut result = Vec::new();
    for &file in files {
        step()?;
        if !seen.insert(file) {
            continue;
        }
        let mut component = BTreeSet::from([file]);
        let mut queue = VecDeque::from([file]);
        while let Some(file) = queue.pop_front() {
            step()?;
            if let Some(keys) = memberships.get(file) {
                for key in keys {
                    step()?;
                    if expanded.insert(*key) {
                        for &other in &groups[key] {
                            step()?;
                            if seen.insert(other) {
                                component.insert(other);
                                queue.push_back(other);
                            }
                        }
                    }
                }
            }
        }
        result.push(component.into_iter().map(str::to_owned).collect());
    }
    Ok(result)
}
