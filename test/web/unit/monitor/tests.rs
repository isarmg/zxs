use super::*;

fn accept_live_response(model: &mut Model, value: Value) {
    accept_status_response(model, live::decode(value));
}

fn live_response(disks: Vec<Value>) -> Value {
    json_codec::json!({
        "collected_at":"2026-01-01T00:00:00Z",
        "cpu":{"usage_percent":25.0,"logical_count":8,"power_watts":null,"power_unavailable_reason":"sampling"},
        "memory":{"used_bytes":"25","total_bytes":"100","available_bytes":"75"},
        "disks":disks,"disk_sampling_unavailable":false
    })
}
fn live_disk() -> Value {
    json_codec::json!({
        "observation_failure":null,"id":"P01","name":"Disk P01","online":true,"read_only":false,"simulation":false,
        "total_bytes":"1000","used_bytes":"300","available_bytes":"600",
        "usage_percent":30.0,"power_watts":null,"power_unavailable_reason":"sensor_unavailable"
    })
}
fn disk_metric(power: bool) -> Metric {
    Metric::Disk {
        id: "P01".into(),
        name: "Disk P01".into(),
        power,
    }
}

fn empty_history(range: &str) -> Value {
    json_codec::json!({
        "range":range, "from":"2026-01-01T00:00:00Z",
        "to":"2026-01-01T00:01:00Z", "interval_seconds":5, "samples":[]
    })
}

#[test]
fn superseded_history_read_revokes_authentication_without_settling_the_new_read() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            session_epoch: 7,
            ..Default::default()
        });
        state.update(|s| {
            s.monitor.range = "24h".into();
            s.monitor.history_generation = 2;
            s.monitor.history_loading = true;
            accept_history_response(&mut s.monitor, empty_history("24h"), "24h");
        });
        complete_history_load(
            state,
            7,
            1,
            "1min",
            Err(api::Error {
                code: storage_contract::ErrorCode::Unauthorized,
                message: "Session expired".into(),
                status: 401,
                retryable: false,
                request_id: Some("expired-history".into()),
                source: None,
            }),
        );
        state.with_untracked(|s| {
            assert!(!s.session_active());
            assert!(s.monitor.history_loading);
            assert_eq!(s.monitor.history_generation, 2);
            assert!(s.monitor.history.is_some());
            assert_eq!(s.errors.len(), 1);
        });
    });
}

#[test]
fn superseded_history_data_and_prior_session_errors_leave_current_state_unchanged() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            session_epoch: 8,
            ..Default::default()
        });
        state.update(|s| {
            s.monitor.range = "24h".into();
            s.monitor.history_generation = 2;
            s.monitor.history_loading = true;
            accept_history_response(&mut s.monitor, empty_history("24h"), "24h");
        });
        complete_history_load(state, 8, 1, "1min", Ok(json_codec::json!({"stale":true})));
        for (epoch, status) in [(7, 401), (8, 503)] {
            complete_history_load(
                state,
                epoch,
                1,
                "1min",
                Err(api::Error {
                    code: storage_contract::ErrorCode::InjectedFailure,
                    message: "Old request".into(),
                    status,
                    retryable: false,
                    request_id: None,
                    source: None,
                }),
            );
        }
        state.with_untracked(|s| {
            assert!(s.session_active());
            assert!(s.monitor.history_loading);
            assert!(s.monitor.history.is_some());
            assert!(s.monitor.history_failure.is_none());
            assert!(s.errors.is_empty());
        });
    });
}

#[test]
fn missing_or_invalid_samples_are_never_reported_as_zero_usage_or_power() {
    assert_eq!(ratio(None, Some(100)), None);
    assert_eq!(ratio(Some(101), Some(100)), None);
    assert_eq!(ratio(Some(20), Some(100)), Some(20.));
    assert_eq!(ratio(Some(0), Some(0)), None);
    assert_eq!(Metric::Cpu.value(None), None);
    let mut value = live_response(vec![live_disk()]);
    value["cpu"]["usage_percent"] = Value::Null;
    value["cpu"]["power_watts"] = Value::from(0.0);
    value["disks"][0]["id"] = "B01".into();
    let snapshot = live::decode(value.clone()).unwrap();
    assert_eq!(Metric::Cpu.value(Some(&snapshot)), None);
    assert_eq!(Metric::CpuPower.value(Some(&snapshot)), Some(0.));
    assert_eq!(disk_metric(true).value(Some(&snapshot)), None);
    for (record, field, invalid) in [
        ("cpu", "usage_percent", Value::from(101)),
        ("cpu", "usage_percent", Value::from(-1)),
        ("cpu", "power_watts", Value::from(-1)),
    ] {
        let mut invalid_value = value.clone();
        invalid_value[record][field] = invalid;
        assert!(live::decode(invalid_value).is_err());
    }
    for (field, invalid) in [
        ("usage_percent", Value::from(101)),
        ("power_watts", Value::from(-1)),
    ] {
        let mut invalid_value = value.clone();
        invalid_value["disks"][0][field] = invalid;
        assert!(live::decode(invalid_value).is_err());
    }
}

#[test]
fn changing_history_range_rejects_an_earlier_response_even_after_switching_back() {
    let mut model = Model::default();
    assert!(history_matches(&model, 0, "1min"));
    model.range = "24h".into();
    model.history_generation += 1;
    assert!(!history_matches(&model, 0, "1min"));
    model.range = "1min".into();
    model.history_generation += 1;
    assert!(!history_matches(&model, 0, "1min"));
    assert!(history_matches(&model, 2, "1min"));
}

fn valid_capacity_response() -> Value {
    json_codec::json!({
        "global": {
            "records":{"used":"8","limit":"10","remaining":"2"},
            "cold_records":"3",
            "metadata_bytes":{"used":"2048","limit":"4096","remaining":"2048"}
        },
        "principal": {
            "records":{"used":"4","limit":"5","remaining":"1"},
            "cold_records":"2",
            "metadata_bytes":{"used":"1024","limit":"2048","remaining":"1024"}
        },
        "warning":"near_limit", "automatic_retirement":true
    })
}

#[test]
fn capacity_warning_distinguishes_missing_near_and_exhausted_evidence() {
    let mut value: WorkspaceCapacity = json_codec::from_value(valid_capacity_response()).unwrap();
    let missing = capacity_warning(None);
    let near = capacity_warning(Some(&value));
    value.warning = CapacityWarning::Ok;
    let ok = capacity_warning(Some(&value));
    value.warning = CapacityWarning::Exhausted;
    let exhausted = capacity_warning(Some(&value));
    assert_ne!(missing, ok);
    assert_ne!(near, ok);
    assert_ne!(exhausted, near);
    assert!(near.contains("80%"));
}

#[test]
fn capacity_summary_keeps_missing_values_unavailable() {
    let value: WorkspaceCapacity = json_codec::from_value(valid_capacity_response()).unwrap();
    let empty = capacity_summary(None);
    let populated = capacity_summary(Some(&value.principal));
    assert!(!empty.contains("0 / 0"));
    assert!(populated.contains("4 / 5"));
    assert!(populated.contains(": 2 ·"));
    assert!(capacity_summary(Some(&value.global)).contains("8 / 10"));
}

#[test]
fn malformed_capacity_responses_clear_previous_data_instead_of_showing_zero() {
    let mut missing_global = valid_capacity_response();
    missing_global.as_object_mut().unwrap().remove("global");
    let mut cases = vec![Value::Null, missing_global];
    for (field, replacement) in [
        ("warning", Value::from("unexpected")),
        ("automatic_retirement", Value::from("true")),
        ("global", Value::Null),
        ("unknown", Value::from(false)),
    ] {
        let mut value = valid_capacity_response();
        value[field] = replacement;
        cases.push(value);
    }
    for replacement in [
        Value::from(4),
        Value::from("04"),
        Value::from("-1"),
        Value::Null,
    ] {
        let mut value = valid_capacity_response();
        value["principal"]["records"]["used"] = replacement;
        cases.push(value);
    }
    let mut value = valid_capacity_response();
    value["principal"]["metadata_bytes"]["used"] = Value::from(1024);
    cases.push(value);
    for value in cases {
        let mut model = Model::default();
        accept_capacity_response(&mut model, valid_capacity_response());
        assert!(model.capacity.is_some());
        accept_capacity_response(&mut model, value);
        assert!(model.capacity.is_none());
        assert!(model.capacity_failure.is_some());
        assert_ne!(
            capacity_warning(model.capacity.as_ref()),
            capacity_warning(Some(
                &json_codec::from_value(valid_capacity_response()).unwrap()
            ))
        );
        accept_capacity_response(&mut model, valid_capacity_response());
        assert!(model.capacity.is_some());
        assert!(model.capacity_failure.is_none());
    }
}

#[test]
fn valid_zero_capacity_is_not_missing_or_malformed() {
    let mut response = valid_capacity_response();
    response["principal"]["records"]["used"] = Value::from("0");
    response["principal"]["records"]["remaining"] = Value::from("5");
    response["warning"] = Value::from("ok");
    let mut model = Model::default();
    accept_capacity_response(&mut model, response);
    assert!(model.capacity_failure.is_none());
    let value = model.capacity.as_ref().unwrap();
    assert_eq!(value.principal.records.used, 0);
    assert!(capacity_summary(Some(&value.principal)).contains("0 / 5"));
    assert_ne!(capacity_warning(Some(value)), capacity_warning(None));
}

#[test]
fn disk_usage_preserves_physical_used_bytes_when_free_space_is_reserved() {
    let metric = disk_metric(false);
    let mut value = live_response(vec![live_disk()]);
    let snapshot = live::decode(value.clone()).unwrap();
    assert_eq!(metric.value(Some(&snapshot)), Some(30.0));
    assert_eq!(metric.subtitle(Some(&snapshot)), "300 B / 1000 B");
    value["disks"][0]["usage_percent"] = Value::Null;
    let snapshot = live::decode(value.clone()).unwrap();
    assert_eq!(metric.value(Some(&snapshot)), Some(30.0));
    value["disks"][0]["used_bytes"] = Value::Null;
    let snapshot = live::decode(value).unwrap();
    assert_eq!(metric.value(Some(&snapshot)), None);
    assert_eq!(
        metric.subtitle(Some(&snapshot)),
        format!("{} / 1000 B", t("不可用", "Unavailable"))
    );
}

#[test]
fn disk_sampling_notice_distinguishes_unavailable_from_empty_success() {
    assert!(disk_sampling_notice(None).is_none());
    let mut value = live_response(vec![]);
    let snapshot = live::decode(value.clone()).unwrap();
    assert!(disk_sampling_notice(Some(&snapshot)).is_none());
    value["disk_sampling_unavailable"] = Value::Bool(true);
    let snapshot = live::decode(value.clone()).unwrap();
    assert_eq!(
        disk_sampling_notice(Some(&snapshot)),
        Some(t(
            "磁盘采样不可用，无法确认当前磁盘状态。",
            "Disk sampling is unavailable; current disk status cannot be confirmed.",
        ))
    );
    assert_eq!(Metric::Cpu.value(Some(&snapshot)), Some(25.0));
    assert_eq!(Metric::Memory.value(Some(&snapshot)), Some(25.0));
    assert_eq!(
        metrics(Some(&snapshot)),
        vec![Metric::Cpu, Metric::CpuPower, Metric::Memory]
    );
    value["disk_sampling_unavailable"] = Value::Bool(false);
    let snapshot = live::decode(value).unwrap();
    assert!(disk_sampling_notice(Some(&snapshot)).is_none());
}

#[test]
fn cancelled_superseded_history_completion_cannot_clear_current_loading_or_feedback() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            session_epoch: 9,
            ..Default::default()
        });
        state.update(|s| {
            s.monitor.range = "1min".into();
            s.monitor.history_generation = 21;
            s.monitor.history_loading = true;
            s.monitor.history_failure = Some(Feedback {
                message: "current".into(),
                request_id: None,
            });
        });
        // Return to the same range after rapid switching: generation still distinguishes reads.
        for generation in 1..21 {
            complete_history_load(
                state,
                9,
                generation,
                "1min",
                Err(api::error(
                    storage_contract::ErrorCode::TransportError,
                    "The read was cancelled",
                )),
            );
        }
        state.with_untracked(|s| {
            assert!(s.session_active());
            assert!(s.monitor.history_loading);
            assert_eq!(
                s.monitor.history_failure.as_ref().unwrap().message,
                "current"
            );
            assert_eq!(s.monitor.history_generation, 21);
        });
        complete_history_load(state, 9, 21, "1min", Ok(empty_history("1min")));
        state.with_untracked(|s| {
            assert!(!s.monitor.history_loading);
            assert!(s.monitor.history_loaded);
            assert!(s.monitor.history_failure.is_none());
            assert!(s.monitor.history_cancellation.is_none());
        });
    });
}

#[test]
fn disk_capacity_failure_is_visible_without_exposing_source_messages() {
    let record: Value = json_codec::from_str(include_str!(
        "../../../../test/server/fixtures/disk-observation-unavailable.json"
    ))
    .unwrap();
    let snapshot = live_response(vec![record]);
    let metric = Metric::Disk {
        id: "P01".into(),
        name: "Disk".into(),
        power: false,
    };
    let mut model = Model::default();
    accept_live_response(&mut model, snapshot);
    assert!(model.failure.is_none());
    assert!(metric.value(model.snapshot.as_ref()).is_none());
    assert_eq!(
        metric.subtitle(model.snapshot.as_ref()),
        t("容量观测失败", "Capacity observation failed")
    );
}

#[test]
fn invalid_live_observation_classification_clears_stale_snapshot_and_preserves_decode_source() {
    let record: Value = json_codec::from_str(include_str!(
        "../../../../test/server/fixtures/disk-observation-unavailable.json"
    ))
    .unwrap();
    for field in ["stage", "code", "extra"] {
        let mut model = Model::default();
        let valid = live_response(vec![record.clone()]);
        accept_live_response(&mut model, valid.clone());
        assert!(model.failure.is_none());
        let mut invalid = valid;
        invalid["disks"][0]["observation_failure"][field] = Value::String("unknown".into());
        accept_live_response(&mut model, invalid);
        assert!(model.snapshot.is_none());
        assert!(model.failure.is_some());
        assert!(model.failure.as_ref().unwrap().source.is_some());
    }
}

#[test]
fn live_observation_boundary_requires_explicit_classification_and_unique_disk_ids() {
    let valid = live_response(vec![live_disk()]);
    assert!(
        live::decode(valid.clone())
            .unwrap()
            .disk("P01")
            .unwrap()
            .observation_failure
            .is_none()
    );
    let mut missing = valid.clone();
    missing["disks"][0]
        .as_object_mut()
        .unwrap()
        .remove("observation_failure");
    let mut empty = valid.clone();
    empty["disks"][0]["id"] = "".into();
    let duplicate = live_response(vec![live_disk(), live_disk()]);
    for invalid in [missing, empty, duplicate] {
        assert!(live::decode(invalid).is_err());
    }
}

#[test]
fn capacity_decode_failure_retains_source_without_exposing_wire_detail() {
    use std::error::Error;
    let mut model = Model::default();
    let mut value = valid_capacity_response();
    value["warning"] = "private-wire-marker".into();
    accept_capacity_response(&mut model, value);
    let error = model.capacity_failure.as_ref().unwrap();
    assert_eq!(error.code, storage_contract::ErrorCode::InvalidResponse);
    let source = error.source().unwrap();
    assert!(source.to_string().contains("private-wire-marker"));
    assert!(source.source().is_some());
    assert!(!feedback(error).message.contains("private-wire-marker"));
    assert!(model.capacity.is_none());
    accept_capacity_response(&mut model, valid_capacity_response());
    assert!(model.capacity_failure.is_none());
    assert!(model.capacity.is_some());
}

#[test]
fn refresh_is_pending_until_each_status_page_read_settles() {
    let mut state = State::default();
    assert!(!refreshing(&state));
    state.monitor.loading = true;
    state.monitor.history_loading = true;
    state.monitor.capacity_loading = true;
    state.monitor.scrub.loading = true;
    assert!(refreshing(&state));
    state.monitor.loading = false;
    assert!(refreshing(&state));
    state.monitor.history_loading = false;
    assert!(refreshing(&state));
    state.monitor.capacity_loading = false;
    assert!(refreshing(&state));
    state.monitor.scrub.loading = false;
    assert!(!refreshing(&state));
}

#[test]
fn live_telemetry_rejects_partial_unknown_and_legacy_wire_records() {
    let valid = live_response(vec![live_disk()]);
    for path in [vec![], vec!["cpu"], vec!["memory"], vec!["disks", "0"]] {
        let original = match path.as_slice() {
            [] => &valid,
            ["disks", "0"] => &valid["disks"][0],
            [key] => &valid[*key],
            _ => unreachable!(),
        };
        let fields = original.as_object().unwrap().keys().map(String::as_str);
        for field in fields.chain(std::iter::once("unknown")) {
            let mut invalid = valid.clone();
            let record = match path.as_slice() {
                [] => &mut invalid,
                ["disks", "0"] => &mut invalid["disks"][0],
                [key] => &mut invalid[*key],
                _ => unreachable!(),
            };
            if field == "unknown" {
                record[field] = Value::Bool(true);
            } else {
                record.as_object_mut().unwrap().remove(field);
            }
            assert!(live::decode(invalid).is_err(), "{path:?}: {field}");
        }
    }
    for field in [
        "power_unavailable_reason",
        "online",
        "read_only",
        "simulation",
        "name",
    ] {
        let mut invalid = valid.clone();
        invalid["disks"][0][field] = Value::Null;
        if field == "power_unavailable_reason" {
            invalid["disks"][0][field] = "unknown-reason".into();
        }
        assert!(live::decode(invalid).is_err(), "{field}");
    }
    assert!(live::decode(valid["disks"].clone()).is_err());
    assert!(live::decode(Value::Null).is_err());
}

#[test]
fn live_capacity_uses_current_canonical_u64_strings_without_javascript_rounding() {
    for record in ["memory", "disk"] {
        for field in ["used_bytes", "total_bytes", "available_bytes"] {
            for invalid in [
                json_codec::json!(4),
                json_codec::json!("001"),
                json_codec::json!("-1"),
                json_codec::json!("1.5"),
                json_codec::json!(""),
                json_codec::json!("18446744073709551616"),
            ] {
                let mut value = live_response(vec![live_disk()]);
                let target = if record == "disk" {
                    &mut value["disks"][0]
                } else {
                    &mut value[record]
                };
                target[field] = invalid;
                assert!(live::decode(value).is_err(), "{record}.{field}");
            }
        }
    }
    let mut value = live_response(vec![live_disk()]);
    value["disks"][0]["total_bytes"] = u64::MAX.to_string().into();
    value["disks"][0]["used_bytes"] = (u64::MAX - 1).to_string().into();
    let snapshot = live::decode(value).unwrap();
    let disk = snapshot.disk("P01").unwrap();
    assert_eq!(disk.total_bytes, Some(u64::MAX));
    assert_eq!(disk.used_bytes, Some(u64::MAX - 1));
}

#[test]
fn live_snapshot_identity_and_index_follow_one_immutable_response() {
    let mut model = Model::default();
    let disks = (0..512)
        .map(|index| {
            let mut disk = live_disk();
            disk["id"] = format!("P{index}").into();
            disk["name"] = format!("Disk {index}").into();
            disk
        })
        .collect();
    let value = live_response(disks);
    accept_live_response(&mut model, value.clone());
    let original = model.snapshot.clone();
    accept_live_response(&mut model, value.clone());
    assert!(original == model.snapshot);
    let snapshot = model.snapshot.as_ref().unwrap();
    assert_eq!(snapshot.disk("P511").unwrap().name, "Disk 511");
    let metrics = metrics(Some(snapshot));
    assert_eq!(metrics.len(), 3 + 512 * 2);
    assert_eq!(metrics[3].key(), "disk-P0-capacity");
    assert_eq!(metrics.last().unwrap().key(), "disk-P511-power");
    let mut changed = value;
    changed["disks"][511]["id"] = "renamed".into();
    accept_live_response(&mut model, changed);
    assert!(original != model.snapshot);
    let snapshot = model.snapshot.as_ref().unwrap();
    assert!(snapshot.disk("P511").is_none());
    assert!(snapshot.disk("renamed").is_some());
    assert!(original.as_ref().unwrap().disk("P511").is_some());
    let mut invalid = live_response(vec![live_disk()]);
    invalid["cpu"]["power_unavailable_reason"] = "private-wire-marker".into();
    accept_live_response(&mut model, invalid);
    assert!(model.snapshot.is_none());
    assert!(
        model
            .failure
            .as_ref()
            .unwrap()
            .source
            .as_ref()
            .unwrap()
            .to_string()
            .contains("private-wire-marker")
    );
    assert!(
        !model
            .failure
            .as_ref()
            .unwrap()
            .message
            .contains("private-wire-marker")
    );
    accept_live_response(&mut model, live_response(vec![]));
    assert!(model.failure.is_none());
}

#[test]
fn live_disks_keep_offline_and_sensor_states_distinct() {
    let mut value = live_response(vec![live_disk()]);
    value["disks"][0]["online"] = Value::Bool(false);
    value["disks"][0]["power_unavailable_reason"] = "disk_offline".into();
    let snapshot = live::decode(value.clone()).unwrap();
    assert_eq!(disk_metric(false).value(Some(&snapshot)), None);
    assert_eq!(
        disk_metric(false).subtitle(Some(&snapshot)),
        t("离线", "Offline")
    );
    assert_eq!(
        disk_metric(true).subtitle(Some(&snapshot)),
        t("硬盘离线", "Disk is offline")
    );
    value["disks"][0]["power_watts"] = Value::from(0.0);
    let snapshot = live::decode(value).unwrap();
    assert_eq!(disk_metric(true).value(Some(&snapshot)), Some(0.));
    assert_eq!(
        disk_metric(true).subtitle(Some(&snapshot)),
        t("实时功耗 · W", "Live power · W")
    );
}

#[test]
fn live_request_failure_retains_the_previous_snapshot_and_complete_diagnostic() {
    let mut model = Model::default();
    accept_live_response(&mut model, live_response(vec![live_disk()]));
    let previous = model.snapshot.clone();
    let mut error = api::error(
        storage_contract::ErrorCode::InjectedFailure,
        "exact failure",
    );
    error.status = 503;
    error.request_id = Some("live-request".into());
    accept_status_response(&mut model, Err(error));
    assert!(model.snapshot == previous);
    let error = model.failure.as_ref().unwrap();
    assert_eq!(error.code, storage_contract::ErrorCode::InjectedFailure);
    assert_eq!(error.status, 503);
    assert_eq!(error.message, "exact failure");
    assert_eq!(error.request_id.as_deref(), Some("live-request"));
    assert_eq!(feedback(error).request_id.as_deref(), Some("live-request"));
    accept_live_response(&mut model, Value::Null);
    assert!(model.snapshot.is_none());
    let error = model.failure.as_ref().unwrap();
    assert_eq!(error.code, storage_contract::ErrorCode::InvalidResponse);
    assert!(matches!(
        error.source.as_deref(),
        Some(api::ErrorSource::MonitorSnapshot(_))
    ));
    assert!(std::error::Error::source(error).unwrap().source().is_some());
}

#[test]
fn live_power_reasons_require_the_current_string_tokens_not_object_aliases() {
    for reason in [
        "sensor_unavailable",
        "sensor_unreadable",
        "sampling",
        "counter_reset",
        "disk_offline",
        "simulated_disk",
        "sensor_ambiguous",
    ] {
        let mut valid = live_response(vec![live_disk()]);
        valid["cpu"]["power_unavailable_reason"] = reason.into();
        valid["disks"][0]["power_unavailable_reason"] = reason.into();
        assert!(live::decode(valid.clone()).is_ok());
        let mut alias = json_codec::json!({});
        alias[reason] = Value::Null;
        for target in ["cpu", "disk"] {
            let mut invalid = valid.clone();
            let record = if target == "cpu" {
                &mut invalid["cpu"]
            } else {
                &mut invalid["disks"][0]
            };
            record["power_unavailable_reason"] = alias.clone();
            assert!(live::decode(invalid).is_err(), "{target}: {reason}");
        }
    }
}
