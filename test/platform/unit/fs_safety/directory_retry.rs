use std::{fs, os::unix::fs::MetadataExt, path::PathBuf};

pub fn check<E: std::error::Error + 'static>(mut create: impl FnMut() -> Result<(), E>) {
    let root = PathBuf::from(std::env::var_os("ZXS_DIRECTORY_ROOT").unwrap());
    let log = PathBuf::from(std::env::var_os("ZXS_DIRECTORY_LOG").unwrap());
    let stage = std::env::var("ZXS_DIRECTORY_STAGE").unwrap();
    let mode = std::env::var("ZXS_DIRECTORY_MODE").unwrap();
    let error = create().expect_err("first creation must encounter injected EIO");
    let mut source: &dyn std::error::Error = &error;
    loop {
        if let Some(error) = source.downcast_ref::<std::io::Error>() {
            assert_eq!(error.raw_os_error(), Some(5));
            break;
        }
        source = source.source().expect("EIO source must be retained");
    }
    let child = root.join("branch");
    let before = fs::metadata(&child).unwrap();
    fs::write(child.join("retained"), b"keep").unwrap();
    let first = fs::read_to_string(&log).unwrap();
    assert_eq!(
        first
            .lines()
            .filter(|line| line.starts_with("fail "))
            .count(),
        1
    );
    create().unwrap();
    let events = fs::read_to_string(&log).unwrap();
    let retry = events.strip_prefix(&first).unwrap();
    assert!(!retry.contains("fail "));
    let child_sync = format!("sync {}\n", child.display());
    let parent_sync = format!("sync {}\n", root.display());
    let child_position = retry.find(&child_sync).expect("retry child barrier");
    assert!(
        retry[child_position + child_sync.len()..].contains(&parent_sync),
        "retry must complete parent barrier after child"
    );
    let after = fs::metadata(&child).unwrap();
    assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
    assert_eq!(after.mode() & 0o7777, 0o700);
    assert_eq!(fs::read(child.join("retained")).unwrap(), b"keep");
    println!("\nZXS_DIRECTORY_RETRY_COMPLETE mode={mode} stage={stage}");
}
