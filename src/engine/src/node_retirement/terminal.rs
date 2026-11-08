//! Last-node decisions close a detached stable identity, not only its old bodies.
//! Current namespace/HEAD/D references are still rejected by the common guard.
use super::*;

const MAX_TERMINAL_VERSIONS: usize = 65_536;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct TerminalFile {
    pub(super) file_id: String,
    version_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct TerminalFiles {
    pub(super) files: Vec<TerminalFile>,
}

fn retained_versions<'a>(state: &'a State, files: &BTreeSet<&str>) -> Result<BTreeMap<&'a str, Vec<&'a str>>> {
    let mut result = BTreeMap::<&str, Vec<&str>>::new();
    let mut count = 0;
    for version in state.versions.values() {
        if !files.contains(version.file_id.as_str()) { continue; }
        if count == MAX_TERMINAL_VERSIONS {
            return Err(Error::new(ErrorCode::ResourceLimit, "terminal file history exceeds its complete identity budget"));
        }
        count += 1;
        result.entry(&version.file_id).or_default().push(&version.version_id);
    }
    for ids in result.values_mut() { ids.sort(); }
    Ok(result)
}

fn require_detached(state: &State, file: &str) -> Result<()> {
    if state.file_deletions.contains_key(file)
        || state.file_selections.contains_key(file)
        || state.file_write_origins.contains_key(file)
        || state.branches.values().any(|branch| branch.file_id.as_deref() == Some(file))
        || state.file_branch_merges.values().any(|event| event.file_id == file) {
        return Err(Error::new(ErrorCode::ContentPinned,
            "last-node retirement requires a detached file; retained file branches or a deletion frontier require whole-file termination"));
    }
    Ok(())
}

impl TerminalFiles {
    pub(super) fn capture(state: &State, request: &Request) -> Result<Self> {
        let selected = request.version_ids.iter().map(String::as_str).collect::<BTreeSet<_>>();
        let files = request.version_ids.iter().map(|id| state.versions[id].file_id.as_str()).collect::<BTreeSet<_>>();
        let mut ended = Vec::new();
        for (file, versions) in retained_versions(state, &files)? {
            if versions.iter().any(|id| !selected.contains(id) && !is_retired(state, id)) { continue; }
            require_detached(state, file)?;
            if versions.iter().any(|id| state.versions[*id].history_tree_id != request.history_tree_id) { return Err(invalid()); }
            ended.push(TerminalFile { file_id: file.into(), version_ids: versions.into_iter().map(str::to_owned).collect() });
        }
        Ok(Self { files: ended })
    }

    pub(super) fn validate(&self, state: &State, request: &Request) -> Result<()> {
        if self.files.len() > storage_contract::shared_history::MAX_SHARED_HISTORY_FILES
            || self.files.windows(2).any(|pair| pair[0].file_id >= pair[1].file_id) { return Err(invalid()); }
        let files = self.files.iter().map(|file| file.file_id.as_str()).collect::<BTreeSet<_>>();
        let versions = retained_versions(state, &files)?;
        let mut total = 0;
        for file in &self.files {
            HistoryFileId::parse(&file.file_id)?;
            require_detached(state, &file.file_id)?;
            total += file.version_ids.len();
            if file.version_ids.is_empty() || total > MAX_TERMINAL_VERSIONS
                || file.version_ids.windows(2).any(|pair| pair[0] >= pair[1])
                || !request.version_ids.iter().any(|id| state.versions[id].file_id == file.file_id)
                || versions.get(file.file_id.as_str()).is_none_or(|ids| !ids.iter().copied().eq(file.version_ids.iter().map(String::as_str))) {
                return Err(invalid());
            }
            for id in &file.version_ids {
                HistoryVersionId::parse(id)?;
                if !is_retired(state, id) || state.versions[id].history_tree_id != request.history_tree_id { return Err(invalid()); }
            }
        }
        Ok(())
    }
}

/// Every completely retired stable file has exactly one permanent terminal
/// decision. New versions with that file_id cannot escape by using new IDs or
/// empty source lists: each terminal receipt freezes the complete version set.
pub(super) fn require_complete(state: &State, terminals: &BTreeSet<String>) -> Result<()> {
    let mut all = BTreeSet::new();
    let mut live = BTreeSet::new();
    for version in state.versions.values() {
        all.insert(version.file_id.as_str());
        if !is_retired(state, &version.version_id) { live.insert(version.file_id.as_str()); }
    }
    if all.difference(&live).copied().ne(terminals.iter().map(String::as_str)) { return Err(invalid()); }
    Ok(())
}
