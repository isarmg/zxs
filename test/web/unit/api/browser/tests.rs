use super::*;

#[test]
fn browser_diagnostics_and_source_chain_preserve_parsed_exception_fields() {
    let failure = BrowserFailure {
        original: JsValue::NULL,
        name: Some("SecurityError".into()),
        message: Some("Blocked by browser policy".into()),
        kind: FailureKind::Request,
    }
    .into_error();
    assert!(
        failure
            .message
            .contains("SecurityError: Blocked by browser policy")
    );
    let source = std::error::Error::source(&failure).unwrap();
    assert!(source.to_string().contains("SecurityError"));
    assert!(
        std::error::Error::source(source)
            .unwrap()
            .to_string()
            .contains("Blocked by browser policy")
    );
    assert_eq!(failure.code, storage_contract::ErrorCode::TransportError);
    assert_eq!(failure.status, 0);
}

#[test]
fn every_browser_failure_keeps_mutations_unknown_even_when_the_request_was_aborted() {
    for kind in [
        FailureKind::Browser,
        FailureKind::Request,
        FailureKind::ReadCancelled,
        FailureKind::Timeout,
    ] {
        let failure = BrowserFailure {
            original: JsValue::NULL,
            name: Some("AbortError".into()),
            message: Some("The operation was aborted".into()),
            kind,
        }
        .into_error();
        assert_eq!(failure.status, 0);
        assert!(failure.mutation_outcome_unknown());
        assert!(failure.clone().with_status(200).mutation_outcome_unknown());
        assert!(failure.with_status(503).mutation_outcome_unknown());
    }
}

#[test]
fn primitive_exception_text_is_kept_without_inventing_a_name() {
    let failure = BrowserFailure {
        original: JsValue::NULL,
        name: None,
        message: Some("literal browser rejection".into()),
        kind: FailureKind::Browser,
    }
    .into_error();
    assert!(failure.message.ends_with(": literal browser rejection"));
}
