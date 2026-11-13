//! Complete preparation-image and live-WAL projection readback regressions.
use super::*;
use sqlite_access::{Execute, Sqlite, sqlite::SqliteOwnedBuf};
use std::cell::Cell;

fn fixture() -> (tempfile::TempDir, Engine, Vec<u8>) {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temp.path().join("system"),
        Config::development([temp.path().join("primary")]).unwrap(),
    )
    .unwrap();
    let repository = engine
        .create_repository(
            &RepositoryName::parse("readback").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "before.txt", b"retained body")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "prepared SYSTEM image".into(),
            request_digest: None,
        })
        .unwrap();
    engine
        .file_namespace(
            &repository.repository_id,
            NamespaceOperation::parse(
                NamespaceOperationKind::Rename,
                "before.txt",
                Some("after.txt"),
            )
            .unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let bytes = encode_state(engine.state(), &|| Ok(())).unwrap();
    (temp, engine, bytes)
}

fn change(bytes: &[u8], query: impl Execute<'static, Sqlite> + 'static) -> Vec<u8> {
    platform_io::sqlite::block_on_sqlite_connection(async {
        let mut connection = sqlite_access::SqliteConnection::connect_with(
            &SqliteConnectOptions::new().in_memory(true),
        )
        .await
        .unwrap();
        connection
            .deserialize(None, SqliteOwnedBuf::try_from(bytes).unwrap(), false)
            .await
            .unwrap();
        connection.execute(query).await.unwrap();
        let result = connection.serialize(None).await.unwrap().to_vec();
        connection.close().await.unwrap();
        result
    })
}

fn reject(bytes: &[u8], state: &State) {
    assert_eq!(
        readback::verify(bytes, state, &|| Ok(()))
            .unwrap_err()
            .code(),
        ErrorCode::ContainerLayoutInvalid
    );
}

#[test]
fn complete_prepared_system_image_survives_a_different_valid_sqlite_page_layout() {
    let (_temp, engine, bytes) = fixture();
    readback::verify(&bytes, engine.state(), &|| Ok(())).unwrap();
    let repacked = change(&bytes, "PRAGMA page_size=8192; VACUUM;");
    assert_ne!(repacked, bytes);
    readback::verify(&repacked, engine.state(), &|| Ok(())).unwrap();
}

#[test]
fn same_sequence_but_other_original_state_is_not_the_same_system_material() {
    let (_temp, engine, bytes) = fixture();
    let mut other = engine.state().clone();
    let file = other.versions.values().next().unwrap().file_id.clone();
    other
        .file_notes
        .insert(file, "another retained note".into());
    assert_eq!(other.sequence, engine.state().sequence);
    reject(&bytes, &other);
}

#[test]
fn original_version_and_membership_rows_cannot_be_missing_changed_or_extended() {
    let (_temp, engine, bytes) = fixture();
    for sql in [
        "DELETE FROM file_history_versions",
        "UPDATE file_history_versions SET version_sequence_key=X'ffffffffffffffff'",
        "INSERT INTO file_history_versions VALUES('extra','file','history',X'0000000000000000')",
        "DELETE FROM history_scope_members WHERE kind=0",
        "UPDATE history_scope_members SET file_id='other' WHERE kind=0",
        "INSERT INTO history_scope_members VALUES(99,'extra','','file','history')",
        "DELETE FROM current_state",
        "UPDATE current_state SET sequence='0001'",
    ] {
        reject(&change(&bytes, sql), engine.state());
    }
}

#[test]
fn every_present_global_family_and_exact_original_principal_must_survive() {
    let (_temp, engine, bytes) = fixture();
    let kinds: Vec<i64> = platform_io::sqlite::block_on_sqlite_connection(async {
        let mut connection = sqlite_access::SqliteConnection::connect_with(
            &SqliteConnectOptions::new().in_memory(true),
        )
        .await
        .unwrap();
        connection
            .deserialize(
                None,
                SqliteOwnedBuf::try_from(bytes.as_slice()).unwrap(),
                true,
            )
            .await
            .unwrap();
        let kinds =
            sqlite_access::query_scalar("SELECT DISTINCT kind FROM system_changes ORDER BY kind")
                .fetch_all(&mut connection)
                .await
                .unwrap();
        connection.close().await.unwrap();
        kinds
    });
    for kind in [0, 2, 17, 36, 37, 38] {
        assert!(
            kinds.contains(&kind),
            "fixture must actually contain family {kind}"
        );
    }
    assert!(!kinds.contains(&1), "rename facts must not manufacture path nodes");
    assert_eq!(engine.state().operations.values().filter(|receipt| {
        receipt.result.get("path_changes").and_then(|facts| facts.as_array())
            .is_some_and(|facts| !facts.is_empty())
    }).count(), 1);
    for kind in kinds {
        reject(
            &change(
                &bytes,
                sqlite_access::query("DELETE FROM system_changes WHERE kind=?").bind(kind),
            ),
            engine.state(),
        );
    }
    // The permanent rename fact is inside its original operation payload.
    reject(
        &change(
            &bytes,
            "UPDATE system_changes SET payload=replace(payload,'before.txt','forged.txt') WHERE kind=0",
        ),
        engine.state(),
    );
    reject(
        &change(
            &bytes,
            "INSERT INTO system_changes VALUES(1,'unexpected',X'0000000000000000','{}')",
        ),
        engine.state(),
    );
    // Same-length, valid JSON and unchanged primary/sequence keys are not
    // sufficient when the original operation owner has been rewritten.
    reject(
        &change(
            &bytes,
            "UPDATE system_changes SET payload=replace(payload,'admin','other') WHERE kind=36",
        ),
        engine.state(),
    );
    reject(
        &change(
            &bytes,
            "INSERT INTO system_changes VALUES(38,'extra',X'0000000000000000','{}')",
        ),
        engine.state(),
    );
}

#[test]
fn prepared_system_readback_rejects_unknown_schema_instead_of_ignoring_it() {
    let (_temp, engine, bytes) = fixture();
    for sql in [
        "CREATE TABLE retained(value TEXT); INSERT INTO retained VALUES('do not discard');",
        "CREATE INDEX unrecognized ON system_changes(id)",
        "CREATE VIEW retained AS SELECT payload FROM system_changes",
        "CREATE TRIGGER retained AFTER INSERT ON current_state BEGIN SELECT 1; END",
    ] {
        reject(&change(&bytes, sql), engine.state());
    }
}

#[test]
fn physically_damaged_system_pages_are_not_certified_by_matching_logical_rows() {
    let (_temp, engine, mut bytes) = fixture();
    let page: i64 = platform_io::sqlite::block_on_sqlite_connection(async {
        let mut connection = sqlite_access::SqliteConnection::connect_with(
            &SqliteConnectOptions::new().in_memory(true),
        )
        .await
        .unwrap();
        connection
            .deserialize(
                None,
                SqliteOwnedBuf::try_from(bytes.as_slice()).unwrap(),
                true,
            )
            .await
            .unwrap();
        let page = sqlite_access::query_scalar(
            "SELECT rootpage FROM sqlite_schema WHERE name='current_state'",
        )
        .fetch_one(&mut connection)
        .await
        .unwrap();
        connection.close().await.unwrap();
        page
    });
    let size = usize::from(u16::from_be_bytes([bytes[16], bytes[17]]));
    assert_eq!(size, 4096);
    let header = (usize::try_from(page).unwrap() - 1) * size;
    assert_eq!(bytes[header], 0x0d);
    assert!(bytes[header + 7] < 60);
    bytes[header + 7] += 1;
    reject(&bytes, engine.state());
}

#[test]
fn preparation_readback_retains_full_u64_state_metadata() {
    let (_temp, engine, _) = fixture();
    let mut state = engine.state().clone();
    state.sequence = u64::MAX;
    state.business_generation = u64::MAX;
    state.topology_generation = u64::MAX;
    let bytes = encode_state(&state, &|| Ok(())).unwrap();
    readback::verify(&bytes, &state, &|| Ok(())).unwrap();
    reject(
        &change(
            &bytes,
            "UPDATE current_state SET sequence='18446744073709551614'",
        ),
        &state,
    );
}

#[test]
fn preparation_readback_reports_cancellation_without_a_partial_success() {
    let (_temp, engine, bytes) = fixture();
    let checks = Cell::new(0);
    let error = readback::verify(&bytes, engine.state(), &|| {
        checks.set(checks.get() + 1);
        if checks.get() == 3 {
            Err(Error::new(
                ErrorCode::OperationAborted,
                "original preparation cancelled",
            ))
        } else {
            Ok(())
        }
    })
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::OperationAborted);
    assert_eq!(checks.get(), 3);
}

fn live_writer(path: &Path, state: &State) -> IndexConnection {
    let mut writer = IndexConnection::open(path, true).unwrap();
    writer
        .project(None, state, &HistoryDelta::between(None, &state.versions))
        .unwrap();
    writer
}

#[test]
fn move_readback_verifies_a_live_wal_projection_without_changing_its_mode() {
    let (temp, engine, _) = fixture();
    let path = temp.path().join("move-readback.sqlite");
    let mut writer = live_writer(&path, engine.state());
    let bytes = platform_io::sqlite::block_on_sqlite_connection(async {
        writer
            .connection
            .as_mut()
            .unwrap()
            .serialize(None)
            .await
            .unwrap()
    });
    // This is the exact input that cannot be deserialized into :memory:.
    assert_eq!(&bytes[18..20], &[2, 2]);
    assert!(
        std::fs::metadata(path.with_extension("sqlite-wal"))
            .unwrap()
            .len()
            > 0
    );
    writer.check_identity(&path).unwrap();
    writer.verify_move_projection(engine.state()).unwrap();
    writer.check_identity(&path).unwrap();
    let mode: String = platform_io::sqlite::block_on_sqlite_connection(async {
        sqlite_access::query_scalar("PRAGMA journal_mode")
            .fetch_one(writer.connection.as_mut().unwrap())
            .await
            .unwrap()
    });
    assert_eq!(mode, "wal");
    writer.close().unwrap();
}

#[test]
fn live_wal_move_readback_rejects_changed_rows_without_repairing_them() {
    let (temp, engine, _) = fixture();
    let mut writer = live_writer(&temp.path().join("move-readback.sqlite"), engine.state());
    for sql in [
        "DELETE FROM file_history_versions",
        "UPDATE history_scope_members SET file_id='other' WHERE kind=0",
        "UPDATE system_changes SET payload=replace(payload,'admin','other') WHERE kind=36",
    ] {
        platform_io::sqlite::block_on_sqlite_connection(async {
            writer
                .connection
                .as_mut()
                .unwrap()
                .execute(sql)
                .await
                .unwrap();
        });
        assert_eq!(
            writer
                .verify_move_projection(engine.state())
                .unwrap_err()
                .code(),
            ErrorCode::ContainerLayoutInvalid
        );
        // A rejected observation must not rewrite the rows or keep its read
        // transaction open. The same bad rows remain rejected until the owner
        // explicitly projects the authoritative State again.
        assert_eq!(
            writer
                .verify_move_projection(engine.state())
                .unwrap_err()
                .code(),
            ErrorCode::ContainerLayoutInvalid
        );
        writer
            .project(
                None,
                engine.state(),
                &HistoryDelta::between(None, &engine.state().versions),
            )
            .unwrap();
        writer.verify_move_projection(engine.state()).unwrap();
    }
    writer.close().unwrap();
}

fn admitted_encoding_state(state: &State) -> AdmittedState {
    let raw: crate::history_package::RawState =
        storage_contract::decode_strict(&storage_contract::canonical_json(state).unwrap()).unwrap();
    AdmittedState::try_from(raw).unwrap()
}

#[test]
fn reusable_system_input_keeps_exact_bytes_and_distinguishes_same_sequence_rows() {
    let (_temp, engine, expected) = fixture();
    let admitted = admitted_encoding_state(engine.state());
    let input = SystemEncodingInput::new(&admitted, &|| Ok(())).unwrap();
    let actual = input.encode(&|| Ok(())).unwrap();
    assert_eq!(actual, expected);
    input.verify(&actual, &|| Ok(())).unwrap();

    let mut changed = engine.state().clone();
    let file = changed.versions.values().next().unwrap().file_id.clone();
    changed.file_notes.insert(file, "different encoding input".into());
    let changed = admitted_encoding_state(&changed);
    let other = SystemEncodingInput::new(&changed, &|| Ok(())).unwrap();
    assert_ne!(input.input_hash(), other.input_hash());
    assert!(other.verify(&actual, &|| Ok(())).is_err());
}

#[test]
fn reusable_system_input_rechecks_supplied_sqlite_instead_of_trusting_its_key() {
    let (_temp, engine, bytes) = fixture();
    let admitted = admitted_encoding_state(engine.state());
    let input = SystemEncodingInput::new(&admitted, &|| Ok(())).unwrap();
    input.verify(&bytes, &|| Ok(())).unwrap();
    let corrupted = change(&bytes, "DELETE FROM file_history_versions");
    assert_eq!(
        input.verify(&corrupted, &|| Ok(())).unwrap_err().code(),
        ErrorCode::ContainerLayoutInvalid
    );
}

#[test]
fn reusable_system_input_checks_cancellation_at_construction_encoding_and_readback() {
    let (_temp, engine, bytes) = fixture();
    let admitted = admitted_encoding_state(engine.state());
    let stopped = AtomicBool::new(true);
    let checkpoint = || preparation_checkpoint(&stopped);
    assert!(SystemEncodingInput::new(&admitted, &checkpoint).is_err());
    let input = SystemEncodingInput::new(&admitted, &|| Ok(())).unwrap();
    assert_eq!(input.encode(&checkpoint).unwrap_err().code(), ErrorCode::OperationAborted);
    assert_eq!(input.verify(&bytes, &checkpoint).unwrap_err().code(), ErrorCode::OperationAborted);
}
