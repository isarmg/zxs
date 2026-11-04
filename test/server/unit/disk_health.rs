use super::*;
use json_codec::json;

fn read(value: json_codec::Value, status: i32) -> Result<DiskHealth, Reason> {
    parse(
        &json_codec::to_vec(&value).unwrap(),
        status,
        1234,
        "/dev/fixture".into(),
    )
}

#[test]
fn actual_report_passed_failed_and_nullable_metrics() {
    for (passed, status, assessment) in [
        (true, 0, DiskHealthAssessment::Passed),
        (false, 8, DiskHealthAssessment::Failed),
    ] {
        assert_eq!(
            read(
                json!({"smartctl":{"exit_status":status},"smart_status":{"passed":passed},
            "temperature":{"current":36},"power_on_time":{"hours":u64::MAX}}),
                status
            )
            .unwrap(),
            DiskHealth::Smart {
                observed_at_unix_ms: 1234,
                device_path: "/dev/fixture".into(),
                assessment,
                temperature_celsius: Some(36),
                power_on_hours: Some(u64::MAX)
            }
        );
    }
    assert!(matches!(
        read(
            json!({"smartctl":{"exit_status":0},"smart_status":{"passed":true}}),
            0
        ),
        Ok(DiskHealth::Smart {
            temperature_celsius: None,
            power_on_hours: None,
            ..
        })
    ));
}

#[test]
fn zero_exit_is_not_health_and_partial_malformed_or_contradictory_results_are_rejected() {
    assert_eq!(
        read(json!({"smartctl":{"exit_status":0}}), 0),
        Err(Reason::IncompleteOutput)
    );
    for value in [
        json!({"smartctl":{"exit_status":0},"smart_status":{"passed":"true"}}),
        json!({"smartctl":{"exit_status":0},"smart_status":{"passed":true},"temperature":4}),
        json!({"smartctl":{"exit_status":0},"smart_status":{"passed":true},"power_on_time":{"hours":-1}}),
    ] {
        assert_eq!(read(value, 0), Err(Reason::InvalidOutput));
    }
    assert_eq!(
        read(
            json!({"smartctl":{"exit_status":8},"smart_status":{"passed":true}}),
            8
        ),
        Err(Reason::InvalidOutput)
    );
    assert_eq!(
        read(
            json!({"smartctl":{"exit_status":0},"smart_status":{"passed":true}}),
            8
        ),
        Err(Reason::InvalidOutput)
    );
    assert_eq!(parse(b"{", 0, 0, String::new()), Err(Reason::InvalidOutput));
    assert_eq!(
        parse(
            br#"{"smartctl":{"exit_status":0},"smart_status":{"passed":false,"passed":true}}"#,
            0,
            0,
            String::new()
        ),
        Err(Reason::InvalidOutput)
    );
    assert_eq!(
        read(
            json!({"smartctl":{"exit_status":4},"smart_status":{"passed":true}}),
            4
        ),
        Err(Reason::QueryFailed)
    );
}

#[test]
fn low_power_requires_report_and_not_just_exit_bits() {
    assert_eq!(
        read(
            json!({"smartctl":{"exit_status":3},"power_mode":{"name":"STANDBY"}}),
            3
        ),
        Err(Reason::LowPower)
    );
    assert_eq!(
        read(json!({"smartctl":{"exit_status":3}}), 3),
        Err(Reason::QueryFailed)
    );
}

fn fake(script: &str) -> Command {
    let mut command = Command::new("/usr/bin/python3");
    command
        .args(["-c", script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command
}

fn query_command(command: &mut Command, timeout: Duration) -> Result<(i32, Vec<u8>), Reason> {
    let root = tempfile::tempdir().unwrap();
    let engine = storage_engine::Engine::initialize(
        &root.path().join("system"),
        storage_contract::Config::development([root.path().join("disk")]).unwrap(),
    )
    .unwrap();
    let directory = engine.open_disk_observation_directory("P01").unwrap();
    directory
        .observe(|access| Ok(run(command, timeout, access)))
        .unwrap()
}

#[test]
fn command_output_and_time_are_bounded_and_exit_failure_is_preserved() {
    let (code, bytes) = query_command(
        &mut fake("import sys; print('fixture'); sys.exit(8)"),
        Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(code, 8);
    assert_eq!(bytes, b"fixture\n");
    assert_eq!(
        query_command(
            &mut fake("import time; time.sleep(3)"),
            Duration::from_millis(30),
        ),
        Err(Reason::TimedOut)
    );
    assert_eq!(
        query_command(&mut fake("print('x' * 100000)"), Duration::from_secs(1)),
        Err(Reason::InvalidOutput)
    );
    assert_eq!(
        query_command(
            &mut Command::new("/nonexistent/zxs-smartctl-fixture"),
            Duration::from_secs(1),
        ),
        Err(Reason::ToolMissing)
    );
}

#[test]
fn unavailable_mapping_never_executes_a_device_query() {
    let root = tempfile::tempdir().unwrap();
    let engine = storage_engine::Engine::initialize(
        &root.path().join("system"),
        storage_contract::Config::development([root.path().join("disk")]).unwrap(),
    )
    .unwrap();
    let directory = engine.open_disk_observation_directory("P01").unwrap();
    let mut hardware = HardwareObservation::unknown();
    assert_eq!(
        observe(root.path(), &directory, &hardware),
        unavailable(Reason::MappingUnknown)
    );
    hardware.mapping = HardwareMapping::Multiple;
    assert_eq!(
        observe(root.path(), &directory, &hardware),
        unavailable(Reason::MultipleDevices)
    );
}

#[test]
fn held_sysfs_binding_rejects_replacement_mount_changes_and_dependencies() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let sysfs = temp.path();
    let disk = sysfs.join("devices/disk");
    let partition = disk.join("part");
    fs::create_dir_all(disk.join("slaves")).unwrap();
    fs::create_dir(&partition).unwrap();
    fs::create_dir_all(sysfs.join("dev/block")).unwrap();
    fs::write(
        disk.join("uevent"),
        "MAJOR=259\nMINOR=0\nDEVNAME=fixture\nDEVTYPE=disk\n",
    )
    .unwrap();
    fs::write(
        partition.join("uevent"),
        "MAJOR=259\nMINOR=1\nDEVNAME=fixture1\nDEVTYPE=partition\n",
    )
    .unwrap();
    symlink(&partition, sysfs.join("dev/block/259:1")).unwrap();
    let mount = RootMount {
        id: 77,
        major: 259,
        minor: 1,
    };
    let binding = DeviceBinding {
        mount,
        sysfs: File::open(&disk).unwrap(),
        sysfs_path: disk.clone(),
        mounted_sysfs: File::open(&partition).unwrap(),
        mounted_sysfs_path: partition.clone(),
        device: File::open(&disk).unwrap(),
        device_path: "fixture".into(),
        protocol: Protocol::Nvme,
        number: (259, 0),
    };
    assert_eq!(binding.revalidate_mount(mount, sysfs), Ok(()));
    assert_eq!(
        binding.revalidate_mount(RootMount { id: 78, ..mount }, sysfs),
        Err(Reason::DeviceChanged)
    );
    fs::write(disk.join("slaves/child"), "").unwrap();
    assert_eq!(
        binding.revalidate_mount(mount, sysfs),
        Err(Reason::DeviceChanged)
    );
    fs::remove_file(disk.join("slaves/child")).unwrap();
    fs::rename(&partition, disk.join("old-part")).unwrap();
    fs::create_dir(&partition).unwrap();
    assert_eq!(
        binding.revalidate_mount(mount, sysfs),
        Err(Reason::DeviceChanged)
    );
}

#[test]
fn status_bits_preserve_current_failure_and_historical_warnings() {
    for (status, assessment) in [
        (16, DiskHealthAssessment::Failed),
        (32, DiskHealthAssessment::Warning),
        (64, DiskHealthAssessment::Warning),
        (128, DiskHealthAssessment::Warning),
        (224, DiskHealthAssessment::Warning),
    ] {
        assert!(
            matches!(read(json!({"smartctl":{"exit_status":status},"smart_status":{"passed":true}}), status),
            Ok(DiskHealth::Smart { assessment: actual, .. }) if actual == assessment)
        );
    }
}

#[test]
fn observation_errors_do_not_invent_device_changes() {
    assert_eq!(
        observation_reason(ObservationError::Io {
            operation: "fixture",
            path: "fixture".into(),
            source: io::Error::from(io::ErrorKind::PermissionDenied)
        }),
        Reason::PermissionDenied
    );
    assert_eq!(
        observation_reason(ObservationError::Invalid {
            path: "fixture".into(),
            detail: "malformed"
        }),
        Reason::MappingUnknown
    );
}

#[test]
fn held_node_does_not_follow_path_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("synthetic-device");
    fs::write(&path, "old-device").unwrap();
    let held = File::open(&path).unwrap();
    assert_eq!(same_held_inode(&held, &path), Ok(()));
    fs::rename(&path, temp.path().join("old-device")).unwrap();
    fs::write(&path, "replacement").unwrap();
    assert_eq!(same_held_inode(&held, &path), Err(Reason::DeviceChanged));
    let mut text = String::new();
    held.take(32).read_to_string(&mut text).unwrap();
    assert_eq!(text, "old-device");
}

#[test]
fn canceled_query_refuses_spawn_before_starting_an_external_process() {
    let root = tempfile::tempdir().unwrap();
    let engine = storage_engine::Engine::initialize(
        &root.path().join("system"),
        storage_contract::Config::development([root.path().join("disk")]).unwrap(),
    )
    .unwrap();
    let directory = engine.open_disk_observation_directory("P01").unwrap();
    let result = directory.observe(|access| {
        let mut stop = engine.inject_disk_quiesce("P01").unwrap();
        stop.release_control();
        assert_eq!(
            run(
                &mut Command::new("/nonexistent/zxs-smartctl-fixture"),
                Duration::from_secs(1),
                access,
            ),
            Err(Reason::DeviceChanged),
        );
        Ok(())
    });
    assert_eq!(
        result.unwrap_err().code(),
        storage_contract::ErrorCode::DiskUnavailable
    );
}

#[test]
fn observation_child_is_reaped_before_error_and_panic_release_admission() {
    for unwind in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let engine = storage_engine::Engine::initialize(
            &root.path().join("system"),
            storage_contract::Config::development([root.path().join("disk")]).unwrap(),
        )
        .unwrap();
        let directory = engine.open_disk_observation_directory("P01").unwrap();
        let mut process_id = None;
        let mut stop = None;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            directory.observe(|access| {
                let process = fake("import time; time.sleep(30)").spawn().unwrap();
                process_id = Some(process.id());
                let _child = QueryChild {
                    process: Some(process),
                    directory: access,
                };
                let mut stopping = engine.inject_disk_quiesce("P01").unwrap();
                stopping.release_control();
                assert!(!stopping.is_drained().unwrap());
                stop = Some(stopping);
                if unwind {
                    panic!("controlled observation callback panic");
                }
                Err::<(), _>(storage_engine::Error::new(
                    storage_contract::ErrorCode::OperationAborted,
                    "controlled observation callback error",
                ))
            })
        }));
        if unwind {
            assert!(result.is_err());
        } else {
            assert_eq!(
                result.unwrap().unwrap_err().code(),
                storage_contract::ErrorCode::OperationAborted
            );
        }
        assert!(stop.unwrap().is_drained().unwrap());
        let pid =
            rustix::process::Pid::from_raw(i32::try_from(process_id.unwrap()).unwrap()).unwrap();
        assert_eq!(
            rustix::process::waitpid(Some(pid), rustix::process::WaitOptions::NOHANG).unwrap_err(),
            rustix::io::Errno::CHILD,
        );
    }
}

#[test]
fn quiesced_directory_cannot_start_smart_worker_even_for_a_single_mapping() {
    let root = tempfile::tempdir().unwrap();
    let disk = root.path().join("disk");
    let engine = storage_engine::Engine::initialize(
        &root.path().join("system"),
        storage_contract::Config::development([disk.clone()]).unwrap(),
    )
    .unwrap();
    let directory = engine.open_disk_observation_directory("P01").unwrap();
    let mut stop = engine.inject_disk_quiesce("P01").unwrap();
    stop.release_control();
    let mut hardware = HardwareObservation::unknown();
    hardware.mapping = HardwareMapping::Single;
    assert_eq!(
        observe(&disk, &directory, &hardware),
        unavailable(Reason::DeviceChanged)
    );
    assert!(stop.is_drained().unwrap());
}

fn run(
    command: &mut Command,
    timeout: Duration,
    directory: &DiskObservationAccess<'_>,
) -> Result<(i32, Vec<u8>), Reason> {
    run_until(command, Instant::now() + timeout, directory)
}

fn ata_fixture(health: AtaHealth) -> AtaSnapshot {
    let mut identify = [0; 512];
    for (index, word) in [
        (0, 0x0040_u16),
        (60, 1000),
        (80, 0x0080),
        (82, 1),
        (83, 0x4000),
        (85, 1),
        (87, 0x4000),
    ] {
        identify[index * 2..index * 2 + 2].copy_from_slice(&word.to_le_bytes());
    }
    let model = b"ZXS SYNTHETIC OFFLINE DISK               ";
    for (index, pair) in model.as_chunks::<2>().0.iter().enumerate() {
        identify[54 + index * 2] = pair[1];
        identify[55 + index * 2] = pair[0];
    }
    let mut attributes = [0; 512];
    let mut thresholds = [0; 512];
    attributes[0] = 16;
    thresholds[0] = 16;
    for (index, id, raw) in [(0, 5, 0_u64), (1, 9, 1234), (2, 194, 36)] {
        let offset = 2 + index * 12;
        attributes[offset] = id;
        attributes[offset + 1] = 3;
        attributes[offset + 3] = 100;
        attributes[offset + 4] = 100;
        attributes[offset + 5..offset + 11].copy_from_slice(&raw.to_le_bytes()[..6]);
        thresholds[offset] = id;
        thresholds[offset + 1] = if id == 5 { 36 } else { 0 };
    }
    attributes[511] = 0_u8.wrapping_sub(
        attributes[..511]
            .iter()
            .fold(0_u8, |sum, byte| sum.wrapping_add(*byte)),
    );
    thresholds[511] = 0_u8.wrapping_sub(
        thresholds[..511]
            .iter()
            .fold(0_u8, |sum, byte| sum.wrapping_add(*byte)),
    );
    AtaSnapshot {
        identify,
        measurements: Some(AtaMeasurements {
            attributes,
            thresholds,
            health,
        }),
    }
}

#[test]
fn offline_decoder_receives_only_bounded_anonymous_replay_and_no_device_path() {
    let snapshot = ata_fixture(AtaHealth::Passed);
    let replay = snapshot.replay();
    assert!(replay.len() < 8 * 1024);
    assert_eq!(replay.matches("DATA START").count(), 3);
    assert_eq!(replay.matches("REPORT-IOCTL: Device=offline").count(), 8);
    assert!(!replay.contains("/dev/"));
    assert!(!replay.contains("/proc/"));
    assert!(!replay.contains("CHECK POWER MODE"));
    let mut input = snapshot.input().unwrap();
    let mut actual = String::new();
    input.read_to_string(&mut actual).unwrap();
    assert_eq!(actual, replay);
    let command = replay_command(snapshot.input().unwrap());
    assert_eq!(command.get_program(), "/usr/sbin/smartctl");
    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        ["--json", "--health", "--attributes", "-"]
    );
    assert_eq!(
        command.get_envs().collect::<Vec<_>>(),
        [(
            std::ffi::OsStr::new("LC_ALL"),
            Some(std::ffi::OsStr::new("C"))
        )]
    );
}

#[test]
fn ata_capability_does_not_enable_disabled_or_unknown_smart() {
    let mut snapshot = ata_fixture(AtaHealth::Passed);
    assert!(ata_smart_readable(&snapshot.identify));
    snapshot.identify[82 * 2] = 0;
    assert!(!ata_smart_readable(&snapshot.identify));
    snapshot.identify[83 * 2 + 1] = 0;
    assert!(
        ata_smart_readable(&snapshot.identify),
        "explicit enable admits ambiguous support"
    );
    snapshot.identify[85 * 2] = 0;
    assert!(!ata_smart_readable(&snapshot.identify));
    snapshot.identify[85 * 2] = 1;
    snapshot.identify[87 * 2 + 1] = 0;
    assert!(!ata_smart_readable(&snapshot.identify));
}

#[test]
fn unavailable_ata_registers_are_replayed_as_missing_not_healthy() {
    let text = ata_fixture(AtaHealth::RegistersUnavailable).replay();
    assert!(text.contains("SMART STATUS CHECK returned -1 errno=38"));
    assert!(!text.contains("SMART STATUS CHECK returned 0"));
}

#[test]
fn native_nvme_report_preserves_unknown_critical_bits_and_nullable_temperature() {
    for (warning, assessment) in [
        (0, DiskHealthAssessment::Passed),
        (1, DiskHealthAssessment::Failed),
        (128, DiskHealthAssessment::Failed),
    ] {
        assert_eq!(
            nvme_record(
                disk_smart::NvmeHealth {
                    critical_warning: warning,
                    temperature_celsius: None,
                    power_on_hours: 0
                },
                123,
                "/dev/fixture".into()
            ),
            DiskHealth::Smart {
                observed_at_unix_ms: 123,
                device_path: "/dev/fixture".into(),
                assessment,
                temperature_celsius: None,
                power_on_hours: Some(0),
            }
        );
    }
}

#[test]
fn native_capture_checks_stop_and_deadline_before_any_ioctl() {
    let root = tempfile::tempdir().unwrap();
    let engine = storage_engine::Engine::initialize(
        &root.path().join("system"),
        storage_contract::Config::development([root.path().join("disk")]).unwrap(),
    )
    .unwrap();
    let directory = engine.open_disk_observation_directory("P01").unwrap();
    // A harmless regular file deliberately cannot answer device ioctls. If a
    // call crossed the stop/deadline boundary it would fail with a different error.
    let file = tempfile::tempfile().unwrap();
    directory
        .observe(|access| {
            assert!(matches!(
                capture_nvme(&file, access, Instant::now()),
                Err(Reason::TimedOut)
            ));
            assert!(matches!(
                capture_ata(&file, access, Instant::now()),
                Err(Reason::TimedOut)
            ));
            let mut stop = engine.inject_disk_quiesce("P01").unwrap();
            stop.release_control();
            let deadline = Instant::now() + Duration::from_secs(2);
            assert!(matches!(
                capture_nvme(&file, access, deadline),
                Err(Reason::DeviceChanged)
            ));
            assert!(matches!(
                capture_ata(&file, access, deadline),
                Err(Reason::DeviceChanged)
            ));
            Ok(())
        })
        .unwrap_err();
}

#[test]
#[ignore = "requires an installed official smartctl; offline synthetic replay only, never a device"]
fn installed_smartctl_offline_replay_preserves_real_health_and_metrics() {
    for (health, assessment) in [
        (AtaHealth::Passed, DiskHealthAssessment::Passed),
        (AtaHealth::Failed, DiskHealthAssessment::Failed),
    ] {
        let snapshot = ata_fixture(health);
        let (status, bytes) = query_command(
            &mut replay_command(snapshot.input().unwrap()),
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(
            parse(&bytes, status, 123, "/dev/fixture".into()).unwrap(),
            DiskHealth::Smart {
                observed_at_unix_ms: 123,
                device_path: "/dev/fixture".into(),
                assessment,
                temperature_celsius: Some(36),
                power_on_hours: Some(1234),
            }
        );
    }
}

#[test]
fn native_error_categories_keep_permissions_timeout_and_invalid_data_distinct() {
    assert_eq!(
        native_reason(disk_smart::Error::Io {
            operation: "fixture",
            source: io::Error::from(io::ErrorKind::PermissionDenied)
        }),
        Reason::PermissionDenied
    );
    assert_eq!(
        native_reason(disk_smart::Error::ScsiStatus {
            operation: "fixture",
            status: 0,
            host: 3,
            driver: 0
        }),
        Reason::TimedOut
    );
    assert_eq!(
        native_reason(disk_smart::Error::ScsiStatus {
            operation: "fixture",
            status: 2,
            host: 0,
            driver: 8
        }),
        Reason::QueryFailed
    );
    assert_eq!(
        native_reason(disk_smart::Error::InvalidResponse {
            operation: "fixture",
            detail: "truncated"
        }),
        Reason::InvalidOutput
    );
}

#[test]
fn offline_smart_replay_preserves_every_fixed_width_row() {
    let data: [u8; 512] = std::array::from_fn(|index| (index % 256) as u8);
    let mut text = String::new();
    replay_data(&mut text, "IDENTIFY DEVICE", &data);
    let rows: Vec<_> = text
        .lines()
        .filter(|line| line.as_bytes().first().is_some_and(u8::is_ascii_digit))
        .collect();
    assert_eq!(rows.len(), 32);
    for (index, row) in rows.iter().enumerate() {
        let start = index * 16;
        let (range, bytes) = row.split_once(": ").unwrap();
        assert_eq!(range, format!("{start:03}-{:03}", start + 15));
        let expected: Vec<_> = (start..start + 16)
            .map(|offset| format!("{:02x}", data[offset]))
            .collect();
        assert_eq!(bytes, expected.join(" "));
    }
    assert!(text.ends_with("===== [IDENTIFY DEVICE] DATA END =====\n"));
}

#[test]
fn boxed_directory_observation_keeps_the_original_health_classification() {
    for (source, expected) in [
        (
            storage_engine::Error::Filesystem(platform_io::fs_safety::Error::Io(io::Error::from(
                io::ErrorKind::PermissionDenied,
            ))),
            Reason::PermissionDenied,
        ),
        (
            storage_engine::Error::new(storage_contract::ErrorCode::DiskUnavailable, "offline"),
            Reason::DeviceChanged,
        ),
        (
            storage_engine::Error::new(
                storage_contract::ErrorCode::DiskIdentityMismatch,
                "changed",
            ),
            Reason::DeviceChanged,
        ),
        (
            storage_engine::Error::new(storage_contract::ErrorCode::ObjectCorrupt, "invalid"),
            Reason::QueryFailed,
        ),
    ] {
        assert_eq!(
            observation_reason(ObservationError::Directory {
                path: PathBuf::from("/registered/directory"),
                source: Box::new(source),
            }),
            expected,
        );
    }
}
