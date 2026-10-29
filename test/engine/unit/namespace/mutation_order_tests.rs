//! Independent flat retain/push oracle from the pre-paged namespace contract.
//! Do not sort between operations: that would erase the behavior under test.
use super::*;
use storage_contract::{DeletedPath, NamespaceOperation, NamespaceOperationKind as Kind};

fn entry(path: &str, kind: NamespaceEntryKind) -> Entry {
    Entry {
        entry_id: format!("entry-{path}"),
        path: name_policy::PortablePath::parse(path).unwrap(),
        kind,
        file_id: if kind == NamespaceEntryKind::File {
            format!("file-{path}")
        } else {
            String::new()
        },
        version_id: if kind == NamespaceEntryKind::File {
            format!("version-{path}")
        } else {
            String::new()
        },
        size: 0,
    }
}

fn directory(path: &str) -> Entry {
    entry(path, NamespaceEntryKind::Directory)
}

fn file(path: &str) -> Entry {
    entry(path, NamespaceEntryKind::File)
}

fn operation(kind: Kind, path: &str, destination: Option<&str>) -> NamespaceOperation {
    NamespaceOperation::parse(kind, path, destination).unwrap()
}

fn descendant(key: &str, parent: &str) -> bool {
    key.strip_prefix(parent)
        .is_some_and(|suffix| suffix.starts_with('/'))
}

struct FlatOracle(Vec<Entry>);

impl FlatOracle {
    fn parents(&mut self, path: &name_policy::PortablePath) -> std::result::Result<(), ErrorCode> {
        let mut parents = Vec::new();
        let mut parent = path.parent();
        while let Some(path) = parent {
            parent = path.parent();
            parents.push(path);
        }
        for path in parents.into_iter().rev() {
            match self.0.iter().find(|entry| entry.lookup_key() == path.key()) {
                Some(entry) if entry.kind != NamespaceEntryKind::Directory => {
                    return Err(ErrorCode::ParentNotDirectory);
                }
                Some(_) => {}
                None => self.0.push(directory(path.as_str())),
            }
        }
        Ok(())
    }

    fn apply(
        &mut self,
        op: &NamespaceOperation,
    ) -> std::result::Result<Vec<DeletedPath>, ErrorCode> {
        let key = op.key();
        match op {
            NamespaceOperation::Mkdir { path } => {
                if self.0.iter().any(|entry| entry.lookup_key() == key) {
                    return Err(ErrorCode::NameCollision);
                }
                self.parents(path)?;
                self.0.push(directory(path.as_str()));
            }
            NamespaceOperation::Delete { .. } | NamespaceOperation::DeleteTree { .. } => {
                if !self.0.iter().any(|entry| entry.lookup_key() == key) {
                    return Err(ErrorCode::EntryNotFound);
                }
                if op.kind() == Kind::Delete
                    && self
                        .0
                        .iter()
                        .any(|entry| descendant(entry.lookup_key(), key))
                {
                    return Err(ErrorCode::DirectoryNotEmpty);
                }
                let selected = |entry: &Entry| {
                    entry.lookup_key() == key
                        || (op.kind() == Kind::DeleteTree && descendant(entry.lookup_key(), key))
                };
                let facts = self
                    .0
                    .iter()
                    .filter(|entry| selected(entry))
                    .map(|entry| DeletedPath {
                        path: entry.path().to_owned(),
                        kind: entry.kind,
                    })
                    .collect();
                self.0.retain(|entry| !selected(entry));
                return Ok(facts);
            }
            NamespaceOperation::Rename { destination, .. } => {
                let source = self
                    .0
                    .iter()
                    .find(|entry| entry.lookup_key() == key)
                    .ok_or(ErrorCode::EntryNotFound)?;
                let depth = source.path().split('/').count();
                if descendant(destination.key(), key) {
                    return Err(ErrorCode::InvalidMove);
                }
                if key != destination.key()
                    && self
                        .0
                        .iter()
                        .any(|entry| entry.lookup_key() == destination.key())
                {
                    return Err(ErrorCode::NameCollision);
                }
                self.parents(destination)?;
                for entry in &mut self.0 {
                    let path = if entry.lookup_key() == key {
                        destination.clone()
                    } else if descendant(entry.lookup_key(), key) {
                        entry.location().rebase(depth, destination).unwrap()
                    } else {
                        continue;
                    };
                    entry.relocate(path);
                }
            }
        }
        Ok(Vec::new())
    }

    fn replace(&mut self, entry: Entry) {
        self.0
            .retain(|previous| previous.lookup_key() != entry.lookup_key());
        self.0.push(entry);
    }
}

enum Edit {
    Operation(NamespaceOperation),
    File(Entry),
}

fn change(kind: Kind, path: &str, destination: Option<&str>) -> Edit {
    Edit::Operation(operation(kind, path, destination))
}

fn check_sequence(mut base: Vec<Entry>, edits: Vec<Edit>) -> Vec<DeletedPath> {
    base.sort_by(|left, right| left.lookup_key().cmp(right.lookup_key()));
    let mut oracle = FlatOracle(base.clone());
    let mut objects = NamespaceObjects::new();
    let namespace = build_from_entries(&base, &mut objects).unwrap();
    let absent = LoaderSource(|_: &NamespaceReference| Err(corrupt("test source object missing")));
    let source = DraftSource {
        fresh: &objects,
        base: &absent,
    };
    let mut editor = NamespaceEditor::open(&namespace, &source).unwrap();
    let mut facts = Vec::new();
    for (index, edit) in edits.into_iter().enumerate() {
        match edit {
            Edit::Operation(op) => {
                let expected = oracle.apply(&op);
                let actual = editor.apply(&op).map_err(|error| error.code());
                assert_eq!(actual, expected, "operation {index}: {op:?}");
                if let Ok(deleted) = actual {
                    facts.extend(deleted);
                }
            }
            Edit::File(entry) => {
                oracle.parents(entry.location()).unwrap();
                editor.ensure_parents(entry.location()).unwrap();
                editor.upsert_file(&entry).unwrap();
                oracle.replace(entry);
            }
        }
    }
    let mut fresh = NamespaceObjects::new();
    let sealed = editor.seal(&mut fresh).unwrap();
    let source = DraftSource {
        fresh: &fresh,
        base: &source,
    };
    let mut actual = NamespaceView::new(&sealed, &source)
        .collect_entries()
        .unwrap();
    actual.sort_by(|left, right| left.lookup_key().cmp(right.lookup_key()));
    oracle
        .0
        .sort_by(|left, right| left.lookup_key().cmp(right.lookup_key()));
    assert_eq!(actual.len(), oracle.0.len());
    for (actual, expected) in actual.iter().zip(&oracle.0) {
        assert_eq!(
            (
                actual.path(),
                actual.kind,
                &actual.file_id,
                &actual.version_id,
                actual.size
            ),
            (
                expected.path(),
                expected.kind,
                &expected.file_id,
                &expected.version_id,
                expected.size
            )
        );
    }
    facts
}

fn paths(facts: &[DeletedPath]) -> Vec<&str> {
    facts.iter().map(|fact| fact.path.as_str()).collect()
}

#[test]
fn deletion_order_preserves_slots_through_rename_and_tree_delete() {
    let facts = check_sequence(
        vec![directory("p"), file("p/a"), file("p/z")],
        vec![
            change(Kind::Rename, "p/z", Some("p/0")),
            change(Kind::DeleteTree, "p", None),
        ],
    );
    assert_eq!(paths(&facts), ["p", "p/a", "p/0"]);
}

#[test]
fn deletion_order_can_put_moved_children_before_new_ancestors() {
    let facts = check_sequence(
        vec![directory("a"), file("a/z")],
        vec![
            change(Kind::Rename, "a", Some("new/parent/moved")),
            change(Kind::DeleteTree, "new", None),
        ],
    );
    assert_eq!(
        paths(&facts),
        [
            "new/parent/moved",
            "new/parent/moved/z",
            "new",
            "new/parent"
        ]
    );
}

#[test]
fn deletion_order_preserves_appends_replacements_and_nested_moves() {
    let facts = check_sequence(
        vec![
            directory("p"),
            file("p/a"),
            file("p/z"),
            directory("q"),
            file("q/old"),
        ],
        vec![
            Edit::File(file("p/a")),
            Edit::File(file("p/new/deep")),
            change(Kind::Rename, "p/z", Some("p/0")),
            change(Kind::Rename, "p", Some("q/moved")),
            change(Kind::Rename, "q/moved/new", Some("q/first")),
            change(Kind::Rename, "q/moved", Some("q/last")),
            change(Kind::DeleteTree, "q", None),
        ],
    );
    assert_eq!(
        paths(&facts),
        [
            "q/last",
            "q/last/0",
            "q",
            "q/old",
            "q/last/a",
            "q/first",
            "q/first/deep"
        ]
    );
}

#[test]
fn deletion_order_for_reused_paths_keeps_each_intermediate_fact() {
    let facts = check_sequence(
        vec![
            directory("p"),
            file("p/a"),
            file("p/z"),
            directory("pish"),
            file("pish/keep"),
        ],
        vec![
            change(Kind::Rename, "p/z", Some("p/0")),
            change(Kind::Delete, "p/0", None),
            change(Kind::Rename, "p/a", Some("p/0")),
            change(Kind::DeleteTree, "p", None),
            change(Kind::Mkdir, "p/z", None),
            change(Kind::Mkdir, "p/a", None),
            change(Kind::Rename, "p/z", Some("p/0")),
            change(Kind::DeleteTree, "p", None),
        ],
    );
    assert_eq!(paths(&facts), ["p/0", "p", "p/0", "p", "p/0", "p/a"]);
}

#[test]
fn deletion_order_handles_case_folded_moves_and_operation_failures() {
    let facts = check_sequence(
        vec![directory("Straße"), file("Straße/a"), file("Straße/z")],
        vec![
            change(Kind::Rename, "STRASSE/z", Some("Straße/0")),
            change(Kind::Mkdir, "STRASSE", None),
            change(Kind::Delete, "STRASSE", None),
            change(Kind::Rename, "missing", Some("new/place")),
            change(Kind::Rename, "STRASSE", Some("Straße/inside")),
            change(Kind::Rename, "STRASSE/a", Some("Straße/0")),
            change(Kind::Rename, "STRASSE", Some("STRASSE")),
            change(Kind::DeleteTree, "Straße", None),
        ],
    );
    assert_eq!(paths(&facts), ["STRASSE", "STRASSE/a", "STRASSE/0"]);
}

#[test]
fn deletion_order_uses_original_full_keys_not_dfs_or_shared_file_identity() {
    let first = file("p/a/x");
    let mut alias = first.clone();
    alias.relocate(name_policy::PortablePath::parse("p/a-file").unwrap());
    let facts = check_sequence(
        vec![directory("p"), directory("p/a"), first, alias],
        vec![
            change(Kind::Rename, "p/a", Some("q/0")),
            change(Kind::Rename, "p/a-file", Some("q/z")),
            change(Kind::DeleteTree, "q", None),
        ],
    );
    assert_eq!(paths(&facts), ["q/0", "q/z", "q/0/x", "q"]);
}

#[test]
fn deletion_order_restarts_from_canonical_keys_after_sealing() {
    let mut objects = NamespaceObjects::new();
    let namespace =
        build_from_entries(&[directory("p"), file("p/a"), file("p/z")], &mut objects).unwrap();
    let absent = LoaderSource(|_: &NamespaceReference| Err(corrupt("test source object missing")));
    let source = DraftSource {
        fresh: &objects,
        base: &absent,
    };
    let mut editor = NamespaceEditor::open(&namespace, &source).unwrap();
    editor
        .apply(&operation(Kind::Rename, "p/z", Some("p/0")))
        .unwrap();
    let mut fresh = NamespaceObjects::new();
    let sealed = editor.seal(&mut fresh).unwrap();
    let source = DraftSource {
        fresh: &fresh,
        base: &source,
    };
    let mut next = NamespaceEditor::open(&sealed, &source).unwrap();
    let facts = next.apply(&operation(Kind::DeleteTree, "p", None)).unwrap();
    assert_eq!(paths(&facts), ["p", "p/0", "p/a"]);
}

#[test]
fn deletion_order_matches_the_flat_oracle_across_repeated_multioperation_batches() {
    for round in 0..12 {
        let mut edits = Vec::new();
        for step in 0..8 {
            let from = format!("p/{step}");
            let to = format!("p/renamed-{}", (step + round) % 8);
            edits.push(change(Kind::Rename, &from, Some(&to)));
        }
        edits.push(change(Kind::Rename, "p", Some("q/moved")));
        for step in (0..8).rev() {
            let path = format!("q/moved/renamed-{step}");
            if (round + step) % 3 == 0 {
                edits.push(change(Kind::Delete, &path, None));
                edits.push(Edit::File(file(&path)));
            } else {
                edits.push(change(
                    Kind::Rename,
                    &path,
                    Some(&format!("q/moved/new-{step}")),
                ));
            }
        }
        edits.push(change(Kind::DeleteTree, "q", None));
        let mut base = vec![directory("p"), directory("p-extra"), file("p-extra/keep")];
        base.extend((0..8).map(|n| file(&format!("p/{n}"))));
        check_sequence(base, edits);
    }
}
