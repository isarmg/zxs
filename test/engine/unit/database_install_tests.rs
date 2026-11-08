use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn empty_or_missing_retention_cannot_admit_an_oversized_batch() {
    let temporary = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let root = DiskRoot::open(temporary.path()).unwrap();
    let history = root.path().join("history");
    root.private_dir(&history).unwrap();
    for populated in [false, true] {
        if populated {
            root.private_dir(&history.join(".zxs-layout/database-installs"))
                .unwrap();
        }
        for (bytes, entries) in [(MAX_RETAINED_BYTES + 1, 0), (0, MAX_RETAINED_ENTRIES + 1)] {
            assert_eq!(
                check_budget(&root, &history, bytes, entries)
                    .unwrap_err()
                    .code(),
                ErrorCode::ResourceLimit
            );
        }
    }
}
