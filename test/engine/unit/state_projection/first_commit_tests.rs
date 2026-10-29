//! Full-scan oracle for the derived version -> first-commit index.
use super::*;
use crate::namespace::fixture_store::FixtureSource;
use json_codec::json;
use storage_contract::{Commit, Entry, NamespaceEntryKind};

fn oracle(state: &State) -> BTreeMap<String, FirstCommit> {
    let mut best = BTreeMap::<String, FirstCommit>::new();
    for commit in state.commits.values() {
        for entry in crate::namespace::NamespaceView::new(&commit.namespace, &FixtureSource)
            .collect_entries()
            .unwrap()
        {
            if entry.kind != NamespaceEntryKind::File {
                continue;
            }
            let candidate = FirstCommit {
                sequence: commit.sequence,
                commit_id: commit.commit_id.clone(),
            };
            let replace = best
                .get(&entry.version_id)
                .is_none_or(|current| candidate.precedes(current));
            if replace {
                best.insert(entry.version_id.clone(), candidate);
            }
        }
    }
    best
}

fn indexed(index: &DerivedOwners) -> BTreeMap<String, FirstCommit> {
    index
        .first_commits
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn assert_first_commits(state: &State, index: &DerivedOwners) {
    assert_eq!(indexed(index), oracle(state));
    for (version, expected) in oracle(state) {
        assert_eq!(index.first_commit(&version), Some(&expected));
    }
    assert_eq!(index.first_commit("absent-version"), None);
}

fn entry(version: &str, kind: NamespaceEntryKind) -> Entry {
    Entry {
        entry_id: format!("entry-{version}"),
        path: name_policy::PortablePath::parse(&format!("{version}.txt")).unwrap(),
        kind,
        file_id: "file".into(),
        version_id: version.into(),
        size: 1,
    }
}

fn commit(id: &str, sequence: u64, versions: &[&str]) -> Commit {
    Commit {
        commit_id: id.into(),
        history_tree_id: "h1".into(),
        kind: "save".into(),
        parent_commit_ids: Vec::new(),
        tree_id: format!("tree-{id}"),
        sequence,
        operation_id: "op_00000000000000000000000000000000".parse().unwrap(),
        author: "author".into(),
        message: format!("message {id}"),
        namespace: crate::namespace::fixture_store::namespace(
            &versions
                .iter()
                .map(|version| entry(version, NamespaceEntryKind::File))
                .collect::<Vec<_>>(),
        ),
        namespace_deletions: None,
        pending_file_versions: BTreeMap::new(),
        provenance: None,
    }
}

fn fixture() -> State {
    let mut state = State::default();
    state.histories.insert(
        "h1".into(),
        json_codec::from_value(json!({
            "history_tree_id":"h1", "repository_id":"r1", "root_commit_id":"c1",
            "main_branch_id":null, "generation":"1", "state":"active", "provenance":null
        }))
        .unwrap(),
    );
    // Same sequence on c2/c2b: the smaller commit id wins deterministically.
    for commit in [
        commit("c1", 1, &["v1"]),
        commit("c2", 2, &["v1", "v2"]),
        commit("c2b", 2, &["v2", "v3"]),
        commit("c3", 3, &["v1", "v2", "v3", "v4"]),
        commit("c9", u64::MAX, &["v9"]),
    ] {
        state.commits.insert(commit.commit_id.clone(), commit);
    }
    state
}

fn candidate(before: &State) -> State {
    let mut after = before.clone();
    after.begin_tracking();
    after
}

fn step(before: &State, after: &State, index: &DerivedOwners) -> DerivedOwners {
    let transition = ProjectionTransition::new(before, after, Some(index), &FixtureSource).unwrap();
    assert_first_commits(before, transition.previous().owners);
    assert_first_commits(after, transition.next().owners);
    transition.next().owners.clone()
}

#[test]
fn first_commit_index_matches_the_full_scan_after_appends_removals_and_rewrites() {
    let before = fixture();
    let index = DerivedOwners::from_state(&before, &FixtureSource).unwrap();
    assert_first_commits(&before, &index);
    assert_eq!(
        index.first_commit("v2"),
        Some(&FirstCommit {
            sequence: 2,
            commit_id: "c2".into()
        })
    );

    // Append-only publication: a later commit never changes an earlier choice,
    // an earlier commit lowers it.
    let mut appended = candidate(&before);
    appended
        .commits
        .insert("c4".into(), commit("c4", 4, &["v4", "v5"]));
    appended
        .commits
        .insert("c0".into(), commit("c0", 0, &["v3"]));
    let index = step(&before, &appended, &index);

    // Removing the current first commit of several versions (split retire)
    // falls back to the next earliest remaining commit or drops the entry.
    let mut removed = candidate(&appended);
    removed.commits.remove_discard("c0");
    removed.commits.remove_discard("c1");
    removed.commits.remove_discard("c9");
    let index = step(&appended, &removed, &index);
    assert_eq!(index.first_commit("v9"), None);
    assert_eq!(
        index.first_commit("v3"),
        Some(&FirstCommit {
            sequence: 2,
            commit_id: "c2b".into()
        })
    );

    // Rewriting a commit in place (different entries and sequence) is treated
    // as removal plus insertion.
    let mut rewritten = candidate(&removed);
    rewritten
        .commits
        .insert("c2".into(), commit("c2", 7, &["v7"]));
    let index = step(&removed, &rewritten, &index);
    assert_eq!(
        index.first_commit("v1"),
        Some(&FirstCommit {
            sequence: 3,
            commit_id: "c3".into()
        })
    );

    // Same-transition swap: the removed first commit and a new earlier commit
    // for the same version, in both key orders.
    for (removed_id, added_id) in [("c3", "a0"), ("a0", "c3")] {
        let mut base = candidate(&rewritten);
        if removed_id == "a0" {
            base.commits.insert("a0".into(), commit("a0", 1, &["v1"]));
        }
        let base_index = step(&rewritten, &base, &index);
        let mut swapped = candidate(&base);
        swapped.commits.remove_discard(removed_id);
        swapped
            .commits
            .insert(added_id.into(), commit(added_id, 1, &["v1"]));
        step(&base, &swapped, &base_index);
    }

    // An unchanged reinsert and a cold rebuild without a baseline both agree.
    let mut equal = candidate(&rewritten);
    equal
        .commits
        .insert("c3".into(), before.commits["c3"].clone());
    equal
        .commits
        .insert("c3".into(), rewritten.commits["c3"].clone());
    step(&rewritten, &equal, &index);
    let cold = ProjectionTransition::new(&before, &rewritten, None, &FixtureSource).unwrap();
    assert_first_commits(&rewritten, cold.next().owners);
}

#[test]
fn append_first_publishers_prune_shared_pages_before_io_and_rebuild_removed_witnesses() {
    use crate::namespace::{NamespaceEditor, NamespaceNode, NamespaceObjects, NamespaceSource};
    use std::cell::Cell;
    struct Reads<'a> {
        source: &'a dyn NamespaceSource,
        calls: Cell<usize>,
    }
    impl NamespaceSource for Reads<'_> {
        fn node(
            &self,
            reference: &storage_contract::NamespaceReference,
        ) -> crate::Result<NamespaceNode> {
            self.calls.set(self.calls.get() + 1);
            self.source.node(reference)
        }
    }
    for width in [256, 4096] {
        let entries = (0..width)
            .map(|n| entry(&format!("v{n:05}"), NamespaceEntryKind::File))
            .collect::<Vec<_>>();
        let mut original = commit("initial", 1, &[]);
        original.namespace = crate::namespace::fixture_store::namespace(&entries);
        let mut before = State::default();
        before
            .commits
            .insert(original.commit_id.clone(), original.clone());
        let mut index = DerivedOwners::default();
        index
            .commit_first_versions(&original, &FixtureSource)
            .unwrap();
        let mut editor = NamespaceEditor::open(&original.namespace, &FixtureSource).unwrap();
        let mut changed = entries[width / 2].clone();
        changed.version_id = "new-version".into();
        editor.upsert_file(&changed).unwrap();
        let mut objects = NamespaceObjects::new();
        let mut next = commit("append", 2, &[]);
        next.namespace = editor.seal(&mut objects).unwrap();
        crate::namespace::fixture_store::register(objects);
        let source = Reads {
            source: &FixtureSource,
            calls: Cell::new(0),
        };
        index.commit_first_versions(&next, &source).unwrap();
        assert!(
            source.calls.get() <= 8,
            "width {width}: {} opened objects",
            source.calls.get()
        );
        assert_eq!(
            index.first_commit("new-version").unwrap().commit_id,
            "append"
        );
        assert_eq!(
            index
                .first_commit(&entries[0].version_id)
                .unwrap()
                .commit_id,
            "initial"
        );
        let mut after = before.clone();
        after.commits.insert(next.commit_id.clone(), next);
        assert_first_commits(&after, &index);
        after.commits.remove_discard("initial");
        index
            .rebuild_first_versions(&after, &FixtureSource)
            .unwrap();
        assert_first_commits(&after, &index);
    }
}
