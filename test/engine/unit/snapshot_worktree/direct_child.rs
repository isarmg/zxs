//! Real parent registration, Mkdir, namespace FILE and child snapshot
//! publication. A sibling, a matching spelling or bytes never supplies a host.
use super::*;
use crate::persist::layout;
use std::{fs, io::Read, os::unix::fs::PermissionsExt};
use storage_contract::OperationId;
use storage_contract::snapshot_worktree::InitializeSnapshot;

#[path = "draft_gateway/fixture.rs"]
mod fixture;
use fixture::Fixture;

#[test]
fn child_initialization_uses_original_mkdir_entity_copies_only_relative_files_and_cold_replays() {
    let (fixture, mut engine) = Fixture::with_nested_directory(true);
    let repository = fixture.targets[0].repository;
    let actor = "admin".parse().unwrap();
    let context = engine.snapshot_direct_child_context(repository,&LookupPath::parse("folder").unwrap()).unwrap();
    let parent = engine.snapshot_root_context(repository).unwrap();
    assert_eq!(context.source.root.as_ref().unwrap().as_str(),"Folder");
    assert_eq!(context.host.root_id,parent.host.root_id);
    assert_ne!(context.host.directory_id,parent.host.directory_id);
    let folder = engine.locate_current_object(&repository.to_string(),&LookupPath::parse("Folder").unwrap()).unwrap();
    let directory = &engine.state().namespace_directories[&folder.entry().entry_id];
    assert_eq!(context.host.directory_id,directory.replicas[&directory.owner_disk].preparation);
    let creation = directory.replicas[&directory.owner_disk].created_by;
    let body = engine.locate_current_object(&repository.to_string(),&LookupPath::parse("Folder/Child/existing.txt").unwrap())
        .unwrap().entry().version_id.clone();
    let request = InitializeSnapshot {host:context.host,source:context.source,operation_id:OperationId::generate().unwrap()};
    let child = engine.initialize_directory_snapshot(&request,&actor).unwrap();
    assert_ne!(child.repository_id,fixture.source.repository_id);
    assert_ne!(child.branch_id,fixture.source.branch_id);
    let disk = fixture.snapshot_path();
    let child_repo = disk.parent().unwrap().join("Folder").join(layout::SNAPSHOT_REPOSITORY.as_path());
    assert!(child_repo.is_dir());
    let index = engine.snapshot_receipts().unwrap();
    let inner = index.node(child).unwrap();
    assert_eq!(inner.host().directories[0].directory.operation_id,creation);
    let node = index.reopen_node(&engine,child).unwrap();
    let paths = node.manifest().entries().iter().map(|e| e.path.as_str()).collect::<Vec<_>>();
    assert_eq!(paths,["Child","Child/existing.txt"]);
    let mut bytes = Vec::new();
    node.open_payload(body.parse().unwrap()).unwrap().read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes,b"unchanged nested original");
    assert_eq!(index.reopen_node(&engine,fixture.source).unwrap().manifest().entries().len(),5);
    drop(node); drop(index);
    let count = engine.state().events.len();
    let rename = crate::NamespaceOperation::Rename {
        source:LookupPath::parse("Folder").unwrap(),destination:PortablePath::parse("Moved").unwrap()
    };
    assert!(engine.file_namespace(&repository.to_string(),rename,"admin",OperationId::generate().unwrap()).is_err());
    assert_eq!(engine.state().events.len(),count);
    // An unrelated ordinary change on the very same disk must remain writable.
    engine.file_namespace(&repository.to_string(),crate::NamespaceOperation::Mkdir {
        path:PortablePath::parse("Unrelated").unwrap(),
    },"admin",OperationId::generate().unwrap()).unwrap();
    drop(engine);
    let mut engine = Engine::open(&fixture.system).unwrap();
    let media = fixture.media();
    assert_eq!(engine.initialize_directory_snapshot(&request,&actor).unwrap(),child);
    assert_eq!(fixture.media(),media);
    assert!(engine.snapshot_direct_child_context(repository,&LookupPath::parse("Folder/Child").unwrap()).is_err());
}

#[test]
fn foreign_child_marker_and_same_named_directory_impostor_never_become_a_child_registration() {
    let (fixture, mut engine) = Fixture::with_nested_directory(true);
    let repository = fixture.targets[0].repository;
    let child = LookupPath::parse("Folder").unwrap();
    let context = engine.snapshot_direct_child_context(repository,&child).unwrap();
    let request = InitializeSnapshot { host:context.host,source:context.source,operation_id:OperationId::generate().unwrap() };
    let parent = fixture.snapshot_path().parent().unwrap().to_owned();
    let original = parent.join("Folder");
    let parked = fixture.temporary.path().join("parked");
    fs::rename(&original,&parked).unwrap(); fs::create_dir(&original).unwrap();
    fs::set_permissions(&original,fs::Permissions::from_mode(0o700)).unwrap();
    let before = engine.state().events.len();
    assert!(engine.initialize_directory_snapshot(&request,&"admin".parse().unwrap()).is_err());
    assert!(!engine.state().operations.contains_key(&request.operation_id));
    assert_eq!(engine.state().events.len(),before);
    fs::remove_dir(&original).unwrap(); fs::rename(&parked,&original).unwrap();
    let foreign = original.join(layout::SNAPSHOT_REPOSITORY.as_path());
    fs::create_dir(&foreign).unwrap();
    fs::write(foreign.join("owner"),b"foreign").unwrap();
    assert!(engine.initialize_directory_snapshot(&request,&"admin".parse().unwrap()).is_err());
    assert!(!engine.state().operations.contains_key(&request.operation_id));
    assert_eq!(fs::read(foreign.join("owner")).unwrap(),b"foreign");
}

#[test]
fn distinct_child_mount_uses_outer_parent_claim_and_yields_only_the_selected_current_file_branch() {
    use crate::snapshot_worktree::selection::{Pick, within::HeldMountedFiles};
    use name_policy::{LookupName, Name};
    use storage_contract::snapshot_worktree::{CreateWorktree, creation::{AllocateWorktree,WorktreeNaming}};
    let (fixture,mut engine) = Fixture::with_nested_directory(true);
    let repository = fixture.targets[0].repository;
    let actor = "admin".parse().unwrap();
    let root = engine.snapshot_root_context(repository).unwrap();
    let host = engine.snapshot_direct_child_context(repository,&LookupPath::parse("Folder").unwrap()).unwrap();
    let inner = engine.initialize_directory_snapshot(&InitializeSnapshot { host:host.host,source:host.source,
        operation_id:OperationId::generate().unwrap() },&actor).unwrap();
    let outer_name = CreateWorktree {source:fixture.source,parent_directory_id:root.host.directory_id,
        expected_root_generation:root.host.expected_root_generation,name:Name::parse("worktree-main-001").unwrap(),
        operation_id:OperationId::generate().unwrap()};
    let same = engine.create_directory_worktree(&outer_name,&actor).unwrap();
    for (name,parent) in [("Folder",root.host.directory_id),("WORKTREE-main-001",root.host.directory_id),
        ("Unused",host.host.directory_id)] {
        let request = CreateWorktree { source:inner,parent_directory_id:parent,name:Name::parse(name).unwrap(),
            operation_id:OperationId::generate().unwrap(),..outer_name.clone() };
        assert!(engine.create_directory_worktree(&request,&actor).is_err());
        assert!(!engine.state().operations.contains_key(&request.operation_id));
    }
    let auto = AllocateWorktree {source:inner,parent_directory_id:root.host.directory_id,
        expected_root_generation:root.host.expected_root_generation,operation_id:OperationId::generate().unwrap(),
        naming:WorktreeNaming::Automatic};
    let target = engine.allocate_directory_worktree(&auto,&actor).unwrap();
    let status = engine.directory_worktree_mount(repository,&LookupName::parse("WORKTREE-main-002").unwrap()).unwrap().unwrap();
    assert_eq!(status.target,target); assert_eq!(status.original,inner);
    assert_ne!(inner.repository_id,fixture.source.repository_id);
    assert_eq!(status.parent_directory_id,root.host.directory_id);
    let source = PortablePath::parse("Child/existing.txt").unwrap();
    let pick = Pick {worktree:target,visible:status.visible,revision:status.revision,path:&source};
    let prior = fixture.media();
    let mut selected = HeldMountedFiles::capture(&engine,fixture.source,&[pick]).unwrap();
    let (outer,_,outer_node) = selected.outer();
    assert_eq!(outer,fixture.source);
    let (path,row) = selected.occurrences().next().unwrap();
    assert_eq!(path.as_str(),"worktree-main-002/Child/existing.txt"); assert_eq!(row.source,inner);
    assert_ne!(row.source_node,outer_node); assert_eq!(row.host.directories.len(),1);
    assert_eq!(row.host.directories[0].directory.directory_id,host.host.directory_id);
    let original = engine.locate_current_object(&repository.to_string(),&LookupPath::parse("Folder/Child/existing.txt").unwrap()).unwrap();
    assert_eq!(row.selected.file.to_string(),original.entry().file_id);
    assert_eq!(row.head.version.to_string(),original.entry().version_id);
    let mut bytes = Vec::new(); selected.bodies().next().unwrap().1.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes,b"unchanged nested original");
    selected.require_current(&engine).unwrap();
    let projection = selected.project_new_mounts(&engine,storage_contract::working_tree::SnapshotNodeId::generate().unwrap(),
        OperationId::generate().unwrap()).unwrap();
    let manifest = projection.manifest();
    // The same actual FILE/Version occurs at its original path and this
    // independent mounted path; only one physical FULL is charged for it.
    let file_at = |path:&str| manifest.entries().iter().find(|entry|entry.path.as_str()==path).unwrap().content;
    let crate::working_tree::snapshot::SnapshotEntryContent::File {material,source:stable} = file_at("worktree-main-002/Child/existing.txt") else {panic!("expected selected file")};
    let src = projection.source().occurrences().next().unwrap().1;
    assert_eq!(stable,crate::working_tree::snapshot::SnapshotFileSource::Container {branch:src.selected.branch,node:src.head.node});
    assert_eq!(file_at("Folder/Child/existing.txt"),file_at("worktree-main-002/Child/existing.txt"));
    assert!(!manifest.entries().iter().any(|row|row.path.as_str().starts_with("worktree-main-001")));
    assert_eq!(manifest.entries().len(),8);
    assert_eq!(projection.independent_bytes(),manifest.encoded_len() as u64+manifest.files().values().map(|file|file.size).sum::<u64>());
    let mut full=Vec::new(); projection.open_body(&material).unwrap().read_to_end(&mut full).unwrap();
    assert_eq!(full,bytes);
    let own = engine.directory_worktree_status(same).unwrap();
    let own_path = fixture.targets[0].path.clone();
    assert!(HeldMountedFiles::capture(&engine,fixture.source,&[Pick { worktree:same,
        visible:own.visible,revision:own.revision,path:&own_path }]).is_err());
    assert_eq!(fixture.media(),prior);
    drop(projection); drop(engine);
    let mut engine = Engine::open(&fixture.system).unwrap();
    let media = fixture.media();
    assert_eq!(engine.allocate_directory_worktree(&auto,&actor).unwrap(),target);
    assert_eq!(engine.directory_worktree_mount(repository,&LookupName::parse("worktree-main-002").unwrap()).unwrap().unwrap(),status);
    assert_eq!(fixture.media(),media);
}
