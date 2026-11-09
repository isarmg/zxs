//! Object-local rescue artifacts, not active installation or system authority.
use super::*;
use std::os::unix::fs::PermissionsExt;
use storage_engine::{
    ObjectDatabaseScope, inspect_object_database_material, reconstruct_object_database_material,
};

#[test]
fn directory_archive_reconstructs_after_source_system_and_sqlite_are_unavailable() {
    let mut fixture = Fixture::new();
    fixture.save("outside/ignored.txt", b"not local");
    fixture.save("box/file.txt", b"local");
    fixture.initialize("");
    let initialized = fixture.initialize("box");
    fixture.checkpoint("box");
    fixture.create_branch("box", "topic");
    fixture
        .engine
        .reconcile_directory_layout(&fixture.repository.repository_id)
        .unwrap();
    let scope = ObjectDatabaseScope::Directory {
        repository_id: fixture.repository.repository_id.clone(),
        directory_id: initialized.directory_id,
    };
    let survivor = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let bytes = fs::read(fixture.root().join("box/.zxs-directory.sqlite")).unwrap();
    fs::write(survivor.path().join(".zxs-directory.sqlite"), bytes).unwrap();
    drop(fixture);
    let material = inspect_object_database_material(
        fs::File::open(survivor.path()).unwrap(),
        survivor.path().into(),
        &scope,
    )
    .unwrap();
    let archive = material.archive_bytes().unwrap();
    fs::remove_file(survivor.path().join(".zxs-directory.sqlite")).unwrap();
    let rebuilt = reconstruct_object_database_material(&archive, &scope).unwrap();
    assert_eq!(rebuilt.material(), &material);
    assert!(material.records().iter().any(|row| row.kind() == "commit"));
    assert!(
        material
            .records()
            .iter()
            .all(|row| !row.body_json().contains("outside/ignored"))
    );
    assert!(!survivor.path().join(".zxs-directory.sqlite").exists());
}

#[test]
fn directory_archive_rejects_rehashed_exterior_tree_path_and_missing_local_head() {
    let mut fixture = Fixture::new();
    fixture.save("box/file.txt", b"local");
    fixture.initialize("");
    let initialized = fixture.initialize("box");
    fixture.checkpoint("box");
    fixture
        .engine
        .reconcile_directory_layout(&fixture.repository.repository_id)
        .unwrap();
    let scope = ObjectDatabaseScope::Directory {
        repository_id: fixture.repository.repository_id.clone(),
        directory_id: initialized.directory_id,
    };
    let parent = fixture.root().join("box");
    let material =
        inspect_object_database_material(fs::File::open(&parent).unwrap(), parent, &scope).unwrap();
    for missing_head in [false, true] {
        let mut archive: json_codec::Value =
            json_codec::from_slice(&material.archive_bytes().unwrap()).unwrap();
        let rows = archive["material"]["records"].as_array_mut().unwrap();
        if missing_head {
            rows.retain(|row| row["kind"] != "tree");
        } else {
            let row = rows.iter_mut().find(|row| row["kind"] == "tree").unwrap();
            let mut body: json_codec::Value =
                json_codec::from_str(row["body"].as_str().unwrap()).unwrap();
            body["path"] = json_codec::json!("outside/root");
            row["body"] = json_codec::json!(
                String::from_utf8(storage_engine::canonical_json(&body).unwrap()).unwrap()
            );
        }
        archive["material"]["records_hash"] = json_codec::json!(
            storage_contract::object_hash(
                "directory-local-records",
                &archive["material"]["records"]
            )
            .unwrap()
        );
        assert!(
            reconstruct_object_database_material(
                &storage_engine::canonical_json(&archive).unwrap(),
                &scope
            )
            .is_err()
        );
    }
}

#[test]
fn directory_rescue_readback_cannot_substitute_a_newer_valid_local_branch_set() {
    let mut fixture = Fixture::new();
    fixture.save("box/file.txt", b"local");
    fixture.initialize("");
    let initialized = fixture.initialize("box");
    fixture.checkpoint("box");
    fixture
        .engine
        .reconcile_directory_layout(&fixture.repository.repository_id)
        .unwrap();
    let scope = ObjectDatabaseScope::Directory {
        repository_id: fixture.repository.repository_id.clone(),
        directory_id: initialized.directory_id,
    };
    let parent = fixture.root().join("box");
    let material =
        inspect_object_database_material(fs::File::open(&parent).unwrap(), parent.clone(), &scope)
            .unwrap();
    let rebuilt =
        reconstruct_object_database_material(&material.archive_bytes().unwrap(), &scope).unwrap();
    rebuilt
        .verify_database_bytes(&fs::read(parent.join(".zxs-directory.sqlite")).unwrap())
        .unwrap();
    fixture.create_branch("box", "later");
    fixture
        .engine
        .reconcile_directory_layout(&fixture.repository.repository_id)
        .unwrap();
    let later = fs::read(parent.join(".zxs-directory.sqlite")).unwrap();
    assert!(
        inspect_object_database_material(fs::File::open(&parent).unwrap(), parent, &scope).is_ok()
    );
    assert!(rebuilt.verify_database_bytes(&later).is_err());
}
