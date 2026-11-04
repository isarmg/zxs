use super::*;

#[test]
fn deregistration_projection_error_preserves_code_detail_and_typed_source() {
    let source = storage_engine::DeregistrationPlanError::CurrentNamespaceUnavailable;
    let detail = source.to_string();
    let mapped = deregistration_plan_error(source);
    assert_eq!(mapped.code, storage_contract::ErrorCode::RecoveryRequired);
    assert_eq!(mapped.status, http::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(mapped.message, detail);
    assert!(matches!(
        *mapped.source.unwrap(),
        crate::http_boundary::ApiErrorSource::DeregistrationPlan(
            storage_engine::DeregistrationPlanError::CurrentNamespaceUnavailable
        )
    ));

    let missing = deregistration_plan_error(
        storage_engine::DeregistrationPlanError::TargetNotRegistered("P01".into()),
    );
    assert_eq!(missing.code, storage_contract::ErrorCode::NotFound);
    assert_eq!(missing.status, http::StatusCode::NOT_FOUND);
    assert!(missing.message.contains("P01"));
    let corrupt = deregistration_plan_error(
        storage_engine::DeregistrationPlanError::PlacementMissing("ver_abc".into()),
    );
    assert_eq!(corrupt.code, storage_contract::ErrorCode::ObjectCorrupt);
    assert!(std::error::Error::source(&corrupt).is_some());
}
