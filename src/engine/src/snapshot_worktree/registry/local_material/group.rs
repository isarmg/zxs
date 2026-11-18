//! Build a temporary per-repository grouping in one pass over each already
//! authenticated native family. Never rescan every edit/node for every host.
use super::*;
pub(super) struct Group<'r,'s> {
    pub(super) initial: &'r NodeRecord<'s>,
    pub(super) heads: Vec<(SnapshotBranchId, SnapshotHandle)>, pub(super) locked_branch: bool,
    pub(super) working: Vec<&'r WorkingRecord<'s>>, pub(super) trees: Vec<&'r WorktreeRecord<'s>>,
    pub(super) edits: Vec<&'r EditRecordEntry<'s>>,
}
impl<'s> ReceiptIndex<'s> {
    pub(super) fn scope_groups<'r>(&'r self, select: impl Fn(&NodeRecord<'s>) -> bool)
        -> Result<BTreeMap<SnapshotRepositoryId, Group<'r,'s>>> {
        let mut groups = BTreeMap::new();
        let mut count = 0usize;
        let mut add = || -> Result<()> { count = count.checked_add(1).filter(|n| *n <= 65_536).ok_or(RegistrationError::Source)?; Ok(()) };
        for n in self.nodes.values().filter(|n| select(n)) {
            add()?;
            let g = Group { initial: n, heads: vec![], locked_branch: false, working: vec![], trees: vec![], edits: vec![] };
            if groups.insert(n.plan.allocated.repository, g).is_some() { return Err(RegistrationError::Identity); }
        }
        if groups.is_empty() { return Ok(groups); }
        for (&(repo, branch), &h) in &self.branch_heads {
            if let Some(g) = groups.get_mut(&repo) { add()?; g.heads.push((branch,h)); }
        }
        for &(repo, _) in self.pending_branches.keys() {
            if let Some(g) = groups.get_mut(&repo) { g.locked_branch = true; }
        }
        for w in self.working.values() {
            if let Some((n, _)) = w.history_node() && let Some(g) = groups.get_mut(&n.handle().repository_id) {
                add()?; g.working.push(w);
            }
        }
        for w in self.worktrees.values() {
            if let Some(g) = groups.get_mut(&w.plan.request.source.repository_id) { add()?; g.trees.push(w); }
        }
        for e in self.edits.values() {
            let target = e.plan.request.target;
            let Some(CompletionOwner::Worktree(at)) = self.completions.get(&target.publication)
                else { return Err(RegistrationError::Source); };
            let wt = self.worktrees.get(at).ok_or(RegistrationError::Source)?;
            if wt.plan.allocated.worktree != target.worktree_id { return Err(RegistrationError::Identity); }
            if let Some(g) = groups.get_mut(&wt.plan.request.source.repository_id) { add()?; g.edits.push(e); }
        }
        Ok(groups)
    }
}
