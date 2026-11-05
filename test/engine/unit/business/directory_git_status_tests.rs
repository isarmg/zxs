use super::*;

#[test]
fn status_work_and_text_budgets_fail_instead_of_truncating() {
    let cancellation = HistoryWindowCancellation::new();
    let mut budget = Budget::new(&cancellation);
    budget.work = 1;
    budget.step().unwrap();
    assert_eq!(budget.step().unwrap_err().code(), ErrorCode::ResourceLimit);
    let mut budget = Budget::new(&cancellation);
    budget.text_bytes = 3;
    budget.text("abc").unwrap();
    assert_eq!(
        budget.text("d").unwrap_err().code(),
        ErrorCode::ResourceLimit
    );
}

#[test]
fn cancellation_is_checked_during_each_work_and_text_charge() {
    let cancellation = HistoryWindowCancellation::new();
    let mut budget = Budget::new(&cancellation);
    budget.step().unwrap();
    cancellation.cancel();
    assert_eq!(
        budget.step().unwrap_err().code(),
        ErrorCode::OperationAborted
    );
    assert_eq!(
        budget.text("next").unwrap_err().code(),
        ErrorCode::OperationAborted
    );
}
