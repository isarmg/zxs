use super::{HISTORY_LIMIT, history_sequence_may_fit, record_event, utc_event_timestamp};
use json_codec::{Value, json};
use std::{cmp::Reverse, collections::BTreeMap};

fn project_history_sources(
    sources: &[(u64, Vec<String>)],
    skip_older: bool,
) -> (BTreeMap<(Reverse<u64>, String), Value>, usize) {
    let mut events = BTreeMap::new();
    let mut projected = 0;
    for (source, (sequence, ids)) in sources.iter().enumerate() {
        if skip_older && !history_sequence_may_fit(&events, *sequence) {
            continue;
        }
        for id in ids {
            projected += 1;
            record_event(&mut events, *sequence, id.clone(), json!({"source":source}));
        }
    }
    (events, projected)
}

#[test]
fn history_sequence_cutoff_waits_for_distinct_rows_and_keeps_ties() {
    let mut events = BTreeMap::new();
    assert!(history_sequence_may_fit(&events, 0));
    for _ in 0..250 {
        record_event(&mut events, 5, "duplicate".into(), json!({}));
    }
    assert_eq!(events.len(), 1);
    for id in 0..198 {
        record_event(&mut events, 10, id.to_string(), json!({}));
    }
    assert_eq!(events.len(), HISTORY_LIMIT - 1);
    assert!(history_sequence_may_fit(&events, 0));
    record_event(&mut events, 10, "last".into(), json!({}));
    assert_eq!(events.len(), HISTORY_LIMIT);
    assert!(!history_sequence_may_fit(&events, 4));
    assert!(history_sequence_may_fit(&events, 5));
    assert!(history_sequence_may_fit(&events, u64::MAX));
    record_event(&mut events, u64::MAX, "newest".into(), json!({}));
    assert!(!history_sequence_may_fit(&events, 5));
    assert!(history_sequence_may_fit(&events, 10));
}

#[test]
fn history_sequence_cutoff_matches_full_projection_for_shuffled_sources() {
    // Empty recent sources and repeated keys must never count as a full page.
    let mut sources = (0..250).map(|_| (900, Vec::new())).collect::<Vec<_>>();
    sources.push((1, vec!["duplicate".into(); 250]));
    // More than one page at the same sequence, split across multiple sources.
    // IDs intentionally do not sort in numeric order.
    for group in 0..20 {
        sources.push((
            500,
            (group * 20..(group + 1) * 20)
                .rev()
                .map(|id| format!("opaque-{id}"))
                .collect(),
        ));
    }
    sources.extend(
        (0..300).map(|sequence| (sequence, vec![format!("older-{sequence}"), "shared".into()])),
    );
    sources.push((500, vec!["opaque-0".into(), "opaque-10".into()]));
    sources.push((u64::MAX, vec!["maximum".into()]));
    sources.push((u64::MAX, vec!["maximum".into(), "another-maximum".into()]));

    for seed in 0_u64..16 {
        let mut shuffled = sources.clone();
        if seed != 0 {
            let mut random = seed;
            for index in (1..shuffled.len()).rev() {
                random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
                shuffled.swap(index, (random % (index as u64 + 1)) as usize);
            }
        }
        let (expected, _) = project_history_sources(&shuffled, false);
        let (actual, _) = project_history_sources(&shuffled, true);
        assert_eq!(actual, expected, "shuffle seed {seed}");
        assert_eq!(actual.len(), HISTORY_LIMIT);
        assert_eq!(actual.last_key_value().unwrap().0.0, Reverse(500));
    }
}

#[test]
fn history_sequence_cutoff_skips_older_projection_without_stopping_the_scan() {
    let mut sources = vec![(
        1000,
        (0..HISTORY_LIMIT)
            .map(|id| format!("recent-{id}"))
            .collect(),
    )];
    sources.extend((0..1000).map(|sequence| {
        (
            sequence,
            (0..10).map(|id| format!("older-{sequence}-{id}")).collect(),
        )
    }));
    sources.push((2000, vec!["newest-after-older-sources".into()]));
    let (expected, full_projection) = project_history_sources(&sources, false);
    let (actual, limited_projection) = project_history_sources(&sources, true);
    assert_eq!(actual, expected);
    assert_eq!(full_projection, HISTORY_LIMIT + 10_000 + 1);
    assert_eq!(limited_projection, HISTORY_LIMIT + 1);
    assert_eq!(actual.first_key_value().unwrap().0.0, Reverse(2000));
}

#[test]
fn utc_history_projection_formats_seconds_and_preserves_missing_times() {
    for (seconds, expected) in [
        (0, "1970-01-01T00:00:00Z"),
        (1_792_803_600, "2026-10-24T01:00:00Z"),
    ] {
        let mut event = json!({"timestamp":seconds.to_string(),"event_id":"original"});
        utc_event_timestamp(&mut event).unwrap();
        assert_eq!(event, json!({"timestamp":expected,"event_id":"original"}));
    }
    for mut event in [json!({"timestamp":null}), json!({"event_id":"missing"})] {
        let before = event.clone();
        utc_event_timestamp(&mut event).unwrap();
        assert_eq!(event, before);
    }
}

#[test]
fn restored_deletion_activity_uses_committed_time_without_receipts_or_content() {
    use storage_contract::{CommitRequest, Config, NamespaceOperation, new_id};
    use storage_engine::Engine;

    let root = crate::tests::private_tempdir();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        Config::development([root.path().join("disk")]),
    )
    .unwrap();
    let repo = engine
        .create_repository("Audit", "P01", 1, "owner", &new_id("op"))
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "owner")
        .unwrap();
    for kind in ["mkdir", "delete"] {
        engine
            .namespace_operation(
                &workspace.workspace_id,
                NamespaceOperation::parse(kind.parse().unwrap(), "transient", None).unwrap(),
            )
            .unwrap();
    }
    let operation_id = new_id("op");
    let published = engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: operation_id.clone(),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "record transient removal".into(),
            request_digest: None,
        })
        .unwrap();
    let commit_id = published.commit_id.unwrap();
    let timestamp = engine.state().commits[&commit_id]
        .namespace_deletions
        .as_ref()
        .unwrap()
        .timestamp;
    let deletion_rows = |engine: &Engine| {
        super::read(engine).unwrap()["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["action"] == "delete")
            .cloned()
            .collect::<Vec<_>>()
    };
    let before = deletion_rows(&engine);
    assert_eq!(before.len(), 1);
    assert_eq!(before[0]["path"], "Audit/transient");
    assert_eq!(
        before[0]["timestamp"],
        super::utc_timestamp(timestamp).unwrap()
    );
    assert_eq!(before[0]["file_id"], "");
    assert!(before[0]["version_id"].is_null());

    let package = root.path().join("package");
    engine
        .export_history_package(&repo.history_tree_id, &package)
        .unwrap();
    let restored_root = crate::tests::private_tempdir();
    let mut restored = Engine::initialize(
        &restored_root.path().join("system"),
        Config::development([restored_root.path().join("disk")]),
    )
    .unwrap();
    restored
        .restore_history_package(&package, "P01", 1, "owner", &new_id("op"))
        .unwrap();
    assert!(!restored.state().operations.contains_key(&operation_id));
    assert!(restored.state().versions.is_empty());
    assert_eq!(deletion_rows(&restored), before);
}
