//! Exact scope of the prepared object database, not an authority to select a disk.
use super::*;

pub(crate) fn require_transition_bytes(admitted: &AdmittedState, image: &PreparedDatabases) -> Result<()> {
    let transition = crate::state_projection::ProjectionTransition::new(
        admitted.state(), admitted.state(), None,
    )?;
    for database in image.databases() {
        let Scope::File { history, file } = &database.descriptor.scope;
        let expected = file_database::prepare(transition.previous(), file, history)?;
        assert_eq!(database.bytes(), expected.bytes);
    }
    Ok(())
}

#[test]
fn prepared_database_descriptor_refuses_a_directory_scope_or_mixed_file_owner() {
    let file = crate::new_id("file").unwrap();
    let history = crate::new_id("history").unwrap();
    let repository = crate::new_id("repo").unwrap();
    let hash = blake3::hash(b"one object").to_hex().to_string();
    let descriptor = |scope: json_codec::Value| json_codec::json!({
        "scope":scope, "bytes":"10", "digest":hash
    });
    let file_scope = json_codec::json!({"kind":"file","history":history,"file":file});
    let original = descriptor(file_scope.clone());
    let valid: Descriptor = storage_contract::decode_strict(&crate::canonical_json(&original).unwrap()).unwrap();
    assert_eq!(valid.scope.path(), PathBuf::from("histories").join(&history)
        .join("files").join(file).join(file_database::NAME));
    let mut mixed = file_scope;
    mixed["directory"] = repository.clone().into();
    for forbidden in [mixed, json_codec::json!({
        "kind":"directory","history":history,"repository":repository,"directory":repository
    })] {
        let bytes = crate::canonical_json(&descriptor(forbidden)).unwrap();
        assert!(storage_contract::decode_strict::<Descriptor>(&bytes).is_err());
    }
}

#[test]
fn registered_unborn_file_is_not_a_complete_empty_backup_object_set() {
    use crate::{Config, Engine, Mode, OperationId};
    use storage_contract::file_draft::BirthRequest;
    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("system");
    let config = Config::independent_roots(
        Mode::Development,
        [(name_policy::Name::parse("Primary").unwrap(), temp.path().join("primary"))],
    ).unwrap();
    let slot = config.disks[0].slot_id.clone();
    let mut engine = Engine::initialize(&system, config).unwrap();
    let root = engine.create_disk_root(&slot, "admin", OperationId::generate().unwrap()).unwrap();
    let birth = BirthRequest {
        repository: root.repository_id.parse().unwrap(),
        path: name_policy::PortablePath::parse("project.zxs").unwrap(),
        expected_namespace_head: engine.state().branches[&root.main_branch_id]
            .head_commit_id.parse().unwrap(),
    };
    engine.create_unborn_file(
        &birth, &"admin".parse().unwrap(), OperationId::generate().unwrap(),
    ).unwrap();
    let state = AdmittedState::admit_replayed(engine.state().clone()).unwrap();
    assert_eq!(state.state().file_containers.len(), 1);
    assert!(state.state().versions.is_empty());
    let error = PreparedDatabases::capture(&state, &AtomicBool::new(false)).err().unwrap();
    assert_eq!(error.code(), ErrorCode::UnsupportedOperation);
}
