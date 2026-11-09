//! A restored old identity needs its native displaced-occurrence proof, not a
//! generic known-ID exception. The complete State transition is replayed first.
use super::*;

impl CurrentNamespaces {
    pub(super) fn advance_file_restore(
        &self,
        previous: &State,
        next: &State,
    ) -> Result<Option<Self>> {
        let Some(restored) =
            crate::file_deletion::occurrence::verify_publication(previous, next, Some(self))?
        else {
            return Ok(None);
        };
        let mut view = self.clone();
        let id = &restored.main_branch_id;
        let branch = next.branches.get(id).ok_or_else(stale)?;
        let old = self.branches.get(id).ok_or_else(stale)?;
        let entries = &next
            .commits
            .get(&branch.head_commit_id)
            .ok_or_else(stale)?
            .entries;
        Arc::make_mut(&mut view.branches).insert(
            id.clone(),
            Arc::new(CurrentScope {
                head: branch.head_commit_id.clone(),
                entries: Arc::new(entries.clone()),
                source_head: old.source_head.clone(),
                source_entries: Arc::clone(&old.source_entries),
                writable: old.writable,
                hidden_paths: old.hidden_paths.clone(),
            }),
        );
        for (id, branch) in next
            .directory_branches
            .iter()
            .filter(|(id, branch)| previous.directory_branches.get(*id) != Some(*branch))
        {
            if branch.repository_id != restored.repository_id {
                return Err(stale());
            }
            self.require_directory_branch_write(previous, id)?;
            let old = self.directories.get(id).ok_or_else(stale)?;
            Arc::make_mut(&mut view.directories).insert(
                id.clone(),
                Arc::new(CurrentScope {
                    head: branch.head_tree_id.clone(),
                    entries: Arc::new(branch.entries.clone()),
                    source_head: old.source_head.clone(),
                    source_entries: Arc::clone(&old.source_entries),
                    writable: old.writable,
                    hidden_paths: old.hidden_paths.clone(),
                }),
            );
        }
        view.reserve_history_copy_identities(previous, next);
        Ok(Some(view))
    }
}
