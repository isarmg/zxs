use super::*;

#[test]
fn durability_unknown_is_retryable_unknown_and_diagnostics_are_private() {
    let mapped = super::super::wire::error(storage_engine::Error::new(
        storage_contract::ErrorCode::DurabilityUnknown,
        "private /srv/zxs/disk/config.json directory sync failure after publication",
    ));
    assert_eq!(mapped.status, 500);
    assert_eq!(mapped.code, storage_contract::ErrorCode::DurabilityUnknown);
    assert!(mapped.retryable);
    assert!(storage_contract::client_protocol::mutation_outcome_unknown(
        mapped.status
    ));
    assert!(!mapped.message.contains("private"));
    assert!(!mapped.message.contains("/srv/zxs"));
}

#[test]
fn registrations_are_unique_and_alias_targets_have_explicit_status() {
    let mut internal = std::collections::BTreeSet::new();
    let mut public = std::collections::BTreeSet::new();
    for &(code, http) in PUBLIC_STATUS {
        assert!(public.insert(code), "duplicate public code {code}");
        assert!((400..=599).contains(&http));
    }
    for &(source, target) in ALIASES {
        assert!(internal.insert(source), "duplicate alias {source}");
        assert!(public.contains(&target), "unregistered target {target}");
        assert_eq!(public_code(source), target);
    }
}

#[test]
fn wire_mapping_preserves_status_retry_and_diagnostic_policy() {
    for &(code, expected) in PUBLIC_STATUS {
        let mapped =
            super::super::wire::error(storage_engine::Error::new(code, "private diagnostic"));
        assert_eq!(mapped.code, code);
        assert_eq!(mapped.status, expected, "{code}");
        assert_eq!(
            mapped.retryable,
            (expected >= 500 && expected != 501) || expected == 429
        );
        assert_eq!(mapped.message == "private diagnostic", expected != 500);
    }
    for &(internal, public) in ALIASES {
        let mapped = super::super::wire::error(storage_engine::Error::new(internal, "detail"));
        assert_eq!(mapped.code, public);
        assert_eq!(mapped.status, status(public));
    }
    for (code, expected) in [
        (storage_contract::ErrorCode::NotFound, 404),
        (storage_contract::ErrorCode::WorkspaceNotFound, 404),
        (storage_contract::ErrorCode::InjectedFailure, 400),
    ] {
        assert_eq!(status(code), expected);
        assert_eq!(public_code(code), code);
    }
    let error = storage_engine::Error::new(storage_contract::ErrorCode::InjectedFailure, "detail");
    let error = error.retryable();
    assert!(super::super::wire::error(error).retryable);
}
