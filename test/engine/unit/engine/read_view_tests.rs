use crate::{
    Config, Engine, MembershipExitAction, OperationId, ReadView, RepositoryName, TerminalManagement,
};

fn counters(read: &ReadView<'_>) -> (u64, u64) {
    (read.sequence(), read.topology_generation())
}

#[test]
fn published_counters_follow_repository_creation_replay_and_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("system");
    let config = Config::development([temp.path().join("primary")]).unwrap();
    let mut engine = Engine::initialize(&system, config).unwrap();
    assert_eq!(counters(&engine.read()), (0, 1));

    let name = RepositoryName::parse("counter publication").unwrap();
    let operation = OperationId::generate().unwrap();
    let repository = engine
        .create_repository(&name, "P01", 1, "admin", operation)
        .unwrap();
    let read = engine.read();
    assert_eq!(counters(&read), (1, 1));
    assert_eq!(counters(&engine.read()), counters(&read));

    let replay = engine
        .create_repository(&name, "P01", 1, "admin", operation)
        .unwrap();
    assert_eq!(replay, repository);
    assert_eq!(counters(&engine.read()), (1, 1));
    drop(engine);

    let reopened = Engine::open(&system).unwrap();
    assert_eq!(counters(&reopened.read()), (1, 1));
}

#[test]
fn terminal_counters_follow_the_terminal_publication_and_cold_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("system");
    let disk = temp.path().join("primary");
    let config = Config::development([disk.clone()]).unwrap();
    let disk_id = config.disks[0].disk_id.clone();
    let mut engine = Engine::initialize(&system, config).unwrap();
    engine
        .create_repository(
            &RepositoryName::parse("retained").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let before = counters(&engine.read());
    engine
        .begin_disk_exit(
            &disk_id,
            MembershipExitAction::Deregister,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    assert_eq!(counters(&engine.read()), before);
    let mut exit = engine.prepare_terminal_disk_exit().unwrap();
    let terminal = exit.complete(&mut engine).unwrap();
    let read = terminal.read();
    // The membership fence consumes a log slot before terminal publication.
    let expected = (before.0 + 2, before.1 + 1);
    assert_eq!(counters(&read), expected);
    assert_eq!(read.sequence(), terminal.result().sequence);
    assert_eq!(counters(&terminal.read()), counters(&read));
    drop(terminal);
    drop(exit);
    drop(engine);

    std::fs::rename(&disk, temp.path().join("retired-primary")).unwrap();
    let reopened = TerminalManagement::open(&system).unwrap();
    assert_eq!(counters(&reopened.read()), expected);
}
