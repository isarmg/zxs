use super::*;

#[test]
fn superseded_history_read_revokes_authentication_without_settling_the_new_read() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            session_epoch: 7,
            ..Default::default()
        });
        state.update(|s| {
            s.monitor.range = HistoryRange::Day;
            s.monitor.history_generation = 2;
            s.monitor.history_loading = true;
            s.monitor.history = json_codec::json!({"current":"snapshot"}).into();
        });
        complete_history_load(
            state,
            7,
            1,
            HistoryRange::Minute,
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
            assert_eq!(s.monitor.history["current"], "snapshot");
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
            s.monitor.range = HistoryRange::Day;
            s.monitor.history_generation = 2;
            s.monitor.history_loading = true;
            s.monitor.history = json_codec::json!({"current":"snapshot"}).into();
        });
        complete_history_load(
            state,
            8,
            1,
            HistoryRange::Minute,
            Ok(json_codec::json!({"stale":true})),
        );
        for (epoch, status) in [(7, 401), (8, 503)] {
            complete_history_load(
                state,
                epoch,
                1,
                HistoryRange::Minute,
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
            assert_eq!(s.monitor.history["current"], "snapshot");
            assert!(s.monitor.history_failure.is_none());
            assert!(s.errors.is_empty());
        });
    });
}

#[test]
fn missing_or_invalid_samples_are_never_reported_as_zero_usage_or_power() {
    assert_eq!(percent(&Value::Null), None);
    assert_eq!(ratio(&Value::Null, &Value::from(100)), None);
    assert_eq!(ratio(&Value::from(101), &Value::from(100)), None);
    assert_eq!(ratio(&Value::from(20), &Value::from(100)), Some(20.));
    assert_eq!(power(&Value::Null), None);
    assert_eq!(power(&Value::from(-1)), None);
    assert_eq!(
        Metric::CpuPower.value(&json_codec::json!({"cpu":{"power_watts":0.0}})),
        Some(0.)
    );
    assert_eq!(
        Metric::Disk {
            id: "P01".into(),
            name: "P01".into(),
            power: true
        }
        .value(&json_codec::json!({"disks":[{"id":"B01","power_watts":12.0}]})),
        None
    );
}
#[test]
fn changing_history_range_rejects_an_earlier_response_even_after_switching_back() {
    let mut model = Model::default();
    assert!(history_matches(&model, 0, HistoryRange::Minute));
    model.replace_history_range(HistoryRange::Day);
    assert!(!history_matches(&model, 0, HistoryRange::Minute));
    model.replace_history_range(HistoryRange::Minute);
    assert!(!history_matches(&model, 0, HistoryRange::Minute));
    assert!(history_matches(&model, 2, HistoryRange::Minute));
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
    let metric = Metric::Disk {
        id: "P01".into(),
        name: "P01".into(),
        power: false,
    };
    let mut snapshot = json_codec::json!({"disks":[{
        "id":"P01", "online":true,
        "total_bytes":"1000", "available_bytes":"600", "used_bytes":"300",
        "usage_percent":30.0
    }]});
    assert_eq!(metric.value(&snapshot), Some(30.0));
    assert_eq!(metric.subtitle(&snapshot), "300 B / 1000 B");
    snapshot["disks"][0]["usage_percent"] = Value::Null;
    assert_eq!(metric.value(&snapshot), Some(30.0));
    snapshot["disks"][0]["used_bytes"] = Value::Null;
    assert_eq!(metric.value(&snapshot), None);
    assert_eq!(
        metric.subtitle(&snapshot),
        format!("{} / 1000 B", t("不可用", "Unavailable"))
    );
}

#[test]
fn disk_sampling_notice_distinguishes_unavailable_from_empty_success() {
    assert!(disk_sampling_notice(&Value::Null).is_none());
    let mut snapshot = json_codec::json!({
        "cpu":{"usage_percent":25.0},
        "memory":{"used_bytes":"25","total_bytes":"100"},
        "disks":[], "disk_sampling_unavailable":false
    });
    assert!(disk_sampling_notice(&snapshot).is_none());
    snapshot["disk_sampling_unavailable"] = Value::Bool(true);
    assert_eq!(
        disk_sampling_notice(&snapshot),
        Some(t(
            "磁盘采样不可用，无法确认当前磁盘状态。",
            "Disk sampling is unavailable; current disk status cannot be confirmed.",
        ))
    );
    assert_eq!(Metric::Cpu.value(&snapshot), Some(25.0));
    assert_eq!(Metric::Memory.value(&snapshot), Some(25.0));
    assert_eq!(
        metrics(&snapshot),
        vec![Metric::Cpu, Metric::CpuPower, Metric::Memory]
    );
    snapshot["disk_sampling_unavailable"] = Value::Bool(false);
    assert!(disk_sampling_notice(&snapshot).is_none());
}

#[test]
fn replacing_history_range_resets_only_the_history_read() {
    let mut model = Model {
        history_loading: true,
        history_loaded: true,
        history: json_codec::json!({"old":true}).into(),
        history_failure: Some(Feedback {
            message: "Old read failed".into(),
            request_id: None,
        }),
        loading: true,
        capacity_loading: true,
        snapshot: json_codec::json!({"current":true}),
        ..Default::default()
    };
    model.replace_history_range(HistoryRange::Day);
    assert_eq!(model.range, HistoryRange::Day);
    assert_eq!(model.history_generation, 1);
    assert!(!model.history_loading);
    assert!(!model.history_loaded);
    assert!(model.history.is_null());
    assert!(model.history_failure.is_none());
    assert!(model.loading);
    assert!(model.capacity_loading);
    assert_eq!(model.snapshot["current"], true);
}

#[test]
fn current_history_response_settles_after_an_old_read_was_replaced() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            session_epoch: 8,
            ..Default::default()
        });
        state.update(|s| {
            s.monitor.replace_history_range(HistoryRange::Day);
            s.monitor.history_loading = true;
        });
        complete_history_load(
            state,
            8,
            0,
            HistoryRange::Minute,
            Err(api::Error {
                code: storage_contract::ErrorCode::TransportError,
                message: "Replaced read".into(),
                status: 0,
                retryable: false,
                request_id: None,
                source: None,
            }),
        );
        assert!(state.with_untracked(|s| s.monitor.history_loading));
        complete_history_load(
            state,
            8,
            1,
            HistoryRange::Day,
            Ok(json_codec::json!({"current":true})),
        );
        state.with_untracked(|s| {
            assert!(!s.monitor.history_loading);
            assert!(s.monitor.history_loaded);
            assert_eq!(s.monitor.history["current"], true);
            assert!(s.monitor.history_failure.is_none());
            assert!(s.errors.is_empty());
        });
    });
}

#[test]
fn history_range_parses_only_supported_wire_labels() {
    let expected = ["1min", "30min", "1h", "24h", "1month"];
    assert_eq!(HistoryRange::ALL.map(HistoryRange::as_str), expected);
    for range in HistoryRange::ALL {
        assert_eq!(HistoryRange::parse(range.as_str()), Some(range));
    }
    for invalid in ["", "1MIN", " 1min", "1min ", "60min", "1year"] {
        assert_eq!(HistoryRange::parse(invalid), None);
    }
    assert_eq!(Model::default().range, HistoryRange::Minute);
}
