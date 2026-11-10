//! Snapshot-local independent child boundaries shared by directory operations.
use super::*;

/// Resolve stable child identities in the supplied snapshot, never by their
/// later working paths. Borrowed normalized keys expire with that snapshot.
pub(super) struct ChildBoundaries<'a> {
    paths: BTreeSet<&'a str>,
}

impl<'a> ChildBoundaries<'a> {
    pub(super) fn new(entries: &'a [Entry], mut is_child: impl FnMut(&str) -> bool) -> Self {
        Self {
            paths: entries
                .iter()
                .filter(|entry| {
                    entry.kind == NamespaceEntryKind::Directory && is_child(&entry.entry_id)
                })
                .map(Entry::lookup_key)
                .collect(),
        }
    }

    pub(super) fn has_descendant(&self, key: &str) -> bool {
        let prefix = format!("{key}/");
        self.paths
            .range::<str, _>((
                std::ops::Bound::Included(prefix.as_str()),
                std::ops::Bound::Unbounded,
            ))
            .next()
            .is_some_and(|path| path.starts_with(&prefix))
    }

    pub(super) fn contains(&self, key: &str) -> bool {
        namespace::closest_directory_repository(key, |key| self.paths.get(key).copied()).is_some()
    }
}

/// Borrow all occurrence values. Only the bounded map nodes are allocated, not
/// namespace entries, path strings, file contexts, or nested immutable trees.
#[derive(PartialEq)]
pub(super) struct OwnedEntry<'a> {
    entry_id: &'a str,
    path: &'a str,
    kind: NamespaceEntryKind,
    file_id: &'a str,
    version_id: &'a str,
    size: u64,
    node: Option<&'a str>,
    branch: Option<&'a str>,
}

impl<'a> OwnedEntry<'a> {
    pub(super) fn new(entry: &'a Entry, depth: usize, context: Option<&'a FileContext>) -> Self {
        let Entry {
            entry_id,
            path,
            kind,
            file_id,
            version_id,
            size,
        } = entry;
        Self {
            entry_id,
            path: relative(path.as_str(), depth),
            kind: *kind,
            file_id,
            version_id,
            size: *size,
            node: context.and_then(|context| context.node_id.as_deref()),
            branch: context.map(|context| context.branch_id.as_str()),
        }
    }
}

pub(super) fn relative(path: &str, depth: usize) -> &str {
    path.splitn(depth + 1, '/')
        .nth(depth)
        .expect("invariant: directory entries are strict descendants of their root")
}
