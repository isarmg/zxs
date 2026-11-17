//! One budget for all native ID requests in a draft decision; IDs are still
//! issued and consumed by the unmodified ordered FILE/directory publishers.
use super::{DraftError as Failure, allocations::Allocation, invalid};
use crate::{Error, NamespaceEntryKind, Result, State};
use std::collections::BTreeSet;
use storage_contract::file_draft::DraftLocation;

pub(crate) const MAX_ALLOCATIONS: usize = 65_536;

fn size() -> Error { Error::FileDraft(Failure::Size) }

#[derive(Default)]
struct Counts {
    born: usize,
    namespace: usize,
    files: usize,
    branches: usize,
    trees: usize,
}
impl Counts {
    fn length(&self) -> Result<usize> {
        self.born.checked_mul(2)
            .and_then(|n| self.namespace.checked_mul(2)?.checked_add(n))
            .and_then(|n| self.files.checked_mul(3)?.checked_add(n))
            .and_then(|n| n.checked_add(self.branches))
            .and_then(|n| n.checked_add(self.trees)).ok_or_else(size)
    }
}

/// Count distinct possible lazy FILE roots across all ancestor scans, not a
/// fresh allowance per selected file. The root scan bounds all existing FILEs.
/// Already selected roots cannot be allocated; actual prior/context/main branch
/// reuse may narrow this upper bound still further. A repeated parent may
/// create another *tree*, but never another initial branch for the same ID.
pub(crate) fn limit<'a>(
    previous: &State, changed: impl IntoIterator<Item = (&'a DraftLocation, bool)>,
) -> Result<usize> {
    let mut counts = Counts::default();
    let mut repository = None;
    let mut new_dirs = BTreeSet::new();
    let mut new_files = BTreeSet::new();
    for (target, born) in changed {
        if repository.replace(target.repository).is_some_and(|repo| repo != target.repository) {
            return Err(invalid());
        }
        let record = previous.file_containers.get(&target.file).ok_or_else(invalid)?;
        let occurrence = super::ordinary::OrdinaryOccurrence::capture(previous, target.file, record)?;
        occurrence.require_target(target)?;
        counts.born = counts.born.checked_add(usize::from(born)).ok_or_else(size)?;
        counts.namespace = counts.namespace.checked_add(1).ok_or_else(size)?;
        if !born { new_files.insert(target.file.to_string()); }
        let repo = target.repository.to_string();
        if !previous.directory_selections.contains_key(&repo) { new_dirs.insert(repo); }
        counts.trees = counts.trees.checked_add(1 + occurrence.parents.len()).ok_or_else(size)?;
        for (entry, _) in &occurrence.parents {
            let id = entry.to_string();
            if !previous.directory_selections.contains_key(&id) { new_dirs.insert(id); }
        }
    }
    let Some(repository) = repository else { return Ok(0); };
    let repo = previous.repositories.get(&repository.to_string()).ok_or_else(invalid)?;
    let main = previous.branches.get(&repo.main_branch_id).ok_or_else(invalid)?;
    let entries = &previous.commits.get(&main.head_commit_id).ok_or_else(invalid)?.entries;
    // ensure_shared_file_branch_with_ids reuses either an existing globally
    // selected root, that occurrence's FILE context, or an existing FILE main.
    let mains: BTreeSet<_> = previous.branches.values().filter(|branch| branch.name == "main")
        .filter_map(|branch| branch.file_id.as_deref()).collect();
    for entry in entries.iter().filter(|entry| entry.kind == NamespaceEntryKind::File) {
        if !previous.file_selections.contains_key(&entry.file_id)
            && !mains.contains(entry.file_id.as_str())
            && !super::super::directory::context_at_key(previous, &repo.repository_id, entry.lookup_key())
                .is_some_and(|context| previous.branches.get(&context.branch_id)
                    .is_some_and(|branch| branch.file_id.as_ref() == Some(&entry.file_id)))
        {
            new_files.insert(entry.file_id.clone());
        }
    }
    counts.branches = new_dirs.len();
    counts.files = new_files.len();
    let upper = counts.length()?;
    if upper > MAX_ALLOCATIONS { return Err(size()); }
    Ok(upper)
}

/// For an actual changed item, B and M come from the real branch transitions,
/// F/D/T from the native allocation stream. A length check alone cannot admit
/// it: Replay still consumes each ordered typed ID against the accumulating
/// State, rejects collisions, and requires exact exhaustion.
pub(super) fn require_item(
    allocations: &[Allocation], born: bool, namespace_changed: bool, target: &DraftLocation,
) -> Result<()> {
    let mut counts = Counts { born: usize::from(born), namespace: usize::from(namespace_changed),
        ..Counts::default() };
    let mut commits = 0;
    let mut trees = 0;
    if allocations.len() > MAX_ALLOCATIONS { return Err(size()); }
    for allocation in allocations {
        match allocation {
            Allocation::Branch(_) => counts.files += 1,
            Allocation::Commit(_) => commits += 1,
            Allocation::Tree(_) => trees += 1,
            Allocation::DirectoryBranch(_) => counts.branches += 1,
            Allocation::DirectoryTree(_) => counts.trees += 1,
        }
    }
    let visits = target.path.as_str().split('/').count();
    if commits != trees || counts.branches > visits || counts.trees > visits
        || counts.length()? != allocations.len()
    {
        return Err(invalid());
    }
    Ok(())
}

pub(super) fn namespace_head<'a>(state: &'a State, target: &DraftLocation) -> Result<&'a str> {
    let repo = state.repositories.get(&target.repository.to_string()).ok_or_else(invalid)?;
    Ok(&state.branches.get(&repo.main_branch_id).ok_or_else(invalid)?.head_commit_id)
}
