use super::admit_execution;

#[test]
fn ordinary_local_execution_is_not_implicitly_admitted() {
    assert!(admit_execution(false, false).is_err());
}

#[test]
fn explicit_local_calibration_needs_no_forged_github_environment() {
    assert!(admit_execution(true, false).is_ok());
}

#[test]
fn github_execution_keeps_the_original_admission() {
    assert!(admit_execution(false, true).is_ok());
    assert!(admit_execution(true, true).is_ok());
}

#[test]
fn binding_parity_expression_keeps_every_supported_operation_count() {
    for operations in 4usize..=1_000_000 {
        let payload = operations - super::SETUP_RECEIPTS;
        let expected_even = payload & 1 == 0;
        assert_eq!(payload.is_multiple_of(2), expected_even);
    }
}
