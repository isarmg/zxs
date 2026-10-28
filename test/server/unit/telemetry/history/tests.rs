use super::*;
use crate::telemetry::{CpuStatus, MemoryStatus};

#[path = "budget.rs"]
mod budget;

fn sample(at: i64, value: Option<f64>) -> Sample {
    Sample {
        unix_seconds: at,
        observations: 1,
        statistics: None,
        status: Snapshot {
            disk_sampling_unavailable: false,
            collected_at: super::super::timestamp(at).unwrap(),
            cpu: CpuStatus {
                usage_percent: value.map(|value| value.min(100.0) as f32),
                logical_count: 4,
                power_watts: value,
                power_unavailable_reason: value
                    .is_none()
                    .then_some(crate::PowerUnavailable::SensorUnavailable),
            },
            memory: MemoryStatus {
                total_bytes: Some(18446744073709551615),
                used_bytes: Some(10),
                available_bytes: Some(18446744073709551605),
            },
            disks: vec![],
        },
    }
}

#[test]
fn restart_recovers_real_samples_and_keeps_missing_values_absent() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("cache");
    let now = 1_800_000_120;
    let mut cache = HistoryStore::open(&path, now).unwrap();
    for offset in (0..80).step_by(5) {
        cache.record(sample(now + offset, None)).unwrap();
    }
    drop(cache);
    let cache = HistoryStore::open(&path, now + 80).unwrap();
    let result = cache.samples(now, now + 80, 5);
    assert_eq!(result.len(), 16);
    assert!(
        result
            .iter()
            .all(|sample| sample.cpu.power_watts.is_none() && sample.cpu.usage_percent.is_none())
    );
    assert_eq!(result[0].memory.total_bytes, Some(18446744073709551615));
    assert_eq!(cache.minutes.len(), 1);
}

#[test]
fn range_averages_real_observations_without_duplicate_resolution_or_fake_gaps() {
    let root = tempfile::tempdir().unwrap();
    let now = 1_800_000_120;
    let mut cache = HistoryStore::open(&root.path().join("cache"), now).unwrap();
    for offset in (0..180).step_by(5) {
        cache
            .record(sample(now + offset, Some(offset as f64)))
            .unwrap();
    }
    let result = cache.samples(now, now + 180, 60);
    assert_eq!(result.len(), 3);
    assert_eq!(result[0].cpu.power_watts, Some(27.5));
    assert_eq!(result[1].cpu.power_watts, Some(87.5));
    assert_eq!(result[2].cpu.power_watts, Some(147.5));
    assert!(cache.samples(now + 300, now + 600, 5).is_empty());
}

#[test]
fn cleanup_only_removes_recognized_expired_regular_cache_files() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("cache");
    fs::create_dir(&path).unwrap();
    fs::write(path.join("minute-1.jsonl"), b"expired").unwrap();
    fs::write(path.join("keep.txt"), b"foreign").unwrap();
    fs::create_dir(path.join("recent-1.jsonl")).unwrap();
    let now = 1_800_000_120;
    let cache = HistoryStore::open(&path, now).unwrap();
    assert!(cache.recent.is_empty());
    assert!(!path.join("minute-1.jsonl").exists());
    assert!(path.join("keep.txt").exists());
    assert!(path.join("recent-1.jsonl").is_dir());
}

#[test]
fn month_range_stays_bounded_and_weights_multiple_resolutions_by_real_observations() {
    let root = tempfile::tempdir().unwrap();
    let now = 1_800_000_120;
    let mut cache = HistoryStore::open(&root.path().join("cache"), now).unwrap();
    for at in ((now - RETENTION_SECONDS - 120)..=now).step_by(60) {
        let mut point = sample(at, Some(20.0));
        point = average(&vec![point; 12]);
        cache.minutes.push_back(point);
    }
    cache.prune(now);
    assert!(cache.minutes.len() <= MAX_MINUTE_POINTS);
    assert!(cache.minutes.front().unwrap().unix_seconds >= now - RETENTION_SECONDS);
    let result = cache.samples(now - 30 * 86400, now, 7200);
    assert!(result.len() <= 361);
    assert!(
        result
            .iter()
            .all(|point| point.cpu.power_watts == Some(20.0))
    );
    let mut minute = sample(now - 60, Some(20.0));
    minute = average(&vec![minute; 12]);
    let actual = sample(now, Some(0.0));
    assert_eq!(
        average(&[minute, actual]).status.cpu.power_watts,
        Some(240.0 / 13.0)
    );
}

#[test]
fn mixed_null_metrics_remain_exact_after_two_aggregations_and_persistence() {
    let root = tempfile::tempdir().unwrap();
    let now = 1_800_000_120;
    let metric = |at, value: Option<f64>| {
        let mut sample = sample(at, value);
        let bytes = value.map(|value| value as u64);
        sample.status.memory.total_bytes = bytes;
        sample.status.memory.used_bytes = bytes;
        sample.status.memory.available_bytes = bytes;
        sample.status.disks = vec![super::super::DiskStatus {
            id: "P01".into(),
            name: "P01".into(),
            online: true,
            read_only: false,
            simulation: false,
            total_bytes: bytes,
            used_bytes: bytes,
            available_bytes: bytes,
            usage_percent: value,
            power_watts: value,
            power_unavailable_reason: value
                .is_none()
                .then_some(crate::PowerUnavailable::SensorUnavailable),
        }];
        sample
    };
    let a = (0..12)
        .map(|i| metric(now + i * 5, (i == 0).then_some(100.0)))
        .collect::<Vec<_>>();
    let b = (0..12)
        .map(|i| metric(now + 60 + i * 5, Some(0.0)))
        .collect::<Vec<_>>();
    let minute_a = average(&a);
    let minute_b = average(&b);
    assert_eq!(minute_a.observations, 12);
    assert!(minute_a.valid_at(now + 120));
    assert!(minute_b.valid_at(now + 120));
    // Round trip through the actual cache serializer before recombining.
    let cache = HistoryStore::open(&root.path().join("cache"), now + 120).unwrap();
    cache.append_file(false, &minute_a).unwrap();
    cache.append_file(false, &minute_b).unwrap();
    drop(cache);
    let cache = HistoryStore::open(&root.path().join("cache"), now + 120).unwrap();
    assert_eq!(cache.minutes.len(), 2);
    let twice = average(&cache.minutes.iter().cloned().collect::<Vec<_>>());
    let direct = average(&a.into_iter().chain(b).collect::<Vec<_>>());
    let expected = 100.0 / 13.0;
    assert!((twice.status.cpu.usage_percent.unwrap() - expected as f32).abs() < 0.00001);
    assert_eq!(twice.status.cpu.power_watts, Some(expected));
    assert_eq!(twice.status.memory.used_bytes, Some(7));
    assert_eq!(twice.status.memory.total_bytes, Some(7));
    assert_eq!(twice.status.memory.available_bytes, Some(7));
    assert_eq!(twice.status.disks[0].usage_percent, Some(expected));
    assert_eq!(twice.status.disks[0].power_watts, Some(expected));
    assert_eq!(twice.status.disks[0].used_bytes, Some(7));
    assert_eq!(
        json_codec::to_value(twice.status).unwrap(),
        json_codec::to_value(direct.status).unwrap()
    );
}

#[test]
fn byte_statistics_round_trip_u128_sums_and_avoid_repeated_integer_rounding() {
    let now = 1_800_000_120;
    let mut a = sample(now, Some(1.0));
    a.status.memory.used_bytes = Some(0);
    let mut b = sample(now + 5, Some(1.0));
    b.status.memory.used_bytes = Some(1);
    let minute = average(&[a, b]);
    assert_eq!(minute.status.memory.used_bytes, Some(0));
    // total_bytes is u64::MAX per observation, so its serialized sum is
    // larger than u64. The cache must preserve that u128 integer exactly.
    let encoded = json_codec::to_vec(&minute).unwrap();
    assert!(String::from_utf8_lossy(&encoded).contains("36893488147419103230"));
    let restored: Sample = json_codec::from_slice(&encoded).unwrap();
    assert!(restored.valid_at(now + 60));
    let mut c = sample(now + 60, Some(1.0));
    c.status.memory.used_bytes = Some(2);
    let combined = average(&[restored, c]);
    assert_eq!(combined.status.memory.used_bytes, Some(1));
    assert_eq!(
        combined.status.memory.total_bytes,
        Some(18446744073709551615)
    );
}

#[test]
fn old_aggregates_without_metric_statistics_are_ignored_but_raw_points_recover() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("cache");
    let now = 1_800_000_120;
    let cache = HistoryStore::open(&path, now).unwrap();
    let mut old = sample(now - 60, Some(100.0));
    old.observations = 12;
    assert!(!old.valid_at(now));
    cache.append_file(false, &old).unwrap();
    cache
        .append_file(true, &sample(now - 55, Some(20.0)))
        .unwrap();
    drop(cache);
    let cache = HistoryStore::open(&path, now).unwrap();
    assert_eq!(cache.minutes.len(), 1);
    assert_eq!(
        cache.minutes.front().unwrap().status.cpu.power_watts,
        Some(20.0)
    );
    assert_eq!(cache.minutes.front().unwrap().observations, 1);
}

#[test]
fn disk_averages_keep_valid_observations_when_the_bucket_ends_offline() {
    let now = 1_800_000_120;
    let disk_sample = |at, online: bool| {
        let mut point = sample(at, Some(20.0));
        point.status.disks = vec![super::super::DiskStatus {
            id: "P01".into(),
            name: "P01".into(),
            online,
            read_only: false,
            simulation: true,
            total_bytes: online.then_some(1000),
            used_bytes: online.then_some(300),
            available_bytes: online.then_some(600),
            usage_percent: online.then_some(30.0),
            power_watts: None,
            power_unavailable_reason: Some(crate::PowerUnavailable::SimulatedDisk),
        }];
        point
    };
    for (first_online, last_online, expected) in [
        (true, false, Some(30.0)),
        (false, true, Some(30.0)),
        (false, false, None),
    ] {
        let aggregate = average(&[
            disk_sample(now, first_online),
            disk_sample(now + 5, last_online),
        ]);
        assert!(aggregate.valid_at(now + 5));
        assert_eq!(aggregate.observations, 2);
        assert_eq!(aggregate.status.disks[0].online, last_online);
        assert_eq!(aggregate.status.disks[0].usage_percent, expected);
        assert_eq!(aggregate.status.disks[0].used_bytes, expected.map(|_| 300));
    }
}

#[test]
fn recovery_ignores_partial_invalid_and_future_samples() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("cache");
    let now = 1_800_000_120;
    let mut cache = HistoryStore::open(&path, now).unwrap();
    cache.record(sample(now, Some(1.0))).unwrap();
    drop(cache);
    let path_file = path.join(format!("recent-{}.jsonl", now / 3600));
    let mut file = OpenOptions::new().append(true).open(path_file).unwrap();
    writeln!(file, "{{not-json}}").unwrap();
    writeln!(
        file,
        "{}",
        json_codec::to_string(&sample(now + 100, Some(2.0))).unwrap()
    )
    .unwrap();
    write!(file, "{{\"unix_seconds\":").unwrap();
    drop(file);
    let cache = HistoryStore::open(&path, now + 1).unwrap();
    assert_eq!(cache.samples(now, now + 1, 5).len(), 1);
}

#[cfg(unix)]
#[test]
fn symlink_cache_directory_and_files_are_not_followed() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target");
    fs::create_dir(&target).unwrap();
    symlink(&target, root.path().join("linked")).unwrap();
    assert!(HistoryStore::open(&root.path().join("linked"), 1_800_000_120).is_err());
    let path = root.path().join("cache");
    let mut cache = HistoryStore::open(&path, 1_800_000_120).unwrap();
    let external = root.path().join("external");
    fs::write(&external, b"untouched").unwrap();
    symlink(
        &external,
        path.join(format!("recent-{}.jsonl", 1_800_000_120 / 3600)),
    )
    .unwrap();
    assert!(cache.record(sample(1_800_000_120, Some(1.0))).is_err());
    assert_eq!(fs::read(&external).unwrap(), b"untouched");
    assert_eq!(cache.samples(1_800_000_120, 1_800_000_120, 5).len(), 1);
}

#[test]
fn cache_recovery_rejects_bad_byte_encodings_and_out_of_range_statistics() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("cache");
    let now = 1_800_000_120;
    let mut cache = HistoryStore::open(&directory, now).unwrap();
    cache.record(sample(now, Some(1.0))).unwrap();
    drop(cache);
    let mut file = OpenOptions::new()
        .append(true)
        .open(directory.join(format!("recent-{}.jsonl", now / 3600)))
        .unwrap();
    for bad in [
        json_codec::json!("01"),
        json_codec::json!("-1"),
        json_codec::json!("18446744073709551616"),
        json_codec::json!("bad"),
        json_codec::json!(1),
    ] {
        let mut encoded = json_codec::to_value(sample(now + 5, Some(2.0))).unwrap();
        encoded["status"]["memory"]["used_bytes"] = bad;
        assert!(json_codec::from_value::<Sample>(encoded.clone()).is_err());
        writeln!(file, "{}", json_codec::to_string(&encoded).unwrap()).unwrap();
    }
    // The actual cache codec supports u128 sums; Value deliberately does not.
    let encoded = json_codec::to_string(&average(&[sample(now + 10, Some(3.0))])).unwrap();
    let original = "\"memory/total\":[18446744073709551615,1]";
    assert!(encoded.contains(original));
    let encoded = encoded.replace(original, &format!("\"memory/total\":[{},1]", u128::MAX));
    let poisoned: Sample = json_codec::from_str(&encoded).unwrap();
    assert!(!poisoned.valid_at(now + 10));
    writeln!(file, "{encoded}").unwrap();
    drop(file);
    let cache = HistoryStore::open(&directory, now + 10).unwrap();
    assert_eq!(cache.recent.len(), 1);
    assert_eq!(cache.recent.front().unwrap().unix_seconds, now);
    assert_eq!(
        cache.recent.front().unwrap().status.memory.used_bytes,
        Some(10)
    );
}
