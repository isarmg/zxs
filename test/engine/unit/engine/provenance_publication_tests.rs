use super::*;
use crate::disk::{DiskSeal, version_dir};
use crate::engine::history_window::HistoryCancellationCheckpoint;
use crate::journal::{
    verify_local_publication_at, verify_tombstone_content_at, version_publication_fixture::Fixture,
};
use crate::{HistoryWindowCancellation, canonical_json};
use std::fs;

#[test]
fn local_proof_damage_does_not_revoke_the_original_writer_epoch() {
    let fixture = Fixture::new();
    let engine = &fixture.engine;
    let disk = &engine.config.disks[0];
    let handle = engine.disk_resources.get(disk).unwrap();
    let context = engine
        .metadata_integrity
        .read_context(&engine.config, &handle.seal);
    let epoch = engine.metadata_integrity.capture();
    for name in ["publication-proof.json", "commit-proof.json"] {
        let path = fixture.directory.join(name);
        let original = fs::read(&path).unwrap();
        fs::write(&path, b"broken proof").unwrap();
        let error = verify_local_publication_at(
            &handle.root,
            &fixture.directory,
            &fixture.version,
            &mut |_| Ok(()),
            Some(&context),
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidJson);
        assert!(std::error::Error::source(&error).is_some());
        assert!(epoch.matches(&engine.metadata_integrity));
        fs::write(path, original).unwrap();
    }
    verify_local_publication_at(
        &handle.root,
        &fixture.directory,
        &fixture.version,
        &mut |_| Ok(()),
        Some(&context),
    )
    .unwrap();
    assert!(epoch.matches(&engine.metadata_integrity));
    let mut changed = fixture.version.clone();
    changed.content_hash = "0".repeat(64).parse().unwrap();
    assert_eq!(
        verify_local_publication_at(
            &handle.root,
            &fixture.directory,
            &changed,
            &mut |_| Ok(()),
            Some(&context),
        )
        .unwrap_err()
        .code(),
        ErrorCode::PublicationInvalid
    );
    assert!(epoch.matches(&engine.metadata_integrity));
}

#[test]
fn shared_seal_or_membership_damage_revokes_epoch_and_fallback_cannot_revive_it() {
    let fixture = Fixture::new();
    let engine = &fixture.engine;
    let first = &engine.config.disks[0];
    let first_handle = engine.disk_resources.get(first).unwrap();
    let first_context = engine
        .metadata_integrity
        .read_context(&engine.config, &first_handle.seal);
    let second = &engine.config.disks[1];
    let second_handle = engine.disk_resources.get(second).unwrap();
    let second_context = engine
        .metadata_integrity
        .read_context(&engine.config, &second_handle.seal);
    let second_directory = version_dir(
        second,
        &fixture.version.history_tree_id,
        &fixture.version.version_id,
    );
    for (name, code) in [
        ("disk.json", ErrorCode::DiskIdentityMismatch),
        ("metadata/pool-membership/config.json", ErrorCode::InvalidJson),
    ] {
        let epoch = engine.metadata_integrity.capture();
        let path = first.root.join(name);
        let original = fs::read(&path).unwrap();
        fs::write(&path, b"broken shared authority").unwrap();
        let error = verify_local_publication_at(
            &first_handle.root,
            &fixture.directory,
            &fixture.version,
            &mut |_| Ok(()),
            Some(&first_context),
        )
        .unwrap_err();
        assert_eq!(error.code(), code);
        assert!(std::error::Error::source(&error).is_some());
        assert!(!epoch.matches(&engine.metadata_integrity));
        verify_local_publication_at(
            &second_handle.root,
            &second_directory,
            &fixture.version,
            &mut |_| Ok(()),
            Some(&second_context),
        )
        .unwrap();
        assert!(!epoch.matches(&engine.metadata_integrity));
        fs::write(path, original).unwrap();
        verify_local_publication_at(
            &first_handle.root,
            &fixture.directory,
            &fixture.version,
            &mut |_| Ok(()),
            Some(&first_context),
        )
        .unwrap();
        assert!(!epoch.matches(&engine.metadata_integrity));
    }

    // A valid saved configuration is still not this reader's admitted authority.
    let mut different = engine.config().clone();
    different.pool_id = crate::new_id("pool").unwrap();
    crate::engine::ValidatedConfig::try_from(different.clone()).unwrap();
    let path = first.root.join("metadata/pool-membership/config.json");
    let original = fs::read(&path).unwrap();
    let changed = canonical_json(&different).unwrap();
    fs::write(&path, &changed).unwrap();
    let epoch = engine.metadata_integrity.capture();
    assert_eq!(
        verify_local_publication_at(
            &first_handle.root,
            &fixture.directory,
            &fixture.version,
            &mut |_| Ok(()),
            Some(&first_context),
        )
        .unwrap_err()
        .code(),
        ErrorCode::PublicationInvalid
    );
    assert!(!epoch.matches(&engine.metadata_integrity));
    assert_eq!(fs::read(&path).unwrap(), changed);
    fs::write(path, original).unwrap();
}

#[test]
fn local_publication_checks_full_current_disk_identity_not_only_receipt_ids() {
    let fixture = Fixture::new();
    let engine = &fixture.engine;
    let disk = &engine.config.disks[0];
    let handle = engine.disk_resources.get(disk).unwrap();
    let context = engine
        .metadata_integrity
        .read_context(&engine.config, &handle.seal);
    let path = disk.root.join("disk.json");
    let original = fs::read(&path).unwrap();
    for field in ["device", "inode", "binding_generation", "simulation"] {
        let epoch = engine.metadata_integrity.capture();
        let mut changed: DiskSeal = crate::disk::decode_seal(&original).unwrap();
        match field {
            "device" => changed.device ^= 1,
            "inode" => changed.inode ^= 1,
            "binding_generation" => changed.binding_generation ^= 1,
            "simulation" => changed.simulation = !changed.simulation,
            _ => panic!("fixture field is exhaustive"),
        }
        let bytes = canonical_json(&changed).unwrap();
        fs::write(&path, &bytes).unwrap();
        let error = verify_local_publication_at(
            &handle.root,
            &fixture.directory,
            &fixture.version,
            &mut |_| Ok(()),
            Some(&context),
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::DiskIdentityMismatch, "{field}");
        assert!(!epoch.matches(&engine.metadata_integrity), "{field}");
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
    fs::write(path, original).unwrap();
}

#[test]
fn individual_body_damage_and_local_tombstone_do_not_poison_shared_membership() {
    let fixture = Fixture::new();
    let engine = &fixture.engine;
    let disk = &engine.config.disks[0];
    let handle = engine.disk_resources.get(disk).unwrap();
    let context = engine
        .metadata_integrity
        .read_context(&engine.config, &handle.seal);
    let epoch = engine.metadata_integrity.capture();
    let body = crate::container_storage::content_path_at(
        &handle.root,
        &fixture.directory,
        &fixture.version,
        &mut |_| Ok(()),
    )
    .unwrap();
    let original_permissions = fs::metadata(&body).unwrap().permissions();
    let mut damaged = fs::read(&body).unwrap();
    damaged[0] ^= 1;
    fs::set_permissions(&body, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    fs::write(&body, &damaged).unwrap();
    fs::set_permissions(&body, original_permissions).unwrap();
    assert_eq!(fs::read(&body).unwrap(), damaged);
    verify_local_publication_at(
        &handle.root,
        &fixture.directory,
        &fixture.version,
        &mut |_| Ok(()),
        Some(&context),
    )
    .unwrap();
    assert!(
        handle
            .verify_payload(&fixture.directory, &fixture.version)
            .is_err()
    );
    assert!(epoch.matches(&engine.metadata_integrity));
    fs::write(fixture.directory.join("purged.json"), b"not a valid tombstone").unwrap();
    assert_eq!(
        verify_local_publication_at(
            &handle.root,
            &fixture.directory,
            &fixture.version,
            &mut |_| Ok(()),
            Some(&context),
        )
        .unwrap_err()
        .code(),
        ErrorCode::ContentPurged
    );
    assert!(epoch.matches(&engine.metadata_integrity));
}

fn original_read_bytes(fixture: &Fixture) -> usize {
    [
        fixture.directory.join("publication-proof.json"),
        fixture.directory.join("commit-proof.json"),
        fixture.engine.config.disks[0].root.join("disk.json"),
        fixture.engine.config.disks[0]
            .root
            .join("metadata/pool-membership/config.json"),
    ]
    .iter()
    .map(|path| fs::read(path).unwrap().len())
    .sum()
}

#[test]
fn tombstone_second_identity_read_retains_shared_fault_scope_and_byte_accounting() {
    let mut fixture = Fixture::new();
    let tombstone = fixture.purge();
    let engine = &fixture.engine;
    let disk = &engine.config.disks[0];
    let handle = engine.disk_resources.get(disk).unwrap();
    let context = engine
        .metadata_integrity
        .read_context(&engine.config, &handle.seal);
    let first_pass = original_read_bytes(&fixture);
    let shared_bytes = fs::read(disk.root.join("disk.json")).unwrap().len()
        + fs::read(disk.root.join("metadata/pool-membership/config.json")).unwrap().len();
    let mut observed = 0;
    verify_tombstone_content_at(
        &handle.root,
        &fixture.directory,
        &fixture.version,
        &tombstone,
        &mut |bytes| {
            observed += bytes;
            Ok(())
        },
        Some(&context),
    )
    .unwrap();
    assert_eq!(observed, first_pass + shared_bytes);

    let epoch = engine.metadata_integrity.capture();
    let mut observed = 0;
    let mut changed = false;
    let error = verify_tombstone_content_at(
        &handle.root,
        &fixture.directory,
        &fixture.version,
        &tombstone,
        &mut |bytes| {
            observed += bytes;
            if observed == first_pass && !changed {
                fs::write(disk.root.join("disk.json"), b"changed before tombstone seal").unwrap();
                changed = true;
            }
            Ok(())
        },
        Some(&context),
    )
    .unwrap_err();
    assert!(changed);
    assert_eq!(error.code(), ErrorCode::DiskIdentityMismatch);
    assert!(std::error::Error::source(&error).is_some());
    assert!(!epoch.matches(&engine.metadata_integrity));
}

#[test]
fn publication_cancellation_at_every_observed_read_preserves_writer_epoch() {
    let fixture = Fixture::new();
    let engine = &fixture.engine;
    let disk = &engine.config.disks[0];
    let handle = engine.disk_resources.get(disk).unwrap();
    let context = engine
        .metadata_integrity
        .read_context(&engine.config, &handle.seal);
    let mut observations = Vec::new();
    verify_local_publication_at(
        &handle.root,
        &fixture.directory,
        &fixture.version,
        &mut |bytes| {
            observations.push(bytes);
            Ok(())
        },
        Some(&context),
    )
    .unwrap();
    assert_eq!(
        observations.iter().sum::<usize>(),
        original_read_bytes(&fixture)
    );
    for stop in 0..observations.len() {
        let epoch = engine.metadata_integrity.capture();
        let cancellation = HistoryWindowCancellation::new();
        let mut count = 0;
        let error = verify_local_publication_at(
            &handle.root,
            &fixture.directory,
            &fixture.version,
            &mut |_| {
                if count == stop {
                    cancellation.cancel();
                }
                count += 1;
                cancellation.checkpoint()
            },
            Some(&context),
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::OperationAborted, "{stop}");
        assert_eq!(count, stop + 1);
        assert!(epoch.matches(&engine.metadata_integrity), "{stop}");
    }
}
