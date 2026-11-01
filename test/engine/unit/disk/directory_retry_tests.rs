use super::*;

#[path = "../../../platform/unit/fs_safety/directory_retry.rs"]
mod directory_retry;

#[test]
#[ignore = "requires directory-retry.sh isolated LD_PRELOAD children"]
fn directory_creation_retry_barriers() {
    let root = PathBuf::from(std::env::var_os("ZXS_DIRECTORY_ROOT").unwrap());
    match std::env::var("ZXS_DIRECTORY_MODE").unwrap().as_str() {
        "engine" => {
            let disk = DiskRoot::open(&root).unwrap();
            let object = root.join("branch/objects/state-page/ab/object");
            directory_retry::check(|| disk.write_atomic(&object, b"durable object", false));
            assert_eq!(disk.read_bytes(&object, 100).unwrap(), b"durable object");
        }
        "engine-path" => {
            let path = root.join("branch/nested/export");
            directory_retry::check(|| private_dir(&path));
            assert!(path.is_dir());
        }
        _ => panic!("unknown directory retry mode"),
    }
}
