use super::*;
use std::os::unix::{fs::FileExt, fs::MetadataExt};
#[test]
fn anonymous_program_copy_is_exact_and_cannot_be_rewritten_or_resized() {
    let expected = b"bounded test bytes, never executed";
    let file = sealed(expected).unwrap();
    let mut bytes = vec![0u8; expected.len()];
    file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(bytes, expected);
    assert_eq!(file.metadata().unwrap().mode() & 0o7777, 0o555);
    let seals = rustix::fs::fcntl_get_seals(&file).unwrap();
    assert!(
        seals.contains(SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL)
    );
    assert!(file.write_all_at(b"replacement", 0).is_err());
    assert!(file.set_len(0).is_err());
}
