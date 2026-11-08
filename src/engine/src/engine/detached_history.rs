//! Current-namespace discovery with no disk I/O and no retirement permission.
use super::{Engine, MetadataView};
use crate::{ContentState, Error, ErrorCode, FileVersion, NamespaceEntryKind, Result};
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::detached_history::{self as wire, File, Page, Request};
use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId, HistoryRoot, HistoryTreeId, HistoryVersionId, HistoryWindowCancellation};

const MAX_STEPS: usize = 1_048_576;
const MAX_FILES: usize = 65_536;
const MAX_HISTORIES: usize = 8192;

struct Scan<'a> { cancellation: &'a HistoryWindowCancellation, steps: usize }
impl Scan<'_> {
    fn step(&mut self) -> Result<()> {
        if self.cancellation.is_cancelled() {
            return Err(Error::new(ErrorCode::OperationAborted, "detached history discovery cancelled"));
        }
        if self.steps == MAX_STEPS {
            return Err(Error::new(ErrorCode::ResourceLimit, "detached history discovery exceeds its scan budget"));
        }
        self.steps += 1;
        Ok(())
    }
}
fn invalid(message: &str) -> Error { Error::new(ErrorCode::StateInvalid, message) }
fn changed() -> Error { Error::new(ErrorCode::HistoryChanged, "detached history discovery changed; restart from its first page") }

pub struct DetachedHistoryRead {
    view: MetadataView,
    request: Request,
    cancellation: HistoryWindowCancellation,
}
impl Engine {
    pub fn capture_detached_history(&self, request: Request, cancellation: HistoryWindowCancellation) -> Result<DetachedHistoryRead> {
        request.validate()?;
        Ok(DetachedHistoryRead { view: self.metadata_view(), request, cancellation })
    }
    pub fn recheck_detached_history(&self, read: &DetachedHistoryRead) -> Result<()> {
        if read.cancellation.is_cancelled() {
            return Err(Error::new(ErrorCode::OperationAborted, "detached history discovery cancelled"));
        }
        let previous = read.view.business_projection();
        if self.state.sequence != previous.sequence || self.state.business_generation != previous.business_generation
            || !self.namespace_read_view().same_publication(&read.view.current_namespaces) { return Err(changed()); }
        Ok(())
    }
}

impl DetachedHistoryRead {
    /// Prepare outside the Engine owner lock, then recheck current authorization,
    /// state generation and namespace publication before sending the response.
    pub fn page(&self) -> Result<Page> {
        let mut scan = Scan { cancellation: &self.cancellation, steps: 0 };
        scan.step()?;
        let state = self.view.business_projection();
        let mut roots = BTreeMap::new();
        let mut present = BTreeSet::new();
        for repository in state.repositories.values() {
            scan.step()?;
            if roots.len() == MAX_HISTORIES {
                return Err(Error::new(ErrorCode::ResourceLimit, "detached history discovery exceeds its repository budget"));
            }
            let root = HistoryRoot::parse(self.view.history_root(&repository.history_tree_id)?)?;
            if roots.insert(repository.history_tree_id.as_str(), (repository.repository_id.as_str(), root)).is_some() {
                return Err(invalid("multiple repositories claim one discovery history"));
            }
            for entry in self.view.namespace_tree(&repository.main_branch_id)?.entries() {
                scan.step()?;
                if entry.kind != NamespaceEntryKind::File { continue; }
                present.insert(entry.file_id.as_str());
                if present.len() > MAX_FILES {
                    return Err(Error::new(ErrorCode::ResourceLimit, "current namespace exceeds the discovery identity budget"));
                }
            }
        }
        let snapshot = HistoryRoot::parse(&crate::object_hash("detached-history-discovery", &json_codec::json!({
            "sequence": state.sequence.to_string(), "business_generation": state.business_generation.to_string(),
            "histories": roots, "current_files": present,
        }))?)?;
        if self.request.snapshot.is_some_and(|expected| expected != snapshot) { return Err(changed()); }
        let candidates = candidates(state, &present, &roots, &mut scan)?;
        let mut files = Vec::new();
        let mut has_more = false;
        for (file_id, (version, count)) in candidates {
            scan.step()?;
            if self.request.after_file.as_ref().is_some_and(|after| file_id <= after.as_str()) { continue; }
            if files.len() == wire::PAGE_SIZE { has_more = true; break; }
            let (repository, root) = roots[version.history_tree_id.as_str()];
            files.push(File {
                file_id: HistoryFileId::parse(file_id)?, repository_id: HistoryRepositoryId::parse(repository)?,
                history_tree_id: HistoryTreeId::parse(&version.history_tree_id)?, history_root: root,
                representative_version_id: HistoryVersionId::parse(&version.version_id)?,
                recorded_name: version.creation_name.clone(), representative_size: version.logical_size,
                retained_versions: count,
            });
        }
        let next_after = has_more.then(|| files.last().expect("invariant: a further candidate follows a complete page").file_id.clone());
        scan.step()?;
        let page = Page { snapshot, files, next_after };
        page.validate(&self.request)?;
        Ok(page)
    }
}

fn candidates<'a>(
    state: &'a crate::State, present: &BTreeSet<&str>, roots: &BTreeMap<&str, (&str, HistoryRoot)>, scan: &mut Scan<'_>,
) -> Result<BTreeMap<&'a str, (&'a FileVersion, u64)>> {
    let mut result = BTreeMap::<&str, (&FileVersion, u64)>::new();
    for version in state.versions.values() {
        scan.step()?;
        if present.contains(version.file_id.as_str()) || state.file_deletions.contains_key(&version.file_id)
            || !roots.contains_key(version.history_tree_id.as_str()) { continue; }
        match state.content_states.get(&version.version_id) {
            Some(ContentState::Available | ContentState::Purged) => {}
            Some(ContentState::Migrated | ContentState::Retired) => continue,
            None => return Err(invalid("discovery version has no authenticated content state")),
        }
        if let Some((chosen, count)) = result.get_mut(version.file_id.as_str()) {
            if chosen.history_tree_id != version.history_tree_id { return Err(invalid("stable file crosses discovery histories")); }
            *count += 1; // bounded by the scan budget
            if (version.version_sequence, version.version_id.as_str()) > (chosen.version_sequence, chosen.version_id.as_str()) {
                *chosen = version;
            }
        } else {
            if result.len() == MAX_FILES {
                return Err(Error::new(ErrorCode::ResourceLimit, "detached history discovery exceeds its complete identity budget"));
            }
            result.insert(&version.file_id, (version, 1));
        }
    }
    Ok(result)
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/detached_history_tests.rs"]
mod tests;
