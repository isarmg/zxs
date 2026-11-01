use super::*;
use std::time::Duration;

fn write(path: &Path, value: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, value).unwrap();
}

#[test]
fn cpu_uses_energy_differences_and_avoids_counting_subzones_twice() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("intel-rapl:0");
    let b = root.path().join("intel-rapl:1");
    for (p, name) in [(&a, "package-0"), (&b, "package-1")] {
        write(&p.join("name"), name);
        write(&p.join("energy_uj"), "1000000");
        write(&p.join("max_energy_range_uj"), "1000000000");
    }
    write(&a.join("core/name"), "core");
    write(&a.join("core/energy_uj"), "999999999");
    let mut sampler = CpuPowerSampler {
        root: root.path().into(),
        previous: None,
    };
    let start = Instant::now();
    assert_eq!(
        sampler.sample_at(start).reason,
        Some(crate::PowerUnavailable::Sampling)
    );
    write(&a.join("energy_uj"), "11000000");
    write(&b.join("energy_uj"), "21000000");
    assert_eq!(
        sampler.sample_at(start + Duration::from_secs(5)).watts,
        Some(6.0)
    );
    fs::remove_file(b.join("energy_uj")).unwrap();
    assert_eq!(
        sampler.sample_at(start + Duration::from_secs(10)).reason,
        Some(crate::PowerUnavailable::SensorUnreadable)
    );
}

#[test]
fn wrap_and_missing_counter_range_are_distinct() {
    let e = |microjoules, range| Energy { microjoules, range };
    assert_eq!(
        energy_delta(&e(900, Some(1000)), &e(100, Some(1000))),
        Some(200)
    );
    assert_eq!(energy_delta(&e(900, None), &e(100, None)), None);
    assert_eq!(energy_delta(&e(900, Some(1000)), &e(100, Some(2000))), None);
}

#[cfg(unix)]
#[test]
fn disk_sensor_requires_drive_ownership_and_does_not_sum_unknown_rails() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let drive = root.path().join("devices/nvme0");
    fs::create_dir_all(&drive).unwrap();
    let unrelated = root.path().join("devices/mainboard");
    fs::create_dir_all(&unrelated).unwrap();
    let hwmon = root.path().join("hwmon");
    write(&hwmon.join("hwmon0/power1_input"), "999000000");
    symlink(&unrelated, hwmon.join("hwmon0/device")).unwrap();
    assert_eq!(
        read_disk_power(&drive, &hwmon, None).reason,
        Some(crate::PowerUnavailable::SensorUnavailable)
    );
    write(&hwmon.join("hwmon1/power1_input"), "4200000");
    write(&hwmon.join("hwmon1/power1_average"), "4100000");
    symlink(&drive, hwmon.join("hwmon1/device")).unwrap();
    assert_eq!(read_disk_power(&drive, &hwmon, None).watts, Some(4.2));
    write(&hwmon.join("hwmon1/power2_input"), "1500000");
    assert_eq!(
        read_disk_power(&drive, &hwmon, None).reason,
        Some(crate::PowerUnavailable::SensorAmbiguous)
    );
    assert_eq!(
        disk_power(Some("ignored"), true, true).1,
        Some(crate::PowerUnavailable::SimulatedDisk)
    );
}

#[test]
fn unmanaged_physical_topology_reports_unknown_power_without_a_probe() {
    assert_eq!(
        disk_power(None, false, true).1,
        Some(PowerUnavailable::SensorUnavailable)
    );
    assert_eq!(
        disk_power(None, false, false).1,
        Some(PowerUnavailable::DiskOffline)
    );
}

#[test]
fn cpu_discovery_failure_never_publishes_partial_package_power() {
    let root = tempfile::tempdir().unwrap();
    let healthy = root.path().join("package0");
    let unreadable = root.path().join("package1");
    for (path, name) in [(&healthy, "package-0"), (&unreadable, "package-1")] {
        write(&path.join("name"), name);
        write(&path.join("energy_uj"), "1000000");
        write(&path.join("max_energy_range_uj"), "1000000000");
    }
    let mut sampler = CpuPowerSampler {
        root: root.path().into(),
        previous: None,
    };
    let start = Instant::now();
    assert_eq!(
        sampler.sample_at(start).reason,
        Some(PowerUnavailable::Sampling)
    );
    // A real read error, without permission assumptions or production fault hooks.
    fs::remove_file(unreadable.join("name")).unwrap();
    fs::create_dir(unreadable.join("name")).unwrap();
    write(&healthy.join("energy_uj"), "11000000");
    for seconds in [5, 10] {
        let result = sampler.sample_at(start + Duration::from_secs(seconds));
        assert_eq!(result.watts, None);
        assert_eq!(result.reason, Some(PowerUnavailable::SensorUnreadable));
        assert!(sampler.previous.is_none());
    }
    let error =
        discover_packages(root.path(), 0, &mut BTreeSet::new(), &mut BTreeSet::new()).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::IsADirectory);
    fs::remove_dir(unreadable.join("name")).unwrap();
    write(&unreadable.join("name"), "package-1");
    assert_eq!(
        sampler.sample_at(start + Duration::from_secs(15)).reason,
        Some(PowerUnavailable::Sampling)
    );
    write(&healthy.join("energy_uj"), "21000000");
    write(&unreadable.join("energy_uj"), "11000000");
    assert_eq!(
        sampler.sample_at(start + Duration::from_secs(20)).watts,
        Some(4.0)
    );
}

#[test]
fn absent_powercap_root_remains_sensor_unavailable() {
    let root = tempfile::tempdir().unwrap();
    let mut sampler = CpuPowerSampler {
        root: root.path().join("absent"),
        previous: None,
    };
    assert_eq!(
        sampler.sample().reason,
        Some(PowerUnavailable::SensorUnavailable)
    );
}

#[test]
fn cpu_discovery_does_not_publish_a_truncated_inventory() {
    let root = tempfile::tempdir().unwrap();
    let package = root.path().join("package0");
    write(&package.join("name"), "package-0");
    write(&package.join("energy_uj"), "1000000");
    for index in 0..256 {
        write(&root.path().join(format!("attribute-{index}")), "0");
    }
    let mut sampler = CpuPowerSampler {
        root: root.path().into(),
        previous: None,
    };
    let reading = sampler.sample();
    assert_eq!(reading.watts, None);
    assert_eq!(reading.reason, Some(PowerUnavailable::SensorUnreadable));
    assert!(sampler.previous.is_none());
}

#[test]
fn cpu_discovery_deduplicates_aliases_and_namespace_cycles() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let package = root.path().join("package0");
    write(&package.join("name"), "package-0");
    symlink(&package, root.path().join("alias")).unwrap();
    symlink(root.path(), root.path().join("subsystem")).unwrap();
    let mut packages = BTreeSet::new();
    discover_packages(root.path(), 0, &mut packages, &mut BTreeSet::new()).unwrap();
    assert_eq!(packages, BTreeSet::from([package.canonicalize().unwrap()]));
}

#[test]
fn disk_power_requires_complete_owned_channel_discovery() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    assert_ne!(
        rustix::process::geteuid().as_raw(),
        0,
        "permission failure regression requires a non-root runner"
    );
    let root = tempfile::tempdir().unwrap();
    let drive = root.path().join("drive");
    fs::create_dir(&drive).unwrap();
    let hwmon = root.path().join("hwmon");
    let second = hwmon.join("hwmon1");
    for (path, value) in [
        (hwmon.join("hwmon0"), "4200000"),
        (second.clone(), "1000000"),
    ] {
        write(&path.join("power1_input"), value);
        symlink(&drive, path.join("device")).unwrap();
    }
    // Owner resolution still works, but channel inventory cannot be opened.
    fs::set_permissions(&second, fs::Permissions::from_mode(0o100)).unwrap();
    let result = read_disk_power(&drive, &hwmon, None);
    fs::set_permissions(&second, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(result.watts, None);
    assert_eq!(result.reason, Some(PowerUnavailable::SensorUnreadable));
    assert_eq!(
        read_disk_power(&drive, &hwmon, None).reason,
        Some(PowerUnavailable::SensorAmbiguous)
    );
}

#[test]
fn disk_power_never_selects_a_channel_from_truncated_inventory() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let drive = root.path().join("drive");
    fs::create_dir(&drive).unwrap();
    let hwmon = root.path().join("hwmon");
    let sensor = hwmon.join("hwmon0");
    write(&sensor.join("power1_input"), "4200000");
    symlink(&drive, sensor.join("device")).unwrap();
    for index in 0..512 {
        write(&sensor.join(format!("attribute-{index}")), "0");
    }
    let result = read_disk_power(&drive, &hwmon, None);
    assert_eq!(result.watts, None);
    assert_eq!(result.reason, Some(PowerUnavailable::SensorUnreadable));
}
