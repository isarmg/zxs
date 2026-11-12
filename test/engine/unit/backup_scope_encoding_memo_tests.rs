//! Byte memo contracts only; real admission/readback remains in its callers.
use super::*;
use crate::{DiskRegistrationIdentity, DiskRegistryCommitment, ErrorCode};

fn memo_with_limit(bytes: usize, entries: usize) -> ScopeEncodingMemo {
    ScopeEncodingMemo {
        state: Mutex::new(State::default()),
        maximum_bytes: bytes,
        maximum_entries: entries,
    }
}

#[test]
fn committed_exact_input_reuses_owned_bytes_without_reexecuting_encoder() {
    let memo = ScopeEncodingMemo::new();
    let mut calls = 0;
    let mut first = Session::new(Some((&memo, [1; 32])));
    assert_eq!(first.system([2; 32], || { calls += 1; Ok(vec![3, 4]) }).unwrap(), [3, 4]);
    assert!(memo.state.lock().unwrap().entries.is_empty());
    first.commit();
    let mut next = Session::new(Some((&memo, [1; 32])));
    let mut bytes = next.system([2; 32], || { calls += 1; Ok(vec![9]) }).unwrap();
    bytes[0] = 8;
    assert_eq!(next.system([2; 32], || { calls += 1; Ok(vec![9]) }).unwrap(), [3, 4]);
    assert_eq!(calls, 1);
}

#[test]
fn failed_or_uncommitted_capture_never_adds_material_and_releases_reservations() {
    let memo = ScopeEncodingMemo::new();
    {
        let mut failed = Session::new(Some((&memo, [1; 32])));
        failed.system([2; 32], || Ok(vec![3])).unwrap();
        let stop = AtomicBool::new(true);
        let error = failed.system([4; 32], || {
            preparation_checkpoint(&stop)?;
            Ok(vec![5])
        }).unwrap_err();
        assert_eq!(error.code(), ErrorCode::OperationAborted);
        // The caller returns its readback/binding/error instead of committing.
    }
    let state = memo.state.lock().unwrap();
    assert!(state.entries.is_empty());
    assert_eq!(state.bytes, 0);
    assert_eq!(state.reserved_bytes, 0);
    assert_eq!(state.reserved_entries, 0);
}

#[test]
fn pending_capture_is_not_visible_to_a_different_capture() {
    let memo = ScopeEncodingMemo::new();
    let mut first = Session::new(Some((&memo, [1; 32])));
    first.system([2; 32], || Ok(vec![3])).unwrap();
    let mut second = Session::new(Some((&memo, [1; 32])));
    let mut called = false;
    assert_eq!(second.system([2; 32], || { called = true; Ok(vec![3]) }).unwrap(), [3]);
    assert!(called);
    first.commit();
    second.commit();
    let state = memo.state.lock().unwrap();
    assert_eq!(state.entries.len(), 1);
    assert_eq!(state.reserved_bytes, 0);
    assert_eq!(state.reserved_entries, 0);
}

#[test]
fn cache_full_and_pending_reservations_fall_back_without_changing_results() {
    let entry_bytes = std::mem::size_of::<(Key, Bytes)>() + 1;
    let memo = memo_with_limit(entry_bytes, 1);
    let mut first = Session::new(Some((&memo, [1; 32])));
    first.system([2; 32], || Ok(vec![3])).unwrap();
    let mut other = Session::new(Some((&memo, [1; 32])));
    assert_eq!(other.system([4; 32], || Ok(vec![5])).unwrap(), [5]);
    other.commit();
    assert!(memo.state.lock().unwrap().entries.is_empty());
    drop(first);
    let mut accepted = Session::new(Some((&memo, [1; 32])));
    accepted.system([6; 32], || Ok(vec![7])).unwrap();
    accepted.commit();
    let mut full = Session::new(Some((&memo, [1; 32])));
    let mut calls = 0;
    for _ in 0..2 {
        assert_eq!(full.system([8; 32], || { calls += 1; Ok(vec![9]) }).unwrap(), [9]);
    }
    assert_eq!(calls, 2);
    let state = memo.state.lock().unwrap();
    assert_eq!(state.entries.len(), 1);
    assert_eq!(state.bytes, entry_bytes);
}

#[test]
fn disabled_reader_encodes_every_time_and_does_not_create_a_cache() {
    let mut session = Session::new(None);
    let mut calls = 0;
    for _ in 0..2 {
        assert_eq!(session.system([2; 32], || { calls += 1; Ok(vec![3]) }).unwrap(), [3]);
    }
    assert!(!session.enabled());
    assert_eq!(calls, 2);
    session.commit();
}

#[test]
fn target_context_input_and_encoder_kind_all_separate_entries() {
    let memo = ScopeEncodingMemo::new();
    let mut first = Session::new(Some((&memo, [1; 32])));
    first.system([2; 32], || Ok(vec![3])).unwrap();
    first.commit();
    for (context, input) in [([4; 32], [2; 32]), ([1; 32], [5; 32])] {
        let mut session = Session::new(Some((&memo, context)));
        assert_eq!(session.system(input, || Ok(vec![6])).unwrap(), [6]);
        session.commit();
    }
    let mut pair = Session::new(Some((&memo, [1; 32])));
    assert_eq!(pair.disk_pair(Some([2; 32]), || Ok((vec![7], vec![8]))).unwrap(), (vec![7], vec![8]));
    pair.commit();
    let mut next = Session::new(Some((&memo, [1; 32])));
    assert_eq!(next.disk_pair(Some([2; 32]), || panic!("exact pair should hit")).unwrap(), (vec![7], vec![8]));
    assert_eq!(memo.state.lock().unwrap().entries.len(), 4);
}

fn identity() -> DiskRegistrationIdentity {
    DiskRegistrationIdentity {
        pool_id: format!("pool_{}", "1".repeat(32)),
        disk_id: format!("disk_{}", "2".repeat(32)),
        slot_id: "C02".into(),
        binding_generation: 7,
    }
}

fn registry(root: char, generation: u64) -> DiskRegistryCommitment {
    json_codec::from_str(&format!(
        "{{\"root_hash\":\"{}\",\"view_generation\":\"{generation}\"}}",
        root.to_string().repeat(64)
    )).unwrap()
}

fn context<'a>(
    target: &'a DiskRegistrationIdentity,
    registry: &'a DiskRegistryCommitment,
) -> Context<'a> {
    Context {
        target,
        device: 11,
        inode: 13,
        operation: "op_00000000000000000000000000000001".parse().unwrap(),
        source_fence: "source-fence",
        archive: "archive",
        candidate: "candidate",
        decision: "decision",
        publication: "publication",
        authority: "authority",
        snapshot: "snapshot",
        first_sequence: 17,
        registry,
        configuration: "configuration",
    }
}

#[test]
fn every_context_identity_binding_and_authenticated_boundary_is_in_the_key() {
    let stop = AtomicBool::new(false);
    let target = identity();
    let original_registry = registry('a', 1);
    let original = context(&target, &original_registry).digest(&stop).unwrap();
    macro_rules! changed {
        ($field:ident, $value:expr) => {{
            let mut changed = context(&target, &original_registry);
            changed.$field = $value;
            assert_ne!(changed.digest(&stop).unwrap(), original, stringify!($field));
        }};
    }
    changed!(device, 19);
    changed!(inode, 23);
    changed!(operation, "op_00000000000000000000000000000002".parse().unwrap());
    changed!(source_fence, "other");
    changed!(archive, "other");
    changed!(candidate, "other");
    changed!(decision, "other");
    changed!(publication, "other");
    changed!(authority, "other");
    changed!(snapshot, "other");
    changed!(first_sequence, 29);
    changed!(configuration, "other");
    for field in 0..4 {
        let mut altered = identity();
        match field {
            0 => altered.pool_id = format!("pool_{}", "3".repeat(32)),
            1 => altered.disk_id = format!("disk_{}", "4".repeat(32)),
            2 => altered.slot_id = "C03".into(),
            _ => altered.binding_generation += 1,
        }
        assert_ne!(context(&altered, &original_registry).digest(&stop).unwrap(), original);
    }
    for other in [registry('b', 1), registry('a', 2)] {
        assert_ne!(context(&target, &other).digest(&stop).unwrap(), original);
    }
}

#[test]
fn full_history_and_not_only_the_same_final_snapshot_changes_the_input_key() {
    let stop = AtomicBool::new(false);
    let before = digest(&("same-final-state", vec!["old", "current"]), &stop).unwrap();
    let after = digest(&("same-final-state", vec!["other-old", "current"]), &stop).unwrap();
    assert_ne!(before, after);
    assert_eq!(before, digest(&("same-final-state", vec!["old", "current"]), &stop).unwrap());
}

#[test]
fn bounded_digest_declines_oversize_input_and_preserves_cancellation() {
    let running = AtomicBool::new(false);
    assert!(digest_bounded(&"abc", &running, 5).unwrap().is_some());
    assert_eq!(digest_bounded(&"abcd", &running, 5).unwrap(), None);
    let stopped = AtomicBool::new(true);
    assert_eq!(digest(&"abc", &stopped).unwrap_err().code(), ErrorCode::OperationAborted);
}


// Real SQLite composition below uses file-root publications only. This does
// not install a Backup decision or claim that a synthetic history is native.
struct DiskPairFixture {
    _temporary: tempfile::TempDir,
    engine: crate::Engine,
    admitted: crate::history_package::AdmittedState,
}

impl DiskPairFixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let mut engine = crate::Engine::initialize(
            &temporary.path().join("system"),
            crate::Config::development([temporary.path().join("primary")]).unwrap(),
        ).unwrap();
        let repository = engine.create_disk_root(
            "P01", "admin", crate::OperationId::generate().unwrap(),
        ).unwrap();
        let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
        engine.stage_bytes(&workspace.workspace_id, "file.txt", b"retained memo body").unwrap();
        engine.commit_workspace(crate::CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: crate::OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "memo pair fixture".into(),
            request_digest: None,
        }).unwrap();
        let raw: crate::history_package::RawState = storage_contract::decode_strict(
            &storage_contract::canonical_json(engine.state()).unwrap(),
        ).unwrap();
        let admitted = crate::history_package::AdmittedState::try_from(raw).unwrap();
        Self { _temporary: temporary, engine, admitted }
    }

    fn scopes(
        &self,
        historical_hash: Option<&str>,
    ) -> crate::engine::disk_catalog::PreparedDiskScopes {
        use crate::engine::disk_catalog::{PreparedDiskScopes, visit_published_prefix};
        let stop = AtomicBool::new(false);
        let mut scopes = PreparedDiskScopes::new(&self.engine.config).unwrap();
        visit_published_prefix(
            &self.engine,
            self.engine.state().sequence,
            &|| preparation_checkpoint(&stop),
            |candidate, hash, _, _, admitted| {
                // A deliberate typed encoder-input variant, not admission of
                // another on-disk history. Final catalog rows stay identical.
                scopes.observe(candidate, historical_hash.unwrap_or(hash), admitted, &stop)
            },
        ).unwrap();
        scopes
    }

    fn finish(
        &self,
        memo: &ScopeEncodingMemo,
        historical_hash: Option<&str>,
        encodes: &std::cell::Cell<usize>,
    ) -> Vec<(crate::DiskRegistrationIdentity, Vec<u8>, Vec<u8>)> {
        use crate::engine::disk_catalog::PreparedDiskScopes;
        let stop = AtomicBool::new(false);
        let mut session = Session::new(Some((memo, [1; 32])));
        let mut stored = Vec::new();
        self.scopes(historical_hash).finish_with_encoder(
            &self.admitted,
            &stop,
            |current, history| {
                let input = digest(&(current, history), &stop)?;
                assert!(input.is_some());
                session.disk_pair(input, || {
                    encodes.set(encodes.get() + 1);
                    PreparedDiskScopes::encode_pair(current, history)
                })
            },
            |identity, current, history| {
                stored.push((identity, current, history));
                Ok(())
            },
        ).unwrap();
        preparation_checkpoint(&stop).unwrap();
        session.commit();
        stored
    }
}

#[test]
fn real_disk_pair_hit_still_runs_sqlite_readback_and_rejects_corrupted_cached_bytes() {
    let fixture = DiskPairFixture::new();
    for corrupt_history in [false, true] {
        let memo = ScopeEncodingMemo::new();
        let encodes = std::cell::Cell::new(0);
        let first = fixture.finish(&memo, None, &encodes);
        assert_eq!(first.len(), 1);
        assert_eq!(encodes.get(), 1);
        let hit = fixture.finish(&memo, None, &encodes);
        assert_eq!(hit, first);
        assert_eq!(encodes.get(), 1);

        // Test-only corruption of retained material under the SAME key.
        // The byte count stays unchanged, so this does not alter reservations.
        {
            let mut state = memo.state.lock().unwrap();
            assert_eq!(state.entries.len(), 1);
            let Bytes::DiskPair(current, history) = state.entries.values_mut().next().unwrap() else {
                panic!("fixture only encodes disk pairs");
            };
            let bytes = if corrupt_history { history } else { current };
            let mut damaged = bytes.as_ref().to_vec();
            assert!(damaged.starts_with(b"SQLite format 3\0"));
            damaged[0] ^= 1;
            *bytes = Arc::from(damaged);
        }
        let stop = AtomicBool::new(false);
        let mut session = Session::new(Some((&memo, [1; 32])));
        let result = fixture.scopes(None).finish_with_encoder(
            &fixture.admitted,
            &stop,
            |current, history| {
                session.disk_pair(digest(&(current, history), &stop)?, || {
                    panic!("same input must hit the deliberately corrupted entry")
                })
            },
            |_, _, _| panic!("invalid SQLite must not reach the store callback"),
        );
        assert!(result.is_err());
        drop(session);
        let state = memo.state.lock().unwrap();
        assert_eq!(state.reserved_bytes, 0);
        assert_eq!(state.reserved_entries, 0);
    }
}

#[test]
fn real_disk_pair_same_final_current_different_history_misses_and_reads_back() {
    let fixture = DiskPairFixture::new();
    let memo = ScopeEncodingMemo::new();
    let encodes = std::cell::Cell::new(0);
    let first = fixture.finish(&memo, Some(&"a".repeat(64)), &encodes);
    assert_eq!(first.len(), 1);
    assert_eq!(encodes.get(), 1);
    let other = fixture.finish(&memo, Some(&"b".repeat(64)), &encodes);
    assert_eq!(encodes.get(), 2, "history participates in the full pair key");
    assert_eq!(first[0].0, other[0].0);
    assert_eq!(first[0].1, other[0].1, "final current SQLite is unchanged");
    assert_ne!(first[0].2, other[0].2, "retained history SQLite is different");
    assert_eq!(memo.state.lock().unwrap().entries.len(), 2);
    let hit = fixture.finish(&memo, Some(&"b".repeat(64)), &encodes);
    assert_eq!(hit, other);
    assert_eq!(encodes.get(), 2);
}

#[test]
fn cancellation_during_serialization_returns_original_error_without_a_partial_digest() {
    use data_serialization::{Serializer, ser::SerializeTuple};
    use std::sync::atomic::Ordering;

    struct CancelAfterFirstField<'a>(&'a AtomicBool);
    impl Serialize for CancelAfterFirstField<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
            let mut tuple = serializer.serialize_tuple(2)?;
            tuple.serialize_element("started")?;
            self.0.store(true, Ordering::Release);
            tuple.serialize_element("cancelled")?;
            tuple.end()
        }
    }

    let stop = AtomicBool::new(false);
    let error = digest(&CancelAfterFirstField(&stop), &stop).unwrap_err();
    assert_eq!(error.code(), ErrorCode::OperationAborted);
    assert!(stop.load(Ordering::Acquire));
}
