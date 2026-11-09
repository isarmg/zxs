//! Source-only rescue regressions. No checks were run for this slice.
use super::*;
use std::fs;

fn captured(scope: DiskDatabaseReadScope) -> (tempfile::TempDir, std::path::PathBuf, DiskRegistrationIdentity, DiskDatabaseMaterial) {
    let (temp, mut engine) = fixture();
    let repo = repository(&mut engine, "rescue", "P01");
    save(&mut engine, &repo, "before.txt");
    engine.rebuild_disk_catalog("P01").unwrap();
    engine.rebuild_disk_history("P01").unwrap();
    let root = engine.disk("P01").unwrap().root.clone();
    let identity = engine.registered_disks().find(|row| row.slot_id == "P01").unwrap().identity();
    drop(engine);
    let material = inspect_disk_database_material(fs::File::open(&root).unwrap(), root.clone(), &identity, scope).unwrap();
    (temp, root, identity, material)
}

fn rehash(mut archive: json_codec::Value) -> Vec<u8> {
    archive["records_hash"] = json_codec::json!(storage_contract::object_hash(
        "disk-database-rescue-material", &archive["material"]).unwrap());
    canonical_json(&archive).unwrap()
}

#[test]
fn standalone_rescue_rebuilds_both_families_after_original_system_and_disk_are_gone() {
    let (temp, root, identity, material) = captured(DiskDatabaseReadScope::Both);
    let expected_current = material.current().unwrap().clone();
    let expected_history = material.history().unwrap().clone();
    let expected_fold = material.history_inventory().unwrap().clone();
    let archive = material.archive_bytes().unwrap();
    drop(material);
    drop(temp);
    assert!(!root.exists());
    for source in [DiskDatabaseRebuildSource::StoredCurrent, DiskDatabaseRebuildSource::CurrentFromHistory] {
        let image = reconstruct_disk_database_material(&archive, &identity, source).unwrap();
        assert_eq!(image.source(), source);
        let expected = if source == DiskDatabaseRebuildSource::StoredCurrent { &expected_current } else { &expected_fold };
        assert_eq!(&database::read(image.database_bytes(), &identity).unwrap(), expected);
        assert_eq!(image.source_sequence(), expected.source_sequence);
        assert!(!root.exists());
    }
    let image = reconstruct_disk_database_material(&archive, &identity, DiskDatabaseRebuildSource::History).unwrap();
    assert_eq!(history::read_for_root_install(image.database_bytes(), &identity).unwrap(), expected_history);
    assert!(!root.exists());
}

#[test]
fn rescue_selection_preserves_absence_and_never_invents_a_history() {
    let (_temp, _root, identity, material) = captured(DiskDatabaseReadScope::Current);
    let archive = material.archive_bytes().unwrap();
    assert!(reconstruct_disk_database_material(&archive, &identity, DiskDatabaseRebuildSource::StoredCurrent).is_ok());
    for source in [DiskDatabaseRebuildSource::History, DiskDatabaseRebuildSource::CurrentFromHistory] {
        assert!(reconstruct_disk_database_material(&archive, &identity, source).is_err());
    }
    let (_temp, _root, identity, material) = captured(DiskDatabaseReadScope::History);
    let archive = material.archive_bytes().unwrap();
    assert!(material.current().is_none());
    assert!(reconstruct_disk_database_material(&archive, &identity, DiskDatabaseRebuildSource::StoredCurrent).is_err());
    let image = reconstruct_disk_database_material(&archive, &identity, DiskDatabaseRebuildSource::CurrentFromHistory).unwrap();
    assert_eq!(&database::read(image.database_bytes(), &identity).unwrap(), material.history_inventory().unwrap());
}

#[test]
fn explicit_binding_and_full_current_history_relation_are_required() {
    let (_temp, _root, identity, material) = captured(DiskDatabaseReadScope::Both);
    let bytes = material.archive_bytes().unwrap();
    let mut wrong = identity.clone();
    wrong.binding_generation += 1;
    assert!(reconstruct_disk_database_material(&bytes, &wrong, DiskDatabaseRebuildSource::History).is_err());
    let mut archive: json_codec::Value = decode_strict(&bytes).unwrap();
    archive["material"]["current"]["versions"][0]["owner_path"] = json_codec::json!("other.txt");
    let mismatched = rehash(archive);
    // Asking for history does not hide a contradictory retained current file.
    assert!(reconstruct_disk_database_material(&mismatched, &identity, DiskDatabaseRebuildSource::History).is_err());
}

#[test]
fn internally_rehashed_missing_history_predecessor_cannot_rebuild_a_current() {
    let (_temp, _root, identity, material) = captured(DiskDatabaseReadScope::History);
    let mut archive: json_codec::Value = decode_strict(&material.archive_bytes().unwrap()).unwrap();
    let record = archive["material"]["history"]["changes"].as_array_mut().unwrap().iter_mut()
        .find(|row| row["change"]["kind"] == "entered").unwrap();
    let after = record["change"]["after"].clone();
    let mut before = after.clone();
    before["owner_path"] = json_codec::json!("missing-predecessor.txt");
    record["change"] = json_codec::json!({"kind":"changed","before":before,"after":after});
    assert!(reconstruct_disk_database_material(&rehash(archive), &identity, DiskDatabaseRebuildSource::CurrentFromHistory).is_err());
}

#[test]
fn rescue_archive_rejects_unknown_nested_fields_and_noncanonical_bytes() {
    let (_temp, _root, identity, material) = captured(DiskDatabaseReadScope::Current);
    let bytes = material.archive_bytes().unwrap();
    let mut archive: json_codec::Value = decode_strict(&bytes).unwrap();
    archive["material"]["current"]["versions"][0]["version"]["outside_root"] = json_codec::json!("/other/disk");
    assert!(reconstruct_disk_database_material(&rehash(archive), &identity, DiskDatabaseRebuildSource::StoredCurrent).is_err());
    let mut noncanonical = bytes.clone();
    noncanonical.push(b'\n');
    assert!(reconstruct_disk_database_material(&noncanonical, &identity, DiskDatabaseRebuildSource::StoredCurrent).is_err());
}

#[test]
fn selected_sqlite_sidecars_are_preserved_and_unselected_sidecars_are_not_inspected() {
    let (_temp, root, identity, _material) = captured(DiskDatabaseReadScope::Both);
    let original = fs::read(root.join(DISK_DATABASE_NAME)).unwrap();
    let sidecar = root.join(format!("{DISK_DATABASE_NAME}-wal"));
    fs::write(&sidecar, b"unknown pending bytes").unwrap();
    for scope in [DiskDatabaseReadScope::Current, DiskDatabaseReadScope::Both] {
        assert!(inspect_disk_database_material(fs::File::open(&root).unwrap(), root.clone(), &identity, scope).is_err());
    }
    assert!(inspect_disk_database_material(fs::File::open(&root).unwrap(), root.clone(), &identity, DiskDatabaseReadScope::History).is_ok());
    assert_eq!(fs::read(sidecar).unwrap(), b"unknown pending bytes");
    assert_eq!(fs::read(root.join(DISK_DATABASE_NAME)).unwrap(), original);
}
