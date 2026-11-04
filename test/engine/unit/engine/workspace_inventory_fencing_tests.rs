use super::*;
use std::fs;

fn descriptors(path: &Path) -> usize {
    fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter(
            |entry| match fs::read_link(entry.as_ref().unwrap().path()) {
                Ok(target) => target == path,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => panic!("cannot inspect descriptor: {error}"),
            },
        )
        .count()
}

#[test]
fn idle_inventory_releases_target_root_and_writer_without_releasing_system_lock() {
    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("system");
    let primary = temp.path().join("primary");
    let mut engine =
        Engine::initialize(&system, Config::development([primary.clone()]).unwrap()).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("inventory fencing").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    drop(engine);
    let mut inventory = WorkspaceInventory::open(&system).unwrap();
    let index = inventory
        .locations
        .iter()
        .position(|location| matches!(location, WorkspaceLocation::Primary(_)))
        .unwrap();
    let root = inventory.locations[index].root();
    let binding = root.io_binding().unwrap().clone();
    let directory = root.path().to_owned();
    let manifest = directory
        .join(workspace.workspace_id)
        .join("workspace.json");
    let before = fs::read(&manifest).unwrap();
    inventory.current = Some((index, root.entries(&directory).unwrap()));
    assert!(descriptors(&directory) > 0);
    drop(binding.begin_quiesce().unwrap().unwrap());
    assert!(binding.status().unwrap().drained());
    assert_eq!(descriptors(&directory), 0);
    let writer = fs::File::options()
        .read(true)
        .write(true)
        .open(primary.join(".writer.lock"))
        .unwrap();
    rustix::fs::flock(
        &writer,
        rustix::fs::FlockOperation::NonBlockingLockExclusive,
    )
    .unwrap();
    let system_writer = fs::File::options()
        .read(true)
        .write(true)
        .open(system.join("writer.lock"))
        .unwrap();
    assert_eq!(
        rustix::fs::flock(
            &system_writer,
            rustix::fs::FlockOperation::NonBlockingLockExclusive
        )
        .unwrap_err(),
        rustix::io::Errno::WOULDBLOCK
    );
    assert_eq!(fs::read(manifest).unwrap(), before);
    drop(inventory);
    rustix::fs::flock(
        &system_writer,
        rustix::fs::FlockOperation::NonBlockingLockExclusive,
    )
    .unwrap();
}
