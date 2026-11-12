//! Current-format evidence produced by real writes; not a reader for old pools.
use crate::{CommitRequest, Config, Engine, OperationId, State, canonical_json};
use crate::disk::MAX_METADATA;
use crate::journal::{PreparedMetadataObject, prepare_snapshot};
use std::collections::BTreeMap;
use std::sync::OnceLock;

pub(crate) fn state() -> State {
    static STATE: OnceLock<State> = OnceLock::new();
    STATE
        .get_or_init(|| {
            let temporary = tempfile::tempdir().unwrap();
            let mut engine = Engine::initialize(
                &temporary.path().join("system"),
                Config::development([temporary.path().join("disk")]).unwrap(),
            )
            .unwrap();
            let repository = engine
                .create_disk_root("P01", "admin", OperationId::generate().unwrap())
                .unwrap();
            for (path, body) in [
                ("folder/a.txt", "first a"),
                ("folder/b.txt", "first b"),
                ("folder/a.txt", "second a"),
                ("folder/b.txt", "second b"),
            ] {
                let workspace = engine
                    .create_workspace(&repository.main_branch_id, "admin")
                    .unwrap();
                engine
                    .stage_bytes(&workspace.workspace_id, path, body.as_bytes())
                    .unwrap();
                engine
                    .commit_workspace(CommitRequest {
                        workspace_id: workspace.workspace_id,
                        operation_id: OperationId::generate().unwrap(),
                        principal: "admin".into(),
                        expected_head: workspace.base_head,
                        fencing_token: workspace.fencing_token,
                        message: "current container tag fixture".into(),
                        request_digest: None,
                    })
                    .unwrap();
            }
            let file = engine
                .list_tree(&repository.main_branch_id)
                .unwrap()
                .into_iter()
                .find(|entry| entry.kind == crate::NamespaceEntryKind::File)
                .unwrap();
            engine
                .create_file_branch(
                    &file.file_id,
                    &file.version_id,
                    &storage_contract::BranchName::parse("fixture-retained").unwrap(),
                    "admin",
                    OperationId::generate().unwrap(),
                )
                .unwrap();
            engine
                .initialize_directory_scope(
                    &repository.repository_id,
                    Some(&name_policy::LookupPath::parse("folder").unwrap()),
                    "admin",
                    OperationId::generate().unwrap(),
                )
                .unwrap();
            engine.state().clone()
        })
        .clone()
}

fn objects(prepared: &[PreparedMetadataObject]) -> BTreeMap<String, String> {
    prepared.iter().map(|object| (
        format!("{}/{}", object.kind(), object.hash),
        String::from_utf8(object.bytes().to_vec()).unwrap(),
    )).collect()
}

pub(crate) fn publication() -> json_codec::Value {
    let state = state();
    let snapshot = prepare_snapshot(&state, MAX_METADATA).unwrap();
    let (histories, history_objects) =
        crate::history_package::prepare_history_manifests(&state, MAX_METADATA).unwrap();
    json_codec::json!({
        "state_bytes": String::from_utf8(canonical_json(&state).unwrap()).unwrap(),
        "snapshot_hash": snapshot.hash,
        "snapshot_objects": objects(&snapshot.objects),
        "history_roots": histories.reference(),
        "history_objects": objects(&history_objects),
    })
}
