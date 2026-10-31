use super::*;
use crate::{Commit, FileVersion};
use std::cell::Cell;

use super::super::oracle_tests::{context, fixture, oracle_path, request};

const FILE: &str = "file_00000000000000000000000000000001";
const HISTORY: &str = "history_00000000000000000000000000000001";

fn first_commit_oracle<'a>(state: &'a State, version: &FileVersion) -> Option<&'a Commit> {
    state
        .commits
        .values()
        .filter(|commit| {
            commit.history_tree_id == version.history_tree_id
                && commit.entries.iter().any(|entry| {
                    entry.file_id == version.file_id && entry.version_id == version.version_id
                })
        })
        .min_by_key(|commit| (commit.sequence, &commit.commit_id))
}

fn projection_fixture() -> MetadataView {
    let mut view = fixture();
    let state = Arc::make_mut(&mut view.state.0);
    let mut tied = state.commits["commit_00000000000000000000000000000002"].clone();
    tied.commit_id = "commit_000000000000000000000000000003e8".into();
    tied.author = "a later equal-sequence commit must not win".into();
    tied.message = "ignored".into();
    tied.entries[0].relocate(name_policy::PortablePath::parse("ignored.txt").unwrap());
    state.commits.insert(tied.commit_id.clone(), tied);
    let mut lower = state.commits["commit_00000000000000000000000000000004"].clone();
    lower.commit_id = "commit_000000000000000000000000000003e9".into();
    lower.sequence = 0;
    lower.author = "作者🦀".repeat(15);
    lower.message = "更早🦀".repeat(19);
    lower.entries[0].relocate(name_policy::PortablePath::parse("更早.txt").unwrap());
    state.commits.insert(lower.commit_id.clone(), lower);
    let mut pending = state.versions["ver_00000000000000000000000000000000"].clone();
    pending.version_id = "ver_000000000000000000000000000007d0".into();
    pending.creation_name = "待提交.txt".into();
    state.versions.insert(pending.version_id.clone(), pending);
    view.history_window_roots = Some(HistoryRootBindings::recovered(
        &view.state.0,
        BTreeMap::from([(
            HISTORY.into(),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        )]),
    ));
    view
}

#[test]
fn resolved_content_summary_facts_match_complete_commit_and_path_oracles() {
    let view = projection_fixture();
    let state = view.business_projection();
    let context = context();
    let request = request(HistoryCollection::Nodes, 64);
    let cancel = HistoryWindowCancellation::new();
    let query = Query::new(&view, &request, &context, &cancel).unwrap();
    let mut pending_seen = false;
    for candidate in nodes::all_nodes(state, query.scope(), &cancel) {
        let node = candidate.unwrap();
        let version = node.version(state);
        let first = match node {
            Node::Content(version) => first_commit_oracle(state, version),
            Node::Path(_) | Node::Metadata(_) => None,
        };
        let summary = query
            .node_summary(node, first, HistoryParentSet::Complete)
            .unwrap();
        assert_eq!(summary.path, oracle_path(state, node.id()));
        assert_eq!(
            summary.fields.message.byte_length,
            query.field(node, HistoryField::Message).unwrap().len() as u64
        );
        assert_eq!(
            summary.fields.author.byte_length,
            query.field(node, HistoryField::Author).unwrap().len() as u64
        );
        assert_eq!(
            summary.fields.created_at.byte_length,
            query.field(node, HistoryField::CreatedAt).unwrap().len() as u64
        );
        if let Node::Content(_) = node {
            assert_eq!(
                summary.sequence,
                first.map_or(version.version_sequence, |c| c.sequence)
            );
            let fields = content_fields(version, first, &cancel).unwrap();
            assert_eq!(fields, summary.fields);
            let body_summary = query.version_summary(version).unwrap();
            assert_eq!(body_summary.fields, fields);
            if first.is_none() {
                pending_seen = true;
                assert_eq!(summary.path, "待提交.txt");
                assert_eq!(summary.fields.message.byte_length, 0);
                assert_eq!(summary.fields.author.byte_length, 0);
            }
        }
    }
    assert!(pending_seen);
}

#[test]
fn owner_prefix_visits_only_through_the_exact_minimum_file_version() {
    let view = fixture();
    let state = view.business_projection();
    assert!(
        state
            .versions
            .iter()
            .all(|(id, version)| id == &version.version_id)
    );
    for (file, expected_visits) in [
        (FILE, 1),
        ("file_00000000000000000000000000000002", 18),
        ("file_ffffffffffffffffffffffffffffffff", 19),
    ] {
        let baseline = Cell::new(0);
        let expected = state
            .versions
            .values()
            .inspect(|_| baseline.set(baseline.get() + 1))
            .filter(|version| version.file_id == file)
            .min_by_key(|version| &version.version_id);
        assert_eq!(baseline.get(), 19);
        let visits = Cell::new(0);
        let actual = nodes::owner_version(
            state
                .versions
                .values()
                .inspect(|_| visits.set(visits.get() + 1)),
            file,
            &HistoryWindowCancellation::new(),
        )
        .unwrap();
        assert_eq!(
            actual.map(|v| &v.version_id),
            expected.map(|v| &v.version_id)
        );
        assert_eq!(visits.get(), expected_visits);
    }
    // Exercise first, middle and last owners without adding any query index.
    let mut versions = [0, 8, 16].map(|n| state.versions[format!("ver_{n:032x}").as_str()].clone());
    for (n, version) in versions.iter_mut().enumerate() {
        version.file_id = format!("file_{n:032x}");
    }
    for (rank, version) in versions.iter().enumerate() {
        let visits = Cell::new(0);
        let actual = nodes::owner_version(
            versions.iter().inspect(|_| visits.set(visits.get() + 1)),
            &version.file_id,
            &HistoryWindowCancellation::new(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(actual.version_id, version.version_id);
        assert_eq!(visits.get(), rank + 1);
    }
}

#[test]
fn borrowed_owner_and_resolved_content_facts_observe_cancellation() {
    let view = fixture();
    for cancel_at in [1, 3] {
        let cancel = HistoryWindowCancellation::new();
        let visits = Cell::new(0);
        let error = nodes::owner_version(
            view.business_projection().versions.values().inspect(|_| {
                visits.set(visits.get() + 1);
                if visits.get() == cancel_at {
                    cancel.cancel();
                }
            }),
            "file_00000000000000000000000000000002",
            &cancel,
        )
        .unwrap_err();
        assert_eq!(error.code(), storage_contract::ErrorCode::OperationAborted);
        assert_eq!(visits.get(), cancel_at);
    }
    let cancel = HistoryWindowCancellation::new();
    cancel.cancel();
    let version = &view.business_projection().versions["ver_00000000000000000000000000000000"];
    let commit = first_commit_oracle(view.business_projection(), version);
    let scope = nodes::Scope {
        file: FILE,
        history: HISTORY,
    };
    assert_eq!(
        nodes::content_path(scope, version, commit, &cancel)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::OperationAborted
    );
    assert_eq!(
        content_fields(version, commit, &cancel).unwrap_err().code(),
        storage_contract::ErrorCode::OperationAborted
    );
}
