//! One immutable commit transformation for file forks and aggregate moves.
//! Callers retain their distinct member closure and missing-identity error.
use super::*;

pub(in crate::business) struct CommitScope<'a> {
    pub ids: &'a BTreeMap<String, String>,
    pub files: BTreeSet<&'a str>,
    pub directories: BTreeSet<&'a str>,
    pub trees: BTreeSet<&'a str>,
    pub missing: fn() -> Error,
}

impl CommitScope<'_> {
    fn mapped(&self, id: &str) -> Result<String> {
        self.ids.get(id).cloned().ok_or_else(self.missing)
    }

    fn contains(&self, entry: &Entry) -> bool {
        self.files.contains(entry.file_id.as_str())
            || self.directories.contains(entry.entry_id.as_str())
    }

    fn entries(&self, entries: &[Entry]) -> Result<Vec<Entry>> {
        let mut paths = BTreeSet::new();
        for entry in entries.iter().filter(|entry| {
            (entry.kind == NamespaceEntryKind::File && self.files.contains(entry.file_id.as_str()))
                || self.directories.contains(entry.entry_id.as_str())
        }) {
            let mut path = entry.lookup_key();
            loop {
                if !paths.insert(path) {
                    break;
                }
                let Some((parent, _)) = path.rsplit_once('/') else {
                    break;
                };
                path = parent;
            }
        }
        entries
            .iter()
            .filter(|entry| paths.contains(entry.lookup_key()))
            .map(|entry| {
                let mut entry = entry.clone();
                entry.entry_id = self.mapped(&entry.entry_id)?;
                if entry.kind == NamespaceEntryKind::File {
                    entry.file_id = self.mapped(&entry.file_id)?;
                    entry.version_id = self.mapped(&entry.version_id)?;
                }
                Ok(entry)
            })
            .collect()
    }
}

pub(in crate::business) fn project<'a>(
    previous: &State,
    next: &mut State,
    target: &Repository,
    originals: impl IntoIterator<Item = &'a Commit>,
    scope: CommitScope<'_>,
) -> Result<()> {
    for original in originals {
        let mut commit = original.clone();
        commit.commit_id = scope.mapped(&original.commit_id)?;
        commit.tree_id = scope.mapped(&original.tree_id)?;
        commit.history_tree_id = target.history_tree_id.clone();
        commit.parent_commit_ids = original
            .parent_commit_ids
            .iter()
            .map(|id| scope.mapped(id))
            .collect::<Result<_>>()?;
        let retained_directory_commit = original.kind == "directory_commit"
            && original
                .provenance
                .as_deref()
                .is_some_and(|tree| scope.trees.contains(tree));
        if !retained_directory_commit {
            if commit.parent_commit_ids.is_empty() {
                commit.parent_commit_ids.push(
                    next.histories[&target.history_tree_id]
                        .root_commit_id
                        .clone(),
                );
                commit.kind = "file_history_projection".into();
            } else if original.kind == "directory_commit" {
                commit.kind = "file_history_projection".into();
            }
        }
        commit.entries = scope.entries(&original.entries)?;
        commit.pending_file_versions = original
            .pending_file_versions
            .iter()
            .filter(|(file, _)| scope.files.contains(file.as_str()))
            .map(|(file, version)| Ok((scope.mapped(file)?, scope.mapped(version)?)))
            .collect::<Result<_>>()?;
        // Provenance outside the copied closure is not imported as authority.
        commit.provenance = original
            .provenance
            .as_ref()
            .and_then(|id| scope.ids.get(id).cloned());
        // Keep historical removal paths only when an immediate parent contains
        // a selected member covered by that exact path or its descendants.
        if let Some(deletions) = &mut commit.namespace_deletions {
            deletions.paths.retain(|deleted| {
                let key = name_policy::path_key(&deleted.path);
                original.parent_commit_ids.iter().any(|parent| {
                    previous.commits[parent].entries.iter().any(|entry| {
                        scope.contains(entry)
                            && (entry.path() == deleted.path
                                || namespace::is_descendant(entry.lookup_key(), &key))
                    })
                })
            });
            if deletions.paths.is_empty() {
                commit.namespace_deletions = None;
            }
        }
        next.commits.insert(commit.commit_id.clone(), commit);
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/business/history_copy_commits.rs"]
mod tests;
