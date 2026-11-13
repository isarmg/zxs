use super::*;

#[test]
fn manual_flush_close_and_reopen_writer_keep_shared_body_without_advancing_head() {
    let (_root, shared, _backend, branch) = fixture();
    let identity = identity(&shared);
    let entry = session(Arc::clone(&shared), &branch).entries().unwrap().remove(0);
    let context = crate::ReadContainerEdit {
        file: entry.file_id.parse().unwrap(),
        repository: repository(&shared).parse().unwrap(),
        path: entry.path.clone(),
    };
    shared.backend.set_container_commit_mode(&identity, crate::SetContainerCommitMode {
        context: context.clone(), expected_generation: storage_contract::Generation::new(0),
        mode: storage_contract::container_edit::CommitMode::Manual,
        operation_id: OperationId::generate().unwrap(),
    }).unwrap();
    let head = shared.backend.branch_snapshot(&identity, &branch).unwrap().head_commit_id;
    let mut client = session(Arc::clone(&shared), &branch);
    let (opened, descriptor) = client.open("report.txt".into(), true, false, false, false, false)
        .unwrap();
    let file = File::from(descriptor);
    client.prepare_write(opened.handle, 5, 5).unwrap();
    file.write_all_at(b"draft", 0).unwrap();
    client.dirty(opened.handle).unwrap();
    let first = client.flush(opened.handle).unwrap();
    assert_eq!(first["draft_persisted"], true);
    assert!(first.get("commit_id").is_none());
    assert_eq!(shared.backend.branch_snapshot(&identity, &branch).unwrap().head_commit_id, head);
    assert_eq!(client.entries().unwrap()[0].version_id, entry.version_id);
    client.close(opened.handle, false).unwrap();
    drop(file);
    let current = shared.backend.current_container_content(&identity, context.clone()).unwrap();
    assert!(current.edit.draft().is_some());
    assert_eq!(current.content.size, 5);
    let mut original = shared.backend.content(&identity, &entry.version_id).unwrap();
    let mut bytes = Vec::new();
    original.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"old");
    let (opened, descriptor) = client.open("report.txt".into(), true, false, false, false, true)
        .unwrap();
    let file = File::from(descriptor);
    let mut bytes = [0u8; 5];
    file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"draft");
    client.prepare_write(opened.handle, 5, 5).unwrap();
    file.write_all_at(b"again", 0).unwrap();
    let second = client.dirty(opened.handle).unwrap();
    assert_eq!(second["draft_persisted"], true);
    assert_ne!(second["operation_id"], first["operation_id"]);
    client.close(opened.handle, false).unwrap();
    assert_eq!(shared.backend.branch_snapshot(&identity, &branch).unwrap().head_commit_id, head);
    assert_eq!(client.entries().unwrap()[0].version_id, entry.version_id);
    let stat = client.stat("report.txt").unwrap();
    assert_eq!(stat.size, 5);
    let (opened, descriptor) = client.open("report.txt".into(), false, false, false, false, false)
        .unwrap();
    assert!(opened.version_id.is_none());
    assert_eq!(opened.size, 5);
    drop(descriptor);
    assert_eq!(client.read(opened.handle, 1, 3).unwrap().as_ref(), b"gai");
    client.close(opened.handle, false).unwrap();
}
