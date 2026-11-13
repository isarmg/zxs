fn recover(root: &Path) {
    use std::os::unix::fs::MetadataExt;
    use storage_contract::working_tree::LocalEntityIdentity;
    let system = root.join("system");
    fs::rename(&system, root.join("retained-original-system")).unwrap();
    private(&system);
    let admission = Engine::admit_storage_recovery(vec![
        PrivateDirectory::open_existing(&root.join("disk")).unwrap(),
    ]).unwrap();
    let operation = OperationId::generate().unwrap();
    let principal: storage_contract::Principal = "recovery-owner".parse().unwrap();
    let parent = root.metadata().unwrap();
    let child = system.metadata().unwrap();
    let control = storage_engine::StorageRecoveryControl::new(
        storage_engine::StorageControlBinding {
            operation,
            control: LocalEntityIdentity::new(parent.dev(), parent.ino()),
            system: LocalEntityIdentity::new(child.dev(), child.ino()),
        },
        admission.evidence_commitment(), principal.clone(),
        vec![storage_engine::RecoveryDiskSelection::parse("one").unwrap()],
        storage_engine::StorageRecoveryPredecessor::NewControl,
    );
    let engine = Engine::restore_storage_held(
        PrivateDirectory::open_existing(&system).unwrap(), operation, principal,
        admission, control.commitment().unwrap(), control.clone(), None,
    ).unwrap();
    engine.require_recovery_control(Some(&control)).unwrap();
    drop(engine);
    assert_eq!(open(root), 0);
    println!("completed actual v3 recovery carrier and verified cold content");
}
