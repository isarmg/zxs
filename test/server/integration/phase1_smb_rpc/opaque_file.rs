//! Real local bridge socket and Engine: ZIP is a FILE, never a traversable namespace.
use super::*;

#[test]
fn registered_shares_keep_opaque_zip_as_one_file_and_pin_its_original_version() {
    // Valid empty ZIP end-of-central-directory with no entries and no comment.
    let mut zip = [0_u8; 22];
    zip[..4].copy_from_slice(b"PK\x05\x06");
    let root = tempfile::tempdir().unwrap();
    let system = root.path().join("state");
    let config = Config::development([(
        name_policy::Name::parse("p").unwrap(),
        root.path().join("mounted-primary"),
    )]).unwrap();
    let mut engine = Engine::initialize(&system, config).unwrap();
    let repository = engine.create_disk_root("P01", "alice", OperationId::generate().unwrap())
        .unwrap();
    let workspace = engine.create_workspace(&repository.main_branch_id, "alice").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "box/project.zxs", &zip).unwrap();
    engine.commit_workspace(storage_contract::CommitRequest {
        workspace_id: workspace.workspace_id, principal: "alice".into(),
        operation_id: OperationId::generate().unwrap(), expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token, message: "ordinary archive FILE".into(),
        request_digest: None,
    }).unwrap();
    let original = engine.namespace_tree(&repository.main_branch_id).unwrap()
        .get("box/project.zxs").unwrap().clone();
    let auth = AuthStore::initialize(&system, "alice", &random_test_value::<24>()).unwrap();
    let mut fixture = Fixture::start(root, engine, auth);
    let default = fixture.connect();
    let named = Client::connect(&fixture.root.path().join("run/bridge.sock"), "alice",
        &repository.repository_id).unwrap();
    let path = "p/box/project.zxs";
    let relative = "box/project.zxs";
    assert_eq!(directory(&default, "p/box"), vec![relative.to_owned()]);
    let current = default.stat(path.into()).unwrap();
    assert_eq!(current.kind, storage_contract::NamespaceEntryKind::File);
    assert_eq!(current.entry_id, original.entry_id);
    assert_eq!(current.entry_id, named.stat(relative.into()).unwrap().entry_id);
    assert!(default.stat(format!("{path}/child")).is_err());
    // rmdir is empty-only, regardless of whether a child happens to be ZIP data.
    let error = default.call(Request::Namespace {
        kind: NamespaceOperation::Rmdir, path: "p/box".into(), destination: None,
        replace: false, operation_id: OperationId::generate().unwrap(),
    }).unwrap_err();
    let remote = error.get_ref().unwrap().downcast_ref::<RemoteError>().unwrap();
    assert_eq!(remote.code, RemoteErrorCode::Server(ErrorCode::DirectoryNotEmpty));
    assert_eq!(default.stat(path.into()).unwrap().entry_id, original.entry_id);

    let (reader, _placeholder) = open(&named, relative, false, false, false);
    assert_eq!(reader.version_id.as_deref(), Some(original.version_id.as_str()));
    assert_eq!(read(&default, path).1, zip.as_slice());

    // A non-ZIP replacement is equally valid; never open a decoder or sidecar.
    let opaque = b"ordinary arbitrary bytes";
    save(&default, path, opaque, false);
    let next = fixture.entry(&repository.main_branch_id, relative);
    assert_eq!(next.file_id, original.file_id);
    assert_eq!(next.entry_id, original.entry_id);
    assert_ne!(next.version_id, original.version_id);
    let mut held = [0; 22];
    assert_eq!(named.read(reader.handle, 0, &mut held).unwrap(), zip.len());
    assert_eq!(held, zip);
    assert_eq!(read(&named, relative).1, opaque);
    close(&named, reader.handle);
    drop((named, default));
    let (root, engine) = fixture.cold_open();
    assert_eq!(engine.file_history(&original.file_id).unwrap().len(), 2);
    assert_eq!(exported(&engine, root.path(), &original.version_id), zip.as_slice());
    assert_eq!(exported(&engine, root.path(), &next.version_id), opaque);
}
