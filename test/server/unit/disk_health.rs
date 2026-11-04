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

#[test]
fn command_output_and_time_are_bounded_and_exit_failure_is_preserved() {
    let (code, bytes) = run(
        &mut fake("import sys; print('fixture'); sys.exit(8)"),
        Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(code, 8);
    assert_eq!(bytes, b"fixture\n");
    assert_eq!(
        run(
            &mut fake("import time; time.sleep(3)"),
            Duration::from_millis(30)
        ),
        Err(Reason::TimedOut)
    );
    assert_eq!(
        run(&mut fake("print('x' * 100000)"), Duration::from_secs(1)),
        Err(Reason::InvalidOutput)
    );
    assert_eq!(
        run(
            &mut Command::new("/nonexistent/zxs-smartctl-fixture"),
            Duration::from_secs(1)
        ),
        Err(Reason::ToolMissing)
    );
}

#[test]
fn unavailable_mapping_never_executes_a_device_query() {
    let root = tempfile::tempdir().unwrap();
    let fd = File::open(root.path()).unwrap();
    let mut hardware = HardwareObservation::unknown();
    assert_eq!(
        observe(root.path(), &fd, &hardware),
        unavailable(Reason::MappingUnknown)
    );
    hardware.mapping = HardwareMapping::Multiple;
    assert_eq!(
        observe(root.path(), &fd, &hardware),
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
        protocol: "nvme",
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
