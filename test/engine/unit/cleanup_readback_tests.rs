use super::*;
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

fn roots() -> (tempfile::TempDir, DiskRoot, PathBuf, PathBuf) {
    let temp = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700)).tempdir().unwrap();
    let root = DiskRoot::open(temp.path()).unwrap();
    let installed = root.path().join("container/BODY");
    let prepared = root.path().join("version/staged/BODY");
    root.private_dir(installed.parent().unwrap()).unwrap();
    root.private_dir(prepared.parent().unwrap()).unwrap();
    (temp, root, installed, prepared)
}

fn body(path: &std::path::Path) {
    fs::write(path, b"original owned payload").unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn promotion_between_the_two_reads_cannot_be_reported_as_absent() {
    let (_temp, root, installed, prepared) = roots();
    body(&prepared);
    let mut moved = false;
    let result = observe_pair(&root, &installed, &root, &prepared, &mut || {
        if !moved {
            fs::rename(&prepared, &installed)?;
            moved = true;
        }
        Ok(())
    });
    assert!(moved);
    assert_eq!(result.unwrap_err().code(), crate::ErrorCode::ContainerLayoutInvalid);
    assert_eq!(fs::read(&installed).unwrap(), b"original owned payload");
    assert!(observe_pair(&root, &installed, &root, &prepared, &mut || Ok(())).unwrap());
}

#[test]
fn empty_replacement_parent_is_not_the_original_absent_leaf() {
    let (_temp, root, installed, prepared) = roots();
    for parent in [installed.parent().unwrap(), prepared.parent().unwrap()] {
        let mut checkpoints = 0;
        let swapped = root.path().join("detached");
        let result = observe_pair(&root, &installed, &root, &prepared, &mut || {
            checkpoints += 1;
            if checkpoints == 2 {
                fs::rename(parent, &swapped)?;
                root.private_dir(parent)?;
            }
            Ok(())
        });
        assert_eq!(result.unwrap_err().code(), crate::ErrorCode::ContainerLayoutInvalid);
        assert!(swapped.is_dir() && parent.is_dir());
        fs::remove_dir(parent).unwrap();
        fs::rename(&swapped, parent).unwrap();
    }
    assert!(!observe_pair(&root, &installed, &root, &prepared, &mut || Ok(())).unwrap());
}

#[test]
fn proof_bytes_remain_on_the_held_inode_and_replacement_is_rejected() {
    let (_temp, root, installed, _) = roots();
    body(&installed);
    let proof = BodyLeaf::open(&root, &installed).unwrap();
    let detached = root.path().join("old-proof");
    fs::rename(&installed, &detached).unwrap();
    body(&installed); // Equal bytes do not turn the new inode into the original.
    let mut cost = 0;
    let bytes = disk::read_file_bytes_observed(
        proof.file.as_ref().unwrap().try_clone().unwrap(), MAX_METADATA,
        &mut |bytes| { cost += bytes; Ok(()) },
    ).unwrap();
    assert_eq!(cost, bytes.len());
    assert_eq!(&bytes, b"original owned payload");
    assert_eq!(proof.recheck(&root).unwrap_err().code(), crate::ErrorCode::ContainerLayoutInvalid);
    assert!(detached.exists() && installed.exists());
}
