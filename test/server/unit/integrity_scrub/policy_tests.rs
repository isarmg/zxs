use super::*;
fn positive(value: u64) -> NonZeroU64 {
    NonZeroU64::new(value).unwrap()
}
#[test]
fn periodic_policy_rejects_incoherent_freshness_once_at_host_admission() {
    let error =
        IntegrityScrubPolicy::periodic(positive(2), positive(1), positive(1024)).unwrap_err();
    assert_eq!(error.interval_seconds, 2);
    assert_eq!(error.max_age_seconds, 1);
    let policy = IntegrityScrubPolicy::periodic(positive(2), positive(2), positive(1024)).unwrap();
    assert_eq!(policy.interval(), Some(Duration::from_secs(2)));
    assert_eq!(policy.max_age(), Duration::from_secs(2));
}
#[test]
fn read_budget_uses_actual_bytes_with_fractional_rounding_and_no_lifetime_overflow() {
    let policy = IntegrityScrubPolicy::manual(positive(60), positive(3));
    assert_eq!(policy.interval(), None);
    assert_eq!(policy.read_duration(0), Duration::ZERO);
    assert_eq!(policy.read_duration(1), Duration::from_nanos(333_333_334));
    assert_eq!(policy.read_duration(3), Duration::from_secs(1));
    let slow = IntegrityScrubPolicy::manual(positive(60), positive(1));
    assert_eq!(
        slow.read_duration(usize::MAX),
        Duration::from_secs(usize::MAX as u64)
    );
}

#[test]
fn terminal_feasibility_only_disproves_a_payload_lower_bound() {
    let mut diagnostics = IntegrityScrubDiagnostics {
        bytes_read: 0,
        scan_elapsed: None,
        expected_payload_bytes: None,
        policy: IntegrityScrubPolicy::periodic(positive(1), positive(2), positive(3)).unwrap(),
        metadata_coverage_at_finish: None,
    };
    assert_eq!(
        diagnostics.feasibility(),
        IntegrityScrubFeasibility::UnknownScale
    );
    diagnostics.expected_payload_bytes = Some(5);
    assert_eq!(
        diagnostics.feasibility(),
        IntegrityScrubFeasibility::UnknownTotal
    );
    diagnostics.expected_payload_bytes = Some(6);
    assert_eq!(
        diagnostics.feasibility(),
        IntegrityScrubFeasibility::PayloadCannotFitMaxAge
    );
    diagnostics.expected_payload_bytes = Some(7);
    assert_eq!(
        diagnostics.feasibility(),
        IntegrityScrubFeasibility::PayloadCannotFitMaxAge
    );
    diagnostics.policy = IntegrityScrubPolicy::manual(positive(u64::MAX), positive(u64::MAX));
    diagnostics.expected_payload_bytes = Some(u64::MAX);
    assert_eq!(
        diagnostics.feasibility(),
        IntegrityScrubFeasibility::UnknownTotal
    );
}

#[test]
fn terminal_diagnostics_wire_preserves_wide_counts_and_unknown_observations() {
    let diagnostics = IntegrityScrubDiagnostics {
        bytes_read: u128::from(u64::MAX) + 1,
        scan_elapsed: Some(Duration::from_millis(1234)),
        expected_payload_bytes: None,
        policy: IntegrityScrubPolicy::manual(positive(60), positive(3)),
        metadata_coverage_at_finish: None,
    };
    let status = IntegrityScrubStatus {
        phase: IntegrityScrubPhase::Idle,
        latest: Some(Arc::new(IntegrityScrubRun {
            job_id: IntegrityScrubJobId(1),
            trigger: IntegrityScrubTrigger::Manual,
            outcome: IntegrityScrubOutcome::Rejected(crate::ApiError::limit("fixture")),
            diagnostics,
        })),
        last_complete_success_at: None,
        freshness: IntegrityScrubFreshness::Unavailable,
    };
    let value = json_codec::to_value(StatusResponse::from(status)).unwrap();
    let diagnostics = &value["latest"]["diagnostics"];
    assert_eq!(diagnostics["bytes_read"], "18446744073709551616");
    assert_eq!(diagnostics["scan_elapsed_millis"], "1234");
    assert!(diagnostics["expected_payload_bytes"].is_null());
    assert!(diagnostics["metadata_coverage_at_finish"].is_null());
    assert!(diagnostics["policy"]["interval_seconds"].is_null());
    assert_eq!(diagnostics["policy"]["max_age_seconds"], "60");
    assert_eq!(diagnostics["policy"]["bytes_per_second"], "3");
    assert_eq!(diagnostics["feasibility"], "unknown_scale");
    assert_eq!(value["latest"]["outcome"]["kind"], "rejected");
}

#[test]
fn retained_report_error_projection_preserves_http_redaction() {
    let source = storage_engine::Error::new(
        storage_engine::ErrorCode::IoError,
        "/private/system/metadata: native diagnostic",
    );
    let response = crate::engine_backend::error_response(&source);
    let wire = json_codec::to_string(&response).unwrap();
    assert!(!wire.contains("/private"));
    assert!(!wire.contains("native diagnostic"));
    assert!(source.to_string().contains("native diagnostic"));
}

#[test]
fn http_success_time_is_utc_text_and_not_a_browser_local_clock() {
    let response = StatusResponse::from(IntegrityScrubStatus {
        phase: IntegrityScrubPhase::Idle,
        latest: None,
        last_complete_success_at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
        freshness: IntegrityScrubFreshness::Expired,
    });
    let value = json_codec::to_value(response).unwrap();
    assert_eq!(value["last_complete_success_at"], "1970-01-01T00:00:01Z");
}
