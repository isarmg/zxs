//! Stable retained endpoints, independent of today's namespace and selection.
use crate::State;
use crate::persist::path_changes::invalid;
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::{BranchId, FileId, HistoryTreeId, Sequence, VersionId};

pub(super) struct References<'a> {
    state: &'a State,
    pending: Option<BTreeMap<(HistoryTreeId, FileId, VersionId), Sequence>>,
}

impl<'a> References<'a> {
    pub(super) fn new(state: &'a State) -> Self {
        Self { state, pending: None }
    }

    pub(super) fn check(
        &mut self,
        file: FileId,
        history: HistoryTreeId,
        branch: BranchId,
        versions: [VersionId; 2],
        published: Sequence,
        checkpoint: &mut impl FnMut() -> crate::Result<()>,
    ) -> crate::Result<()> {
        checkpoint()?;
        let branch = self.state.branches.get(&branch.to_string()).ok_or_else(invalid)?;
        if branch.file_id.as_deref() != Some(file.to_string().as_str())
            || branch.history_tree_id != history.to_string()
            || !self.state.histories.contains_key(&history.to_string())
        {
            return Err(invalid());
        }
        for id in versions {
            let version = self.state.versions.get(&id.to_string()).ok_or_else(invalid)?;
            if Sequence::new(version.version_sequence) > published {
                return Err(invalid());
            }
            if version.file_id == file.to_string()
                && version.history_tree_id == history.to_string()
            {
                continue;
            }
            // Pending selections can be consumed by a later save. Their old
            // immutable commit, not the mutable pending map or current HEAD,
            // must still prove this file's use of that exact borrowed body.
            self.index_pending(checkpoint)?;
            if self.pending.as_ref().and_then(|pending| pending.get(&(history, file, id)))
                .is_none_or(|sequence| *sequence > published)
            {
                return Err(invalid());
            }
        }
        Ok(())
    }

    fn index_pending(
        &mut self,
        checkpoint: &mut impl FnMut() -> crate::Result<()>,
    ) -> crate::Result<()> {
        if self.pending.is_some() {
            return Ok(());
        }
        let mut pending = BTreeMap::<(HistoryTreeId, FileId, VersionId), Sequence>::new();
        for commit in self.state.commits.values() {
            checkpoint()?;
            if commit.pending_file_versions.is_empty() {
                continue;
            }
            let mut members: BTreeSet<(FileId, VersionId)> = BTreeSet::new();
            for entry in &commit.entries {
                checkpoint()?;
                if entry.kind == crate::NamespaceEntryKind::File {
                    members.insert((entry.file_id.parse()?, entry.version_id.parse()?));
                }
            }
            for (file, version) in &commit.pending_file_versions {
                checkpoint()?;
                let member = (file.parse()?, version.parse()?);
                if !members.contains(&member) {
                    return Err(invalid());
                }
                let key = (commit.history_tree_id.parse()?, member.0, member.1);
                pending
                    .entry(key)
                    .and_modify(|sequence| {
                        *sequence = (*sequence).min(Sequence::new(commit.sequence));
                    })
                    .or_insert(Sequence::new(commit.sequence));
            }
        }
        self.pending = Some(pending);
        Ok(())
    }
}
