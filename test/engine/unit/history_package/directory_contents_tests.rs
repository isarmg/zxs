use super::*;

pub(super) fn assert_frozen_payload_validation(manifest: &HistoryManifest, tree_id: &str) {
    let original = canonical_json(manifest).unwrap();
    let tree = &manifest.directory_trees[tree_id];
    let owner = &manifest.directory_branches[&tree.directory_branch_id];
    for field in 0..7 {
        let mut corrupted = manifest.clone();
        let tree = corrupted.directory_trees.get_mut(tree_id).unwrap();
        match field {
            0 => tree.commit_id = new_id("commit").unwrap(),
            1 => tree.path = "missing-directory".into(),
            2 => tree.entries.clear(),
            3 => tree.file_branches.clear(),
            4 => tree.file_contexts.clear(),
            5 => {
                tree.inherited_files
                    .insert(tree.entries[0].file_id.clone(), new_id("dbr").unwrap());
            }
            6 => {
                tree.directory_refs.insert(
                    new_id("entry").unwrap(),
                    DirectoryReference {
                        directory_branch_id: owner.directory_branch_id.clone(),
                        tree_id: tree.tree_id.clone(),
                    },
                );
            }
            _ => unreachable!(),
        }
        // The still-valid current branch must never supply replacement payloads
        // for a damaged immutable tree during manifest admission.
        assert_eq!(
            canonical_json(&corrupted.directory_branches).unwrap(),
            canonical_json(&manifest.directory_branches).unwrap()
        );
        let error = corrupted.validate().unwrap_err();
        if field != 1 {
            assert_eq!(
                error.code(),
                storage_contract::ErrorCode::HistoryPackageInvalid
            );
        }
    }
    assert_eq!(canonical_json(manifest).unwrap(), original);
}
