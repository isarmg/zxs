//! Bind permanent path facts to the exact before/after namespace occurrences.
//! The same proof is used for live Current publication and cold replay.
use crate::engine::CurrentNamespaces;
use crate::persist::path_changes::{self, PathChanged};
use crate::{Entry, NamespaceEntryKind, Result, State};
use storage_contract::RepositoryId;

fn invalid() -> crate::Error {
    path_changes::invalid()
}

struct Occurrence<'a> {
    repository: RepositoryId,
    entry: &'a Entry,
}

fn occurrences<'a>(
    state: &'a State,
    current: Option<&'a CurrentNamespaces>,
) -> Result<Vec<Occurrence<'a>>> {
    let mut result = Vec::new();
    for repository in state.repositories.values() {
        let entries = match current {
            Some(current) => {
                let Some(scope) = current.branches.get(&repository.main_branch_id) else {
                    continue;
                };
                scope.entries.as_slice()
            }
            None => {
                let branch = state.branches.get(&repository.main_branch_id).ok_or_else(invalid)?;
                &state.commits.get(&branch.head_commit_id).ok_or_else(invalid)?.entries
            }
        };
        let repository = repository.repository_id.parse::<RepositoryId>()?;
        for entry in entries {
            if entry.kind == NamespaceEntryKind::File
                && !state.file_deletions.contains_key(&entry.file_id)
            {
                result.push(Occurrence { repository, entry });
            }
        }
    }
    Ok(result)
}

fn same_identity(left: &Occurrence<'_>, right: &Occurrence<'_>) -> bool {
    left.entry.entry_id == right.entry.entry_id && left.entry.file_id == right.entry.file_id
}

fn same_location(left: &Occurrence<'_>, right: &Occurrence<'_>) -> bool {
    left.repository == right.repository && left.entry.path() == right.entry.path()
}

fn matches_before(fact: &PathChanged, occurrence: &Occurrence<'_>) -> bool {
    fact.before.repository_id == occurrence.repository
        && fact.before.path.as_str() == occurrence.entry.path()
        && fact.file_id.to_string() == occurrence.entry.file_id
        && fact.entry_id.to_string() == occurrence.entry.entry_id
        && fact.before_version_id.to_string() == occurrence.entry.version_id
}

fn matches_after(fact: &PathChanged, occurrence: &Occurrence<'_>) -> bool {
    fact.after.repository_id == occurrence.repository
        && fact.after.path.as_str() == occurrence.entry.path()
        && fact.file_id.to_string() == occurrence.entry.file_id
        && fact.entry_id.to_string() == occurrence.entry.entry_id
        && fact.after_version_id.to_string() == occurrence.entry.version_id
}

fn verify_facts(
    previous: &State,
    next: &State,
    before: &[Occurrence<'_>],
    after: &[Occurrence<'_>],
    facts: &[PathChanged],
) -> Result<()> {
    for fact in facts {
        let old = before.iter().find(|entry| matches_before(fact, entry)).ok_or_else(invalid)?;
        let new = after.iter().find(|entry| matches_after(fact, entry)).ok_or_else(invalid)?;
        if same_location(old, new)
            || after.iter().any(|entry| same_identity(old, entry) && same_location(old, entry))
            || before.iter().any(|entry| same_identity(new, entry) && same_location(new, entry))
        {
            return Err(invalid());
        }
        let branch = next.branches.get(&fact.branch_id.to_string()).ok_or_else(invalid)?;
        let version = next.versions.get(&new.entry.version_id).ok_or_else(invalid)?;
        if branch.file_id.as_deref() != Some(new.entry.file_id.as_str())
            || branch.history_tree_id != fact.history_tree_id.to_string()
        {
            return Err(invalid());
        }
        if version.file_id != new.entry.file_id {
            // A pending file may borrow an already-admitted source body. Moving
            // it preserves that exact pending selection, not source ownership.
            if old.entry.version_id != new.entry.version_id
                || branch.pending_file_versions.get(&new.entry.file_id)
                    != Some(&new.entry.version_id)
            {
                return Err(invalid());
            }
        } else if version.history_tree_id != fact.history_tree_id.to_string() {
            return Err(invalid());
        }
        if old.entry.version_id != new.entry.version_id
            && (previous.versions.contains_key(&new.entry.version_id)
                || version.version_sequence != next.sequence
                || version.source_version_ids.as_slice() != [old.entry.version_id.as_str()])
        {
            return Err(invalid());
        }
    }
    // A retained alias is not evidence that a different occurrence was moved.
    // Reserve unchanged locations first and require one fact for each consumed
    // source and destination. One operation may legitimately move many aliases.
    for old in before {
        if after.iter().any(|new| same_identity(old, new))
            && !after.iter().any(|new| same_identity(old, new) && same_location(old, new))
            && facts.iter().filter(|fact| matches_before(fact, old)).count() != 1
        {
            return Err(invalid());
        }
    }
    for new in after {
        if before.iter().any(|old| same_identity(old, new))
            && !before.iter().any(|old| same_identity(old, new) && same_location(old, new))
            && facts.iter().filter(|fact| matches_after(fact, new)).count() != 1
        {
            return Err(invalid());
        }
    }
    Ok(())
}

pub(in crate::engine) fn verify(
    previous: &State,
    next: &State,
    before: Option<&CurrentNamespaces>,
    after: Option<&CurrentNamespaces>,
) -> Result<()> {
    if !next.file_path_nodes.is_empty() {
        return Err(invalid());
    }
    let receipt = crate::engine::disk_registry::current::publication_delta::receipt(
        previous,
        next,
        |receipt| path_changes::is_kind(&receipt.kind),
        invalid,
    )?;
    let Some(receipt) = receipt else {
        return Ok(());
    };
    let facts = path_changes::read(receipt)?;
    let principal = next.operation_principals.get(&receipt.operation_id).ok_or_else(invalid)?;
    if receipt.sequence != next.sequence || next.sequence <= previous.sequence {
        return Err(invalid());
    }
    for fact in &facts {
        if fact.author.as_str() != principal {
            return Err(invalid());
        }
    }
    let old = occurrences(previous, before)?;
    let new = occurrences(next, after)?;
    verify_facts(previous, next, &old, &new, &facts)
}
