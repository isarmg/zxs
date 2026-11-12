//! Bind permanent path facts to the exact before/after namespace occurrences.
//! The same proof is used for live Current publication and cold replay.
use crate::engine::CurrentNamespaces;
use crate::persist::path_changes::{self, PathChanged};
use crate::{Entry, NamespaceEntryKind, Result, State};
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::{EntryId, FileId, RepositoryId};

fn invalid() -> crate::Error {
    path_changes::invalid()
}

struct Occurrence {
    repository: RepositoryId,
    entry: Entry,
    branch_id: Option<String>,
}

mod batch;

fn occurrences(
    state: &State,
    current: Option<&CurrentNamespaces>,
) -> Result<Vec<Occurrence>> {
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
                let key = crate::business::context_key_from_key(
                    &repository.to_string(),
                    entry.lookup_key(),
                );
                result.push(Occurrence {
                    repository,
                    entry: entry.clone(),
                    branch_id: state.file_contexts.get(&key).map(|context| context.branch_id.clone()),
                });
            }
        }
    }
    Ok(result)
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct LocationKey {
    repository: RepositoryId,
    entry: EntryId,
    file: FileId,
    path: name_policy::PortablePath,
}

impl LocationKey {
    fn fact(fact: &PathChanged, location: &storage_contract::file_lifecycle::PathLocation) -> Self {
        Self {
            repository: location.repository_id,
            entry: fact.entry_id,
            file: fact.file_id,
            path: location.path.clone(),
        }
    }
}

struct Index<'a> {
    locations: BTreeMap<LocationKey, &'a Occurrence>,
    identities: BTreeSet<(EntryId, FileId)>,
}

impl<'a> Index<'a> {
    fn new(occurrences: &'a [Occurrence]) -> Result<Self> {
        let mut locations = BTreeMap::new();
        let mut identities = BTreeSet::new();
        for occurrence in occurrences {
            let key = LocationKey {
                repository: occurrence.repository,
                entry: occurrence.entry.entry_id.parse()?,
                file: occurrence.entry.file_id.parse()?,
                path: occurrence.entry.location().clone(),
            };
            identities.insert((key.entry, key.file));
            if let Some(old) = locations.insert(key, occurrence)
                && (old.entry != occurrence.entry || old.branch_id != occurrence.branch_id)
            {
                return Err(invalid());
            }
        }
        Ok(Self { locations, identities })
    }
}

fn verify_facts(
    previous: &State,
    next: &State,
    before: &[Occurrence],
    after: &[Occurrence],
    facts: &[PathChanged],
    require_vacated: bool,
) -> Result<()> {
    let before = Index::new(before)?;
    let after = Index::new(after)?;
    let mut consumed = BTreeMap::<LocationKey, usize>::new();
    let mut produced = BTreeMap::<LocationKey, usize>::new();
    for fact in facts {
        let source = LocationKey::fact(fact, &fact.before);
        let target = LocationKey::fact(fact, &fact.after);
        let old = before.locations.get(&source).ok_or_else(invalid)?;
        let new = after.locations.get(&target).ok_or_else(invalid)?;
        if source == target
            || require_vacated
                && (after.locations.contains_key(&source) || before.locations.contains_key(&target))
            || old.entry.version_id != fact.before_version_id.to_string()
            || new.entry.version_id != fact.after_version_id.to_string()
            || new.branch_id.as_deref() != Some(fact.branch_id.to_string().as_str())
        {
            return Err(invalid());
        }
        *consumed.entry(source).or_default() += 1;
        *produced.entry(target).or_default() += 1;
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
    for key in before.locations.keys() {
        if after.identities.contains(&(key.entry, key.file))
            && !after.locations.contains_key(key)
            && consumed.get(key) != Some(&1)
        {
            return Err(invalid());
        }
    }
    for key in after.locations.keys() {
        if before.identities.contains(&(key.entry, key.file))
            && !before.locations.contains_key(key)
            && produced.get(key) != Some(&1)
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
    if let Some(transfers) = path_changes::batch_transfers(receipt, principal)? {
        let (old, new) = batch::occurrences(previous, next, before, after, &transfers)?;
        verify_facts(previous, next, &old, &new, &facts, false)
    } else {
        let old = occurrences(previous, before)?;
        let new = occurrences(next, after)?;
        verify_facts(previous, next, &old, &new, &facts, true)
    }
}
