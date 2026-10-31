use super::*;

#[test]
fn retained_snapshot_mismatch_rejects_before_repair_or_visible_state_change() {
    for mode in [RecoveryMode::CompletePending, RecoveryMode::PreservePending] {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("system");
        let mut engine = Engine::initialize(
            &root,
            Config::development([
                temporary.path().join("member-a"),
                temporary.path().join("member-b"),
            ]),
        )
        .unwrap();
        for name in ["first", "second"] {
            engine
                .create_repository(
                    &storage_contract::RepositoryName::parse(name).unwrap(),
                    "P01",
                    1,
                    "admin",
                    storage_contract::OperationId::generate(),
                )
                .unwrap();
        }
        // Model the initial replay view while keeping the real admitted disk
        // handles and journals. No synthetic snapshot or publication is written.
        engine.state = std::sync::Arc::new(State {
            topology_generation: 1,
            ..State::default()
        });
        engine.snapshot = None;
        let mut prepared = engine.prepare_recovery().unwrap();
        let (sequence, candidates) = prepared.records.first_key_value().unwrap();
        let (hash, receipts) = candidates.first_key_value().unwrap();
        let mut first = engine
            .read_workspace_receipt_candidate(&prepared.online, *sequence, hash, receipts, &None)
            .unwrap();
        let (_, last_candidates) = prepared.records.last_key_value().unwrap();
        let (last_hash, _) = last_candidates.first_key_value().unwrap();
        let last: Candidate = verified_object_at(
            &engine.disk_resources,
            &prepared.online,
            "candidate",
            last_hash,
        )
        .unwrap();
        assert_ne!(first.candidate.snapshot_hash, last.snapshot_hash);
        first.snapshot = Some(
            verified_recovery_snapshot_at(
                &engine.disk_resources,
                &prepared.online,
                &last.snapshot_hash,
            )
            .unwrap()
            .into_shared(),
        );
        let missing_copy = object_path(&prepared.members[0], "candidate", hash);
        std::fs::remove_file(&missing_copy).unwrap();
        let journals: Vec<_> = prepared
            .members
            .iter()
            .map(|member| (log_path(member), std::fs::read(log_path(member)).unwrap()))
            .collect();
        let original_state = canonical_json(engine.state.as_ref()).unwrap();
        prepared.candidates.insert(*sequence, first);

        let error = engine.recover_prepared(mode, prepared).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ObjectCorrupt);
        assert!(error.to_string().contains("admitted snapshot differs"));
        assert_eq!(
            canonical_json(engine.state.as_ref()).unwrap(),
            original_state
        );
        assert!(engine.snapshot.is_none());
        assert!(
            !missing_copy.exists(),
            "rejection must precede replica repair"
        );
        for (path, bytes) in journals {
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
    }
}
