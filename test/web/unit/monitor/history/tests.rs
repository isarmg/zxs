use super::*;
use json_codec::{Value, json};

fn fixture(samples: usize, disks: usize) -> Value {
    json!({
        "range":"1h", "from":"2026-01-01T00:00:00Z", "to":"2026-01-01T01:00:00Z", "interval_seconds":10,
        "samples": (0..samples).map(|s| json!({
            "collected_at":format!("2026-01-01T00:{:02}:{:02}Z", s / 6, s % 6 * 10),
            "cpu":{"usage_percent":s % 100, "logical_count":8,"power_watts":null,"power_unavailable_reason":"sampling"},
            "memory":{"used_bytes":"25","total_bytes":"100","available_bytes":"75"},
            "disks":(0..disks).filter(|d| (s+d)%7 != 0).map(|d| json!({
                "observation_failure":null,"id":format!("P{d}"),"name":format!("Disk {d}"), "online":s%3!=0,"read_only":false,"simulation":false,
                "total_bytes":"1000","used_bytes":"300","available_bytes":"600",
                "usage_percent":if s%4==0 {None} else {Some(30.)},
                "power_watts":if s%5==0 {None} else {Some(d as f64)},"power_unavailable_reason":null
            })).collect::<Vec<_>>(), "disk_sampling_unavailable":false
        })).collect::<Vec<_>>()
    })
}

fn old_points(
    value: &Value,
    metric: &Metric,
    dates: &mut usize,
    comparisons: &mut usize,
) -> Vec<(f64, Option<f64>)> {
    value["samples"]
        .as_array()
        .unwrap()
        .iter()
        .map(|sample| {
            *dates += 1;
            let time = date_time::OffsetDateTime::parse(
                sample["collected_at"].as_str().unwrap(),
                &date_time::format_description::well_known::Rfc3339,
            )
            .unwrap()
            .unix_timestamp() as f64;
            let record = match metric {
                Metric::Cpu | Metric::CpuPower => &sample["cpu"],
                Metric::Memory => &sample["memory"],
                Metric::Disk { id, .. } => sample["disks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|disk| {
                        *comparisons += 1;
                        disk["id"].as_str() == Some(id)
                    })
                    .unwrap_or(&Value::Null),
            };
            let ratio = || {
                let used: f64 = record["used_bytes"].as_str()?.parse().unwrap();
                let total: f64 = record["total_bytes"].as_str()?.parse().unwrap();
                (total > 0. && used <= total).then_some(used * 100. / total)
            };
            let v = match metric {
                Metric::Cpu => record["usage_percent"].as_f64(),
                Metric::CpuPower | Metric::Disk { power: true, .. } => {
                    record["power_watts"].as_f64()
                }
                Metric::Memory => ratio(),
                Metric::Disk { .. } => record["usage_percent"].as_f64().or_else(ratio),
            };
            (time, v)
        })
        .collect()
}

#[test]
fn sparse_projection_matches_independent_old_algorithm_with_bounded_structure() {
    for (s, d) in [(0, 0), (6, 0), (60, 8), (360, 128)] {
        let value = fixture(s, d);
        let entries: usize = value["samples"]
            .as_array()
            .unwrap()
            .iter()
            .map(|sample| sample["disks"].as_array().unwrap().len())
            .sum();
        let start = std::time::Instant::now();
        let history = project(value.clone(), "1h").unwrap();
        let projection_elapsed = start.elapsed();
        assert_eq!(history.samples.len(), s);
        assert_eq!(
            history
                .samples
                .iter()
                .map(|sample| sample.disks.len())
                .sum::<usize>(),
            entries
        );
        let mut metrics = vec![Metric::Cpu, Metric::CpuPower, Metric::Memory];
        for id in 0..d {
            for power in [false, true] {
                metrics.push(Metric::Disk {
                    id: format!("P{id}"),
                    name: String::new(),
                    power,
                });
            }
        }
        let (mut dates, mut comparisons) = (0, 0);
        let start = std::time::Instant::now();
        let old: Vec<_> = metrics
            .iter()
            .map(|metric| old_points(&value, metric, &mut dates, &mut comparisons))
            .collect();
        let old_elapsed = start.elapsed();
        let start = std::time::Instant::now();
        let projected: Vec<_> = metrics
            .iter()
            .map(|metric| history.points(metric))
            .collect();
        let charts_elapsed = start.elapsed();
        assert_eq!(projected, old);
        assert_eq!(dates, s * (3 + 2 * d));
        if d > 8 {
            assert!(comparisons > s * d * d / 2);
        }
        eprintln!(
            "S={s} D={d} entries={entries} old_date_parses={dates} old_disk_comparisons={comparisons} projection={projection_elapsed:?} charts={charts_elapsed:?} old_charts={old_elapsed:?}"
        );
    }
}

#[test]
fn required_fields_and_bad_timestamps_are_errors_but_null_sensors_are_gaps() {
    let valid = fixture(2, 2);
    for field in ["range", "from", "to", "interval_seconds", "samples"] {
        let mut value = valid.clone();
        value.as_object_mut().unwrap().remove(field);
        assert!(decode(value, "1h").unwrap_err().source.is_some(), "{field}");
    }
    for (field, value) in [
        ("from", json!("bad")),
        ("to", Value::Null),
        ("interval_seconds", json!(0)),
        ("range", json!("24h")),
        ("extra", json!(1)),
    ] {
        let mut bad = valid.clone();
        bad[field] = value;
        assert!(decode(bad, "1h").is_err());
    }
    for field in [
        "cpu",
        "memory",
        "disks",
        "collected_at",
        "disk_sampling_unavailable",
    ] {
        let mut bad = valid.clone();
        bad["samples"][0].as_object_mut().unwrap().remove(field);
        assert!(decode(bad, "1h").is_err(), "{field}");
    }
    for (field, replacement) in [
        ("usage_percent", json!("oops")),
        ("power_watts", json!(-1)),
        ("logical_count", Value::Null),
    ] {
        let mut bad = valid.clone();
        bad["samples"][0]["cpu"][field] = replacement;
        assert!(decode(bad, "1h").is_err());
    }
    let mut bad = valid.clone();
    bad["samples"][0]["cpu"]
        .as_object_mut()
        .unwrap()
        .remove("power_watts");
    assert!(decode(bad, "1h").is_err());
    let mut bad = valid.clone();
    bad["samples"][0]["memory"]["used_bytes"] = json!(25);
    assert!(decode(bad, "1h").is_err());
    let history = decode(valid, "1h").unwrap();
    assert!(
        history
            .points(&Metric::CpuPower)
            .iter()
            .all(|(_, v)| v.is_none())
    );
}

#[test]
fn equal_response_preserves_snapshot_identity_and_invalid_response_preserves_visible_history() {
    let mut model = super::super::Model::default();
    let value = fixture(2, 2);
    super::super::accept_history_response(&mut model, value.clone(), "1h");
    let first = model.history.clone();
    super::super::accept_history_response(&mut model, value.clone(), "1h");
    assert_eq!(model.history, first);
    super::super::accept_history_response(&mut model, json!({}), "1h");
    assert_eq!(model.history, first);
    assert!(model.history_failure.is_some());
    assert!(model.history_loaded);
    let mut changed = value;
    changed["samples"][0]["cpu"]["usage_percent"] = json!(42);
    super::super::accept_history_response(&mut model, changed, "1h");
    assert_ne!(model.history, first);
    assert!(model.history_failure.is_none());
}

#[test]
fn duplicate_ids_and_invalid_sample_dates_do_not_silently_drop_data() {
    let valid = fixture(2, 2);
    let mut duplicate = valid.clone();
    let disk = duplicate["samples"][0]["disks"][0].clone();
    duplicate["samples"][0]["disks"]
        .as_array_mut()
        .unwrap()
        .push(disk);
    assert!(decode(duplicate, "1h").is_err());
    for time in [
        "invalid",
        "2026-02-30T00:00:00Z",
        "2026-01-01T00:00:00+01:00",
    ] {
        let mut bad = valid.clone();
        bad["samples"][0]["collected_at"] = json!(time);
        assert!(decode(bad, "1h").is_err());
    }
}

#[test]
fn offline_bucket_capacity_and_missing_disk_preserve_old_gap_semantics() {
    let mut value = fixture(2, 2);
    let disk = &mut value["samples"][0]["disks"][0];
    disk["online"] = json!(false);
    disk["usage_percent"] = Value::Null;
    let history = decode(value, "1h").unwrap();
    assert_eq!(
        history.points(&Metric::Disk {
            id: "P1".into(),
            name: String::new(),
            power: false
        })[0]
            .1,
        Some(30.)
    );
    assert_eq!(
        history.points(&Metric::Disk {
            id: "P0".into(),
            name: String::new(),
            power: false
        })[0]
            .1,
        None
    );
}

#[test]
fn server_disk_observation_vector_is_strictly_decoded_and_retained() {
    let bytes =
        include_str!("../../../../../test/server/fixtures/disk-observation-unavailable.json");
    let disk: wire::Disk = json_codec::from_str(bytes).unwrap();
    assert!(matches!(
        disk.observation_failure,
        Some(wire::DiskObservationIssue::Capacity(
            storage_contract::ErrorCode::IoError
        ))
    ));
    let mut response = fixture(1, 0);
    response["samples"][0]["disks"] = json!([json_codec::from_str::<Value>(bytes).unwrap()]);
    let history = project(response, "1h").unwrap();
    assert!(
        history.samples[0].disks["P01"]
            .observation_failure
            .is_some()
    );
    let mut value: Value = json_codec::from_str(bytes).unwrap();
    value["observation_failure"]["source"] = json!("private source");
    assert!(json_codec::from_value::<wire::Disk>(value).is_err());
}
