//! Real disposable journal/object reads, not caller-constructed proof tokens.
use super::*;
use crate::journal::object_path;
use std::sync::atomic::AtomicBool;

struct Fixture {
    temporary: tempfile::TempDir,
    engine: Engine,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let mut engine = Engine::initialize(
            &temporary.path().join("system"),
            Config::development([temporary.path().join("primary")]).unwrap(),
        ).unwrap();
        let repository = engine.create_disk_root(
            "P01", "admin", OperationId::generate().unwrap(),
        ).unwrap();
        let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
        engine.stage_bytes(&workspace.workspace_id, "file.txt", b"published source").unwrap();
        engine.commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "published snapshot fixture".into(),
            request_digest: None,
        }).unwrap();
        Self { temporary, engine }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Observation {
    sequence: u64,
    candidate: String,
    publication: String,
    snapshot: String,
    state: Vec<u8>,
}

fn observe(engine: &Engine) -> Vec<Observation> {
    let mut observations = Vec::new();
    visit_published_prefix(engine, engine.state.sequence, &|| Ok(()), |source| {
        let candidate = source.candidate();
        let state = source.admitted().state();
        assert_eq!(source.candidate_hash().as_str(), object_hash("candidate", candidate).unwrap());
        assert_eq!(
            source.publication_hash().as_str(),
            object_hash("publication", source.publication()).unwrap(),
        );
        assert_eq!(candidate.sequence, state.sequence);
        assert_eq!(
            &state.operations[&candidate.operation_id],
            &source.publication().result,
        );
        observations.push(Observation {
            sequence: candidate.sequence,
            candidate: source.candidate_hash().as_str().into(),
            publication: source.publication_hash().as_str().into(),
            snapshot: candidate.snapshot_hash.clone(),
            state: canonical_json(state).unwrap(),
        });
        Ok(())
    }).unwrap();
    assert_eq!(observations.last().unwrap().sequence, engine.state.sequence);
    observations
}

#[test]
fn published_snapshot_proof_retains_exact_original_material_after_cold_open() {
    let Fixture { temporary, engine } = Fixture::new();
    let expected = observe(&engine);
    assert!(expected.len() >= 2);
    drop(engine);
    let reopened = Engine::open(&temporary.path().join("system")).unwrap();
    assert_eq!(observe(&reopened), expected);
    assert_eq!(observe(&reopened), expected);
}

#[test]
fn another_well_formed_object_cannot_replace_the_published_snapshot_proof() {
    for kind in ["candidate", "publication", "snapshot"] {
        let Fixture { temporary, engine } = Fixture::new();
        let expected = observe(&engine);
        let first = &expected[0];
        let last = expected.last().unwrap();
        let (earlier, selected) = match kind {
            "candidate" => (&first.candidate, &last.candidate),
            "publication" => (&first.publication, &last.publication),
            "snapshot" => (&first.snapshot, &last.snapshot),
            _ => panic!("fixture enumerates the three authenticated object domains"),
        };
        assert_ne!(earlier, selected);
        let mut restore = Vec::new();
        for member in engine.members() {
            let path = object_path(&member, kind, selected);
            let original = std::fs::read(&path).unwrap();
            let replacement = std::fs::read(object_path(&member, kind, earlier)).unwrap();
            std::fs::write(&path, &replacement).unwrap();
            restore.push((path, original, replacement));
        }
        let mut received = Vec::new();
        let rejected = visit_published_prefix(
            &engine, engine.state.sequence, &|| Ok(()), |source| {
                received.push(source.candidate().sequence);
                Ok(())
            },
        );
        assert!(rejected.is_err(), "{kind} substitution must fail");
        assert!(!received.contains(&last.sequence));
        for (path, original, replacement) in restore {
            assert_eq!(std::fs::read(&path).unwrap(), replacement);
            std::fs::write(path, original).unwrap();
        }
        drop(engine);
        let reopened = Engine::open(&temporary.path().join("system")).unwrap();
        assert_eq!(observe(&reopened), expected);
    }
}

#[test]
fn authenticated_steps_do_not_certify_an_unmatched_current_prefix_endpoint() {
    let Fixture { temporary: _temporary, mut engine } = Fixture::new();
    let expected = observe(&engine);
    let original = engine.snapshot.clone();
    engine.snapshot = Some(expected[0].snapshot.clone());
    let mut received = 0;
    let rejected = visit_published_prefix(
        &engine, engine.state.sequence, &|| Ok(()), |_| {
            received += 1;
            Ok(())
        },
    );
    assert!(rejected.is_err());
    assert_eq!(received, expected.len());
    engine.snapshot = original;
    assert_eq!(observe(&engine), expected);
}

#[test]
fn cancelled_replay_delivers_no_publication_proof() {
    let fixture = Fixture::new();
    let stop = AtomicBool::new(true);
    let result = visit_published_prefix(
        &fixture.engine,
        fixture.engine.state.sequence,
        &|| crate::history_package::preparation_checkpoint(&stop),
        |_| panic!("cancelled source must not reach the projection"),
    );
    assert!(result.is_err());
    assert!(!observe(&fixture.engine).is_empty());
}

#[test]
fn published_scope_consumer_preserves_bytes_and_staged_sequence_rejection() {
    let fixture = Fixture::new();
    let stop = AtomicBool::new(false);
    let mut proved = preparation::PreparedDiskScopes::new(&fixture.engine.config).unwrap();
    let mut staged = preparation::PreparedDiskScopes::new(&fixture.engine.config).unwrap();
    let mut proved_bytes = Vec::new();
    let mut staged_bytes = Vec::new();
    visit_published_prefix(
        &fixture.engine, fixture.engine.state.sequence, &|| Ok(()), |source| {
            proved.observe_published(&source, &stop)?;
            let mut mismatched = source.candidate().clone();
            // This bounded fixture stays forward of the previously observed
            // sequence, so only the staged candidate/State relation can reject.
            mismatched.sequence = source.candidate().sequence + 1;
            assert!(staged.observe(
                &mismatched, source.candidate_hash().as_str(), source.admitted(), &stop,
            ).is_err());
            staged.observe(
                source.candidate(), source.candidate_hash().as_str(), source.admitted(), &stop,
            )?;
            Ok(())
        },
    ).unwrap();
    let raw: crate::history_package::RawState = storage_contract::decode_strict(
        &canonical_json(fixture.engine.state()).unwrap(),
    ).unwrap();
    let admitted = crate::history_package::AdmittedState::try_from(raw).unwrap();
    proved.finish(&admitted, &stop, |_, current, history| {
        proved_bytes.extend([current, history]);
        Ok(())
    }).unwrap();
    staged.finish(&admitted, &stop, |_, current, history| {
        staged_bytes.extend([current, history]);
        Ok(())
    }).unwrap();
    assert_eq!(proved_bytes, staged_bytes);
    assert_eq!(proved_bytes.len(), 2);
}

#[test]
fn replaced_writer_name_cannot_supply_proof_from_a_still_held_journal() {
    let Fixture { temporary, engine } = Fixture::new();
    let expected = observe(&engine);
    let member = &engine.members()[0];
    let path = crate::engine::membership_transition::pending::activation::log_path(
        &engine.config, member,
    );
    let held = path.with_extension("retained-journal");
    let original = std::fs::read(&path).unwrap();
    std::fs::rename(&path, &held).unwrap();
    std::fs::write(&path, &original).unwrap();
    let rejected = visit_published_prefix(
        &engine, engine.state.sequence, &|| Ok(()), |_| {
            panic!("a byte-identical replacement is not the original held writer")
        },
    );
    assert!(rejected.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(std::fs::read(&held).unwrap(), original);
    std::fs::rename(held, path).unwrap();
    drop(engine);
    let reopened = Engine::open(&temporary.path().join("system")).unwrap();
    assert_eq!(observe(&reopened), expected);
}
