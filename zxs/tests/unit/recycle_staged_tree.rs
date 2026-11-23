use super::*;
use crate::{container_store::stage::FlatStage, test_support::PrivateTemp};
use std::{ffi::CString, io::Write};

#[test]
fn interrupted_copy_cleans_its_unfinished_and_sealed_entries() {
    let temporary = PrivateTemp::under(&std::env::temp_dir());
    let parent = HeldDir::trusted_storage(&temporary.0).unwrap();
    let mut owner = FlatStage::new(&parent, CString::new("incoming-test").unwrap()).unwrap();
    owner.write(c"source-proof", b"proof").unwrap();
    let stage = owner.directory.path.clone();
    let original_tree = stage.join("container");
    {
        let mut tree = CopyTree::new(&owner.directory).unwrap();
        tree.reserve_entry().unwrap();
        let mut finished =
            FlatStage::new(tree.directory(), CString::new("finished").unwrap()).unwrap();
        finished.write(c"payload", b"complete").unwrap();
        finished.write(c"receipt", b"receipt").unwrap();
        let sealed = finished.seal().unwrap();
        tree.record_entry(sealed);

        tree.reserve_entry().unwrap();
        let mut active = FlatStage::new(tree.directory(), CString::new("active").unwrap()).unwrap();
        active
            .create(c"payload")
            .unwrap()
            .write_all(b"partial")
            .unwrap();
    }
    assert!(!original_tree.exists());
    drop(owner);
    assert!(!stage.exists());
}
