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
        Some(crate::telemetry::power::PowerUnavailable::Sampling)
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
        Some(crate::telemetry::power::PowerUnavailable::SensorUnreadable)
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
        Some(crate::telemetry::power::PowerUnavailable::SensorUnavailable)
    );
    write(&hwmon.join("hwmon1/power1_input"), "4200000");
    write(&hwmon.join("hwmon1/power1_average"), "4100000");
    symlink(&drive, hwmon.join("hwmon1/device")).unwrap();
    assert_eq!(read_disk_power(&drive, &hwmon, None).watts, Some(4.2));
    write(&hwmon.join("hwmon1/power2_input"), "1500000");
    assert_eq!(
        read_disk_power(&drive, &hwmon, None).reason,
        Some(crate::telemetry::power::PowerUnavailable::SensorAmbiguous)
    );
    assert_eq!(
        disk_power("ignored", true, true).1,
        Some(crate::telemetry::power::PowerUnavailable::SimulatedDisk)
    );
}
