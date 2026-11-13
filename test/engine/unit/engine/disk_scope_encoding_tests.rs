//! Encoder substitution must not replace authenticated-prefix or SQLite checks.
use super::*;
use crate::history_package::RawState;
use std::cell::Cell;
use std::sync::atomic::Ordering;

struct Fixture {
    _temporary: tempfile::TempDir,
    engine: Engine,
    admitted: AdmittedState,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let mut engine = Engine::initialize(
            &temporary.path().join("system"),
            Config::development([temporary.path().join("primary")]).unwrap(),
        )
        .unwrap();
        let repository = engine
            .create_disk_root("P01", "admin", OperationId::generate().unwrap())
            .unwrap();
        let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
        engine.stage_bytes(&workspace.workspace_id, "file.txt", b"retained").unwrap();
        engine.commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "disk encoding fixture".into(),
            request_digest: None,
        }).unwrap();
        let raw: RawState =
            storage_contract::decode_strict(&storage_contract::canonical_json(engine.state()).unwrap()).unwrap();
        let admitted = AdmittedState::try_from(raw).unwrap();
        Self { _temporary: temporary, engine, admitted }
    }

    fn scopes(&self) -> PreparedDiskScopes {
        let stop = AtomicBool::new(false);
        let mut scopes = PreparedDiskScopes::new(&self.engine.config).unwrap();
        crate::engine::disk_catalog::visit_published_prefix(
            &self.engine,
            self.engine.state().sequence,
            &|| preparation_checkpoint(&stop),
            |source| scopes.observe_published(&source, &stop),
        ).unwrap();
        scopes
    }
}

#[test]
fn supplied_disk_encoder_preserves_original_bytes() {
    let fixture = Fixture::new();
    let stop = AtomicBool::new(false);
    let mut expected = Vec::new();
    fixture.scopes().finish(&fixture.admitted, &stop, |identity, current, history| {
        expected.push((identity, current, history));
        Ok(())
    }).unwrap();
    let calls = Cell::new(0);
    let mut actual = Vec::new();
    fixture.scopes().finish_with_encoder(
        &fixture.admitted,
        &stop,
        |current, history| {
            calls.set(calls.get() + 1);
            PreparedDiskScopes::encode_pair(current, history)
        },
        |identity, current, history| {
            actual.push((identity, current, history));
            Ok(())
        },
    ).unwrap();
    assert_eq!(calls.get(), expected.len());
    assert_eq!(actual, expected);
}

#[test]
fn supplied_disk_encoder_cannot_bypass_original_prefix_validation() {
    let fixture = Fixture::new();
    let mut scopes = fixture.scopes();
    scopes.disks[0].header.through_sequence += 1;
    let error = scopes.finish_with_encoder(
        &fixture.admitted,
        &AtomicBool::new(false),
        |_, _| panic!("encoder must not run before prefix admission"),
        |_, _, _| panic!("invalid prefix must never be stored"),
    );
    assert!(error.is_err());
}

#[test]
fn supplied_disk_bytes_are_decoded_and_compared_before_storage() {
    let fixture = Fixture::new();
    for change_history in [false, true] {
        let result = fixture.scopes().finish_with_encoder(
            &fixture.admitted,
            &AtomicBool::new(false),
            |current, history| {
                let mut current = current.clone();
                let mut history = history.clone();
                if change_history {
                    history.header.through_sequence += 1;
                } else {
                    current.source_sequence += 1;
                }
                PreparedDiskScopes::encode_pair(&current, &history)
            },
            |_, _, _| panic!("another input's encoded bytes must not be stored"),
        );
        assert!(result.is_err());
    }
}

#[test]
fn cancellation_after_supplied_encoding_prevents_storage() {
    let fixture = Fixture::new();
    let stop = AtomicBool::new(false);
    let result = fixture.scopes().finish_with_encoder(
        &fixture.admitted,
        &stop,
        |current, history| {
            let encoded = PreparedDiskScopes::encode_pair(current, history)?;
            stop.store(true, Ordering::Release);
            Ok(encoded)
        },
        |_, _, _| panic!("cancelled encoding must not be stored"),
    );
    assert_eq!(result.unwrap_err().code(), ErrorCode::OperationAborted);
}
