use super::*;
use crate::{Config, Engine};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use storage_contract::{FileId, Mode, OperationId};

fn origin() -> PackOrigin { PackOrigin { file: FileId::from_random([3; 16]), operation: OperationId::from_random([4; 16]) } }

fn setup() -> (tempfile::TempDir, Engine, DiskRoot) {
    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("system");
    let disk = temp.path().join("primary");
    for path in [&system, &disk] {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let config = Config::independent_roots(Mode::Development,
        [(name_policy::Name::parse("Primary").unwrap(), disk)],
    ).unwrap();
    let engine = Engine::initialize(&system, config).unwrap();
    let handle = engine.disk_resources.get(&engine.config.disks[0]).unwrap();
    let path = handle.root.path().join("sample-pack-container");
    handle.root.private_dir(&path).unwrap();
    let root = handle.root.private_child(&path).unwrap();
    (temp, engine, root)
}

#[test]
fn frame_fragments_share_one_inode_and_a_cold_range_checks_only_its_selected_chunks() {
    let (_temp, _engine, root) = setup();
    let id = PackId::generate().unwrap();
    let mut upload = PendingPack::create(&root, id, origin()).unwrap();
    let frame = vec![0x31; 64 * 1024];
    for n in 0..32 { assert_eq!(upload.append(&root, &frame).unwrap(), n * frame.len() as u64); }
    let tail = b"end";
    upload.append(&root, tail).unwrap();
    let mut across = [0; 5];
    let where_ = 2 * VERIFY_CHUNK - 2;
    upload.read_at(&root, &mut across, where_).unwrap();
    assert_eq!(&across, b"11end");
    let held = upload.seal(&root).unwrap();
    let pack = held.evidence();
    assert_eq!(pack.data_bytes(), 2 * VERIFY_CHUNK + 3);
    assert_eq!(pack.physical_bytes(), pack.data_bytes() + 4 * 32);
    assert_eq!(root.entries(root.path()).unwrap().count(), 1);
    assert!(PendingPack::create(&root, id, origin()).is_err());
    drop(held);
    let path = root.path().join(working_pack(id).as_path());
    // Corrupt an unrelated data chunk after dropping the live capture. A cold
    // range may serve intact covered chunks, but never serve the damaged one.
    fs::OpenOptions::new().write(true).open(&path).unwrap().write_all_at(b"x", 0).unwrap();
    let cold = HeldPack::capture(&root, pack).unwrap();
    cold.read_at(&root, &mut across, where_).unwrap();
    assert_eq!(&across, b"11end");
    assert!(cold.read_at(&root, &mut across, 0).is_err());
    assert!(cold.raw_blake3(&root, || Ok(())).is_err(),
        "complete raw evidence cannot skip a corrupt unrequested chunk");
    drop(cold);
    let trailer = pack.data_bytes();
    let file = fs::OpenOptions::new().read(true).write(true).open(&path).unwrap();
    let mut byte = [0];
    file.read_exact_at(&mut byte, trailer).unwrap();
    file.write_all_at(&[byte[0] ^ 0xff], trailer).unwrap();
    assert!(HeldPack::capture(&root, pack).is_err());
}

#[test]
fn partial_and_replaced_pack_names_never_become_accepted_material() {
    let (_temp, _engine, root) = setup();
    let id = PackId::generate().unwrap();
    let mut upload = PendingPack::create(&root, id, origin()).unwrap();
    upload.append(&root, b"original").unwrap();
    let path = root.path().join(working_pack(id).as_path());
    let parked = root.path().join("old");
    fs::rename(&path, &parked).unwrap();
    fs::write(&path, b"original").unwrap();
    assert!(upload.seal(&root).is_err());
    assert_eq!(fs::read(&parked).unwrap(), b"original");
    assert_eq!(fs::read(&path).unwrap(), b"original");
    assert!(PendingPack::create(&root, id, origin()).is_err());
}
