use super::*;
use std::os::unix::fs::symlink;

struct Fixture {
    _directory: tempfile::TempDir,
    sysfs: PathBuf,
    mountinfo: PathBuf,
    uuids: PathBuf,
    disk: PathBuf,
    controller: PathBuf,
    root: RootMount,
}

impl Fixture {
    fn nvme(partition: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let sysfs = directory.path().join("sys");
        let controller = sysfs.join("devices/pci0000:00/nvme/nvme0");
        let disk = controller.join("nvme0n1");
        fs::create_dir_all(disk.join("slaves")).unwrap();
        fs::create_dir_all(sysfs.join("dev/block")).unwrap();
        fs::create_dir_all(sysfs.join("class/nvme")).unwrap();
        symlink(sysfs.join("class/nvme"), controller.join("subsystem")).unwrap();
        symlink(&controller, disk.join("device")).unwrap();
        write_device(&disk, 259, 0, "nvme0n1", "disk");
        fs::write(disk.join("size"), "8000000000\n").unwrap();
        fs::write(controller.join("model"), "  Example E1.S SSD  \n").unwrap();
        fs::write(controller.join("serial"), "  E1S-0001  \n").unwrap();
        // The namespace identifier must never be presented as a filesystem UUID.
        fs::write(disk.join("uuid"), "namespace-not-filesystem").unwrap();
        let minor = u32::from(partition);
        let mounted = if partition {
            let path = disk.join("nvme0n1p1");
            fs::create_dir(&path).unwrap();
            write_device(&path, 259, 1, "nvme0n1p1", "partition");
            fs::write(path.join("size"), "1000\n").unwrap();
            path
        } else {
            disk.clone()
        };
        symlink(mounted, sysfs.join(format!("dev/block/259:{minor}"))).unwrap();
        let mountinfo = directory.path().join("mountinfo");
        fs::write(
            &mountinfo,
            format!("77 1 259:{minor} / /mnt/data rw - ext4 /dev/irrelevant rw\n"),
        )
        .unwrap();
        let uuids = directory.path().join("by-uuid");
        fs::create_dir(&uuids).unwrap();
        symlink("fixture-only-device", uuids.join("fs-identity")).unwrap();
        Self {
            _directory: directory,
            sysfs,
            mountinfo,
            uuids,
            disk,
            controller,
            root: RootMount {
                id: 77,
                major: 259,
                minor,
            },
        }
    }

    fn paths(&self) -> ObservationPaths<'_> {
        ObservationPaths {
            mountinfo: &self.mountinfo,
            sysfs: &self.sysfs,
            filesystem_uuids: &self.uuids,
        }
    }

    fn collect(&self) -> ObservationReport {
        collect(self.root, &self.paths(), |_| {
            Ok(Some((self.root.major, self.root.minor)))
        })
    }
}

fn write_device(path: &Path, major: u32, minor: u32, name: &str, kind: &str) {
    fs::write(
        path.join("uevent"),
        format!("MAJOR={major}\nMINOR={minor}\nDEVNAME={name}\nDEVTYPE={kind}\n"),
    )
    .unwrap();
}

#[test]
fn nvme_partition_uses_controller_identity_whole_namespace_capacity_and_filesystem_uuid() {
    let fixture = Fixture::nvme(true);
    let report = fixture.collect();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(
        report.value,
        HardwareObservation {
            model: Some("Example E1.S SSD".into()),
            serial: Some("E1S-0001".into()),
            capacity_bytes: Some(4_096_000_000_000),
            device_path: Some("/dev/nvme0n1p1".into()),
            mount_path: Some("/mnt/data".into()),
            filesystem_uuid: Some("fs-identity".into()),
            mapping: HardwareMapping::Single,
        }
    );
}

#[test]
fn whole_namespace_filesystem_does_not_require_a_partition() {
    let fixture = Fixture::nvme(false);
    let report = fixture.collect();
    assert!(report.errors.is_empty());
    assert_eq!(report.value.device_path.as_deref(), Some("/dev/nvme0n1"));
    assert_eq!(report.value.capacity_bytes, Some(4_096_000_000_000));
}

#[test]
fn bind_mount_uses_mount_id_and_decodes_kernel_path_escapes() {
    let fixture = Fixture::nvme(true);
    fs::write(&fixture.mountinfo,
        "1 0 259:1 / /original rw - ext4 /dev/nvme0n1p1 rw\n77 1 259:1 /nested /bind\\040mount\\011tab\\012line\\134slash rw - ext4 /dev/nvme0n1p1 rw\n").unwrap();
    let report = fixture.collect();
    assert!(report.errors.is_empty());
    assert_eq!(
        report.value.mount_path.as_deref(),
        Some("/bind mount\ttab\nline\\slash")
    );
    assert_eq!(report.value.device_path.as_deref(), Some("/dev/nvme0n1p1"));
}

#[test]
fn same_device_roots_can_have_distinct_mounts_without_creating_hardware_identity() {
    let fixture = Fixture::nvme(true);
    fs::write(&fixture.mountinfo,
        "77 1 259:1 /a /first rw - ext4 /dev/nvme0n1p1 rw\n78 1 259:1 /b /second rw - ext4 /dev/nvme0n1p1 rw\n").unwrap();
    let first = fixture.collect().value;
    let second = collect(
        RootMount {
            id: 78,
            ..fixture.root
        },
        &fixture.paths(),
        |_| Ok(Some((259, 1))),
    )
    .value;
    assert_eq!(first.mount_path.as_deref(), Some("/first"));
    assert_eq!(second.mount_path.as_deref(), Some("/second"));
    assert_eq!(first.serial, second.serial);
    assert_eq!(first.filesystem_uuid, second.filesystem_uuid);
}

#[test]
fn missing_attribute_does_not_discard_other_observation_fields() {
    let fixture = Fixture::nvme(true);
    fs::remove_file(fixture.controller.join("serial")).unwrap();
    let report = fixture.collect();
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.value.serial, None);
    assert_eq!(report.value.model.as_deref(), Some("Example E1.S SSD"));
    assert_eq!(report.value.filesystem_uuid.as_deref(), Some("fs-identity"));
    assert_eq!(report.value.mapping, HardwareMapping::Single);
}

#[test]
fn non_nvme_controller_keeps_model_and_serial_unknown_without_fallbacks() {
    let fixture = Fixture::nvme(false);
    fs::remove_file(fixture.controller.join("subsystem")).unwrap();
    fs::create_dir_all(fixture.sysfs.join("bus/scsi")).unwrap();
    symlink(
        fixture.sysfs.join("bus/scsi"),
        fixture.controller.join("subsystem"),
    )
    .unwrap();
    let report = fixture.collect();
    assert!(report.errors.is_empty());
    assert_eq!(report.value.model, None);
    assert_eq!(report.value.serial, None);
    assert_eq!(report.value.mapping, HardwareMapping::Single);
    assert_eq!(report.value.capacity_bytes, Some(4_096_000_000_000));
}

#[test]
fn virtual_and_stacked_devices_do_not_claim_single_physical_hardware() {
    for dependencies in 0..=2 {
        let fixture = Fixture::nvme(false);
        let virtual_disk = fixture.sysfs.join("devices/virtual/block/dm-0");
        fs::create_dir_all(virtual_disk.join("slaves")).unwrap();
        write_device(&virtual_disk, 259, 0, "dm-0", "disk");
        for index in 0..dependencies {
            symlink(
                &fixture.disk,
                virtual_disk.join(format!("slaves/candidate{index}")),
            )
            .unwrap();
        }
        let link = fixture.sysfs.join("dev/block/259:0");
        fs::remove_file(&link).unwrap();
        symlink(virtual_disk, link).unwrap();
        let report = fixture.collect();
        assert!(report.errors.is_empty());
        assert_eq!(
            report.value.mapping,
            if dependencies == 2 {
                HardwareMapping::Multiple
            } else {
                HardwareMapping::Unknown
            }
        );
        assert_eq!(report.value.model, None);
        assert_eq!(report.value.serial, None);
        assert_eq!(report.value.capacity_bytes, None);
        assert_eq!(report.value.device_path.as_deref(), Some("/dev/dm-0"));
        assert_eq!(report.value.filesystem_uuid.as_deref(), Some("fs-identity"));
    }
}

#[test]
fn filesystem_uuid_matching_uses_device_number_not_link_target_name() {
    let fixture = Fixture::nvme(true);
    symlink("same-looking-nvme0n1p1", fixture.uuids.join("wrong-fs")).unwrap();
    let uuid = filesystem_uuid(&fixture.uuids, fixture.root, |path| {
        Ok(Some(if path.file_name().unwrap() == "fs-identity" {
            (259, 1)
        } else {
            (8, 1)
        }))
    })
    .unwrap();
    assert_eq!(uuid.as_deref(), Some("fs-identity"));
    let ambiguous = filesystem_uuid(&fixture.uuids, fixture.root, |_| Ok(Some((259, 1))));
    assert!(matches!(
        ambiguous,
        Err(ObservationError::Invalid {
            detail: "multiple filesystem UUID links match the mounted device",
            ..
        })
    ));
}

#[test]
fn absent_uuid_links_never_fall_back_to_namespace_uuid() {
    let fixture = Fixture::nvme(true);
    fs::remove_file(fixture.uuids.join("fs-identity")).unwrap();
    let report = fixture.collect();
    assert!(report.errors.is_empty());
    assert_eq!(report.value.filesystem_uuid, None);
    assert_eq!(report.value.serial.as_deref(), Some("E1S-0001"));
}

#[test]
fn uuid_link_stat_error_is_retained_without_blocking_other_fields() {
    let fixture = Fixture::nvme(true);
    let report = collect(fixture.root, &fixture.paths(), |path| {
        Err(io_error(
            "stat filesystem UUID link",
            path,
            io::Error::from(io::ErrorKind::PermissionDenied),
        ))
    });
    assert_eq!(report.errors.len(), 1);
    assert!(
        matches!(&report.errors[0], ObservationError::Io { source, .. } if source.kind() == io::ErrorKind::PermissionDenied)
    );
    assert_eq!(report.value.filesystem_uuid, None);
    assert_eq!(report.value.model.as_deref(), Some("Example E1.S SSD"));
}

#[test]
fn real_metadata_reader_rejects_regular_fixture_file_without_opening_it_as_device() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("regular");
    fs::write(&target, "not a block device").unwrap();
    let link = directory.path().join("uuid-link");
    symlink(target, &link).unwrap();
    assert_eq!(block_device_number(&link).unwrap(), None);
}

#[test]
fn capacity_overflow_and_bad_number_do_not_hide_model() {
    for value in ["18446744073709551615", "not-a-number"] {
        let fixture = Fixture::nvme(false);
        fs::write(fixture.disk.join("size"), value).unwrap();
        let report = fixture.collect();
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.value.capacity_bytes, None);
        assert_eq!(report.value.model.as_deref(), Some("Example E1.S SSD"));
    }
}

#[test]
fn changed_or_missing_mount_is_unknown_without_guessing_by_path() {
    let fixture = Fixture::nvme(true);
    for text in [
        "78 1 259:1 / /mnt/data rw - ext4 /dev/nvme0n1p1 rw\n",
        "77 1 8:1 / /mnt/data rw - ext4 /dev/sda1 rw\n",
    ] {
        fs::write(&fixture.mountinfo, text).unwrap();
        let report = fixture.collect();
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.value, HardwareObservation::unknown());
    }
}

#[test]
fn malformed_mount_escape_and_oversized_input_are_explicit_errors() {
    assert!(unescape_mount_path("/path\\04", Path::new("fixture")).is_err());
    assert!(unescape_mount_path("/path\\777", Path::new("fixture")).is_err());
    assert!(unescape_mount_path("relative", Path::new("fixture")).is_err());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("attribute");
    fs::write(&path, "12345").unwrap();
    assert!(matches!(
        read_text(&path, 4),
        Err(ObservationError::Invalid {
            detail: "observation exceeds read limit",
            ..
        })
    ));
}

#[test]
fn missing_sysfs_keeps_mount_and_independently_resolved_filesystem_uuid() {
    let fixture = Fixture::nvme(true);
    fs::remove_file(fixture.sysfs.join("dev/block/259:1")).unwrap();
    let report = fixture.collect();
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.value.mount_path.as_deref(), Some("/mnt/data"));
    assert_eq!(report.value.filesystem_uuid.as_deref(), Some("fs-identity"));
    assert_eq!(report.value.device_path, None);
    assert_eq!(report.value.mapping, HardwareMapping::Unknown);
}

#[test]
fn empty_nvme_attributes_are_unknown_without_using_namespace_identifiers() {
    let fixture = Fixture::nvme(false);
    fs::write(fixture.controller.join("model"), "  \n").unwrap();
    fs::write(fixture.controller.join("serial"), "\n").unwrap();
    let report = fixture.collect();
    assert!(report.errors.is_empty());
    assert_eq!(report.value.model, None);
    assert_eq!(report.value.serial, None);
    assert_eq!(report.value.capacity_bytes, Some(4_096_000_000_000));
}

#[test]
fn contradictory_device_number_does_not_publish_block_association() {
    let fixture = Fixture::nvme(true);
    write_device(&fixture.disk.join("nvme0n1p1"), 8, 1, "sda1", "partition");
    let report = fixture.collect();
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.value.mount_path.as_deref(), Some("/mnt/data"));
    assert_eq!(report.value.filesystem_uuid, None);
    assert_eq!(report.value.device_path, None);
    assert_eq!(report.value.model, None);
    assert_eq!(report.value.mapping, HardwareMapping::Unknown);
}

#[test]
fn truncated_mountinfo_record_does_not_publish_mount_or_device_facts() {
    let fixture = Fixture::nvme(false);
    for line in ["77 1 259:0 / /mnt/data", "77 1 259:0 / /mnt/data rw - ext4"] {
        fs::write(&fixture.mountinfo, line).unwrap();
        let report = fixture.collect();
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.value, HardwareObservation::unknown());
    }
}

#[test]
fn held_root_mount_observation_does_not_reopen_a_replaced_directory_name() {
    let directory = tempfile::tempdir().unwrap();
    let registered = directory.path().join("registered");
    let engine = storage_engine::Engine::initialize(
        &directory.path().join("system"),
        storage_contract::Config::development([registered.clone()]).unwrap(),
    )
    .unwrap();
    let held = engine.open_disk_observation_directory("P01").unwrap();
    let before = held
        .observe(|access| Ok(read_root_mount(&registered, access).unwrap()))
        .unwrap();
    let original_inode = held.metadata().unwrap().ino();
    fs::rename(&registered, directory.path().join("moved-original")).unwrap();
    // Following the diagnostic name now would fail; no real mount is needed to
    // prove the observation's syscall uses AT_EMPTY_PATH on the retained FD.
    symlink("missing-replacement", &registered).unwrap();
    let after = held
        .observe(|access| Ok(read_root_mount(&registered, access).unwrap()))
        .unwrap();
    assert_eq!(
        (before.id, before.major, before.minor),
        (after.id, after.major, after.minor)
    );
    assert_eq!(held.metadata().unwrap().ino(), original_inode);
    assert!(fs::metadata(&registered).is_err());
}

#[test]
fn directory_observation_error_preserves_the_concrete_engine_source_chain() {
    use std::error::Error as _;
    let source = storage_engine::Error::new(
        storage_contract::ErrorCode::DiskUnavailable,
        "original directory observation failure",
    )
    .context(
        storage_contract::ErrorCode::DiskUnavailable,
        "held directory mount observation failed",
    );
    let source_message = source.to_string();
    let cause_message = source.source().unwrap().to_string();
    let error = ObservationError::Directory {
        path: PathBuf::from("/registered/directory"),
        source: Box::new(source),
    };
    assert_eq!(
        error.to_string(),
        "observe registered directory /registered/directory"
    );
    let retained = error.source().unwrap();
    assert_eq!(retained.to_string(), source_message);
    assert_eq!(retained.source().unwrap().to_string(), cause_message);
    let ObservationError::Directory { source, .. } = error else {
        panic!("expected the directory observation variant");
    };
    assert_eq!(source.code(), storage_contract::ErrorCode::DiskUnavailable);
}
