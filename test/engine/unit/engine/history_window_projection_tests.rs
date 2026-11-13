use super::*;
use crate::{Commit, FileVersion};
use std::cell::Cell;

use super::super::oracle_tests::{context, fixture, oracle_path, request};

const FILE: &str = "file_00000000000000000000000000000001";
const HISTORY: &str = "history_00000000000000000000000000000001";

#[test]
fn historical_node_and_body_names_use_current_container_name_without_rewriting_evidence() {
    let mut view = fixture();
    let state = Arc::make_mut(&mut view.state.0);
    state.file_locations.insert(FILE.into(), "现在/最新.txt".into());
    state.file_names.insert(FILE.into(), "最新.txt".into());
    view.history_window_roots = Some(HistoryRootBindings::recovered(
        &view.state.0,
        BTreeMap::from([(
            HISTORY.into(),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        )]),
    ));
    let before = storage_contract::encoding::canonical_json(view.business_projection()).unwrap();
    let request = request(HistoryCollection::Nodes, 64);
    let cancel = HistoryWindowCancellation::new();
    for retained in [false, true] {
        let mut context = context();
        if retained {
            context.path = None;
        }
        let query = Query::new(&view, &request, &context, &cancel).unwrap();
        for id in [
            "ver_00000000000000000000000000000000",
            "ver_00000000000000000000000000000001",
        ] {
            let version = &query.state.versions[id];
            let body = query.version_summary(version).unwrap();
            let node = query
                .node_summary(
                    Node::Content(version),
                    first_commit_oracle(query.state, version),
                    HistoryParentSet::Complete,
                )
                .unwrap();
            assert_eq!(body.download_name, "最新.txt");
            assert_eq!(node.download_name, "最新.txt");
            assert_eq!(body.storage_name, format!("{}-最新.txt", version.storage_id));
            assert_eq!(node.storage_name, body.storage_name);
            assert_eq!(node.path, oracle_path(query.state, id));
            assert_eq!(version.creation_name, "原稿.txt");
        }
    }
    assert_eq!(
        storage_contract::encoding::canonical_json(view.business_projection()).unwrap(),
        before,
    );
}

#[test]
fn creation_time_fields_preserve_utc_text_range_errors_and_state_bytes() {
    use std::error::Error as _;
    use storage_contract::{ErrorCode, encoding::canonical_json};

    const PATH: &str = "fnode_00000000000000000000000000000000";
    const METADATA: &str = "fmeta_00000000000000000000000000000000";
    for (timestamp, expected) in [
        (0, Some("1970-01-01T00:00:00Z")),
        (1_709_251_199, Some("2024-02-29T23:59:59Z")),
        (253_402_300_799, Some("9999-12-31T23:59:59Z")),
        (253_402_300_800, None),
        (i64::MAX as u64, None),
        (u64::MAX, None),
    ] {
        let mut view = fixture();
        let state = Arc::make_mut(&mut view.state.0);
        state.file_path_nodes.get_mut(PATH).unwrap().created_at = timestamp;
        state.file_metadata_nodes.get_mut(METADATA).unwrap().created_at = timestamp;
        // Arc::make_mut dissociates the fixture's weak root binding.
        view.history_window_roots = Some(HistoryRootBindings::recovered(
            &view.state.0,
            BTreeMap::from([(
                HISTORY.into(),
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            )]),
        ));
        let before = canonical_json(view.business_projection()).unwrap();
        let context = context();
        let request = request(HistoryCollection::Nodes, 64);
        let cancel = HistoryWindowCancellation::new();
        let query = Query::new(&view, &request, &context, &cancel).unwrap();
        for node in [
            Node::Path(&query.state.file_path_nodes[PATH]),
            Node::Metadata(&query.state.file_metadata_nodes[METADATA]),
        ] {
            let actual = query.field(node, HistoryField::CreatedAt);
            if let Some(expected) = expected {
                assert_eq!(actual.unwrap(), expected);
            } else {
                let error = actual.unwrap_err();
                assert!(!error.is_retryable());
                assert!(error.published_result().is_none());
                if timestamp == u64::MAX {
                    assert_eq!(error.code(), ErrorCode::StateInvalid);
                    assert!(
                        error
                            .source()
                            .unwrap()
                            .source()
                            .unwrap()
                            .is::<std::num::TryFromIntError>()
                    );
                } else {
                    assert_eq!(error.code(), ErrorCode::HistoryPackageInvalid);
                    assert!(
                        error
                            .source()
                            .unwrap()
                            .is::<date_time::error::ComponentRange>()
                    );
                }
            }
        }
        assert_eq!(canonical_json(view.business_projection()).unwrap(), before);
    }
}

#[test]
fn creation_time_field_cancellation_precedes_calendar_conversion() {
    use storage_contract::{ErrorCode, encoding::canonical_json};

    const PATH: &str = "fnode_00000000000000000000000000000000";
    let mut view = fixture();
    Arc::make_mut(&mut view.state.0)
        .file_path_nodes
        .get_mut(PATH)
        .unwrap()
        .created_at = 253_402_300_800;
    // Arc::make_mut dissociates the fixture's weak root binding.
    view.history_window_roots = Some(HistoryRootBindings::recovered(
        &view.state.0,
        BTreeMap::from([(
            HISTORY.into(),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        )]),
    ));
    let before = canonical_json(view.business_projection()).unwrap();
    let context = context();
    let request = request(HistoryCollection::Nodes, 64);
    let cancel = HistoryWindowCancellation::new();
    let query = Query::new(&view, &request, &context, &cancel).unwrap();
    cancel.cancel();
    let error = query
        .field(
            Node::Path(&query.state.file_path_nodes[PATH]),
            HistoryField::CreatedAt,
        )
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::OperationAborted);
    assert_eq!(canonical_json(view.business_projection()).unwrap(), before);
}

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
