use super::*;
use json_codec::json;

fn observation(freshness: &str) -> Value {
    json!({
        "phase":{"state":"idle"}, "latest":null,
        "last_complete_success_at":"2026-10-27T06:00:00Z", "freshness":freshness
    })
}
fn state() -> RwSignal<State> {
    RwSignal::new(State {
        authenticated: true,
        session_epoch: 7,
        ..Default::default()
    })
}
fn error(status: u16) -> api::Error {
    api::Error {
        code: storage_contract::ErrorCode::MonitorUnavailable,
        message: "unavailable".into(),
        retryable: true,
        status,
        request_id: Some("scrub-test".into()),
        source: None,
    }
}
#[test]
fn canonical_large_progress_survives_without_javascript_number_rounding() {
    let mut value = observation("not_checked");
    value["phase"] = json!({
        "state":"running", "job_id":"9007199254740993", "cancellation_requested":true,
        "baseline":{"sequence":"9007199254740995","snapshot_hash":"exact-baseline"},
        "started_at":"2026-10-27T06:00:00Z",
        "bytes_read":"18446744073709551616", "expected_payload_bytes":"9007199254740997"
    });
    let status: Status = Status::decode(value.clone()).unwrap();
    let Phase::Running {
        job_id,
        bytes_read,
        expected_payload_bytes,
        cancellation_requested,
        ..
    } = status.phase
    else {
        panic!("running");
    };
    assert_eq!(job_id, 9_007_199_254_740_993);
    assert_eq!(bytes_read, 18_446_744_073_709_551_616);
    assert_eq!(expected_payload_bytes, 9_007_199_254_740_997);
    assert!(cancellation_requested);
    for invalid in [
        json!(1),
        json!("01"),
        json!("-1"),
        json!("+1"),
        json!("340282366920938463463374607431768211456"),
    ] {
        value["phase"]["bytes_read"] = invalid;
        assert!(Status::decode(value.clone()).is_err());
    }
}
#[test]
fn unknown_states_and_missing_required_progress_fail_closed() {
    let mut value = observation("green");
    assert!(Status::decode(value.clone()).is_err());
    value["freshness"] = json!("fresh");
    value["phase"] = json!({"state":"running","job_id":"1"});
    assert!(Status::decode(value.clone()).is_err());
    value["phase"] = json!({"state":"maybe_healthy"});
    assert!(Status::decode(value).is_err());
}
#[test]
fn failed_or_malformed_refresh_removes_the_old_green_observation() {
    Owner::new().with(|| {
        let state = state();
        complete(state, 7, Ok(observation("fresh")));
        assert_eq!(
            state.with_untracked(|s| s.monitor.scrub.observation.as_ref().unwrap().freshness),
            Freshness::Fresh
        );
        complete(state, 7, Err(error(503)));
        state.with_untracked(|s| {
            assert!(s.monitor.scrub.observation.is_none());
            assert_eq!(
                s.monitor
                    .scrub
                    .failure
                    .as_ref()
                    .unwrap()
                    .request_id
                    .as_deref(),
                Some("scrub-test")
            );
        });
        complete(state, 7, Ok(observation("fresh")));
        complete(state, 7, Ok(json!({"freshness":"fresh"})));
        state.with_untracked(|s| {
            assert!(s.monitor.scrub.observation.is_none());
            assert!(s.monitor.scrub.failure.is_some());
        });
    });
}
#[test]
fn late_session_response_cannot_restore_old_health_or_revoke_new_login() {
    Owner::new().with(|| {
        let state = state();
        complete(state, 7, Ok(observation("expired")));
        complete(state, 6, Ok(observation("fresh")));
        complete(state, 6, Err(error(401)));
        state.with_untracked(|s| {
            assert!(s.session_active());
            assert_eq!(
                s.monitor.scrub.observation.as_ref().unwrap().freshness,
                Freshness::Expired
            );
        });
        complete(state, 7, Err(error(401)));
        assert!(!state.with_untracked(State::session_active));
    });
}
#[test]
fn cancellation_and_supersession_are_not_presented_as_complete_success() {
    for (completion, expected) in [
        (
            "cancelled",
            t("已取消，非完整成功", "Cancelled, not complete success"),
        ),
        (
            "expired",
            t(
                "基线已过期，非完整成功",
                "Baseline expired, not complete success",
            ),
        ),
    ] {
        let mut value = observation("unavailable");
        value["latest"] = json!({"job_id":"4","trigger":"manual","outcome":{"kind":"report","report":{
            "state":completion,"sequence":"9","snapshot_hash":"baseline",
            "started_at":"2026-10-27T06:00:00Z","finished_at":"2026-10-27T06:01:00Z",
            "payload_copies_required":1,"payload_copies_verified":0,"metadata_failures":0,
            "identity_failures":0,"payload_copies_unavailable":0,"payload_copies_unverified":0
        }}});
        let status: Status = Status::decode(value).unwrap();
        assert!(latest(&status).contains(&expected));
        assert_eq!(
            freshness(Some(&status)),
            t("完整性结论不可用", "Integrity conclusion unavailable")
        );
    }
}

#[test]
fn fresh_without_complete_success_evidence_is_not_a_valid_observation() {
    let mut value = observation("fresh");
    value["last_complete_success_at"] = Value::Null;
    assert!(Status::decode(value).is_err());
}
