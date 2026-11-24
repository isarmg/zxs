use super::{ReceiptAction, receipt_args};

fn args(extra: &[&str]) -> Vec<String> {
    [
        "--receipt-session",
        "https://example.invalid",
        "fixture",
        "/unused-state",
    ]
    .into_iter()
    .chain(extra.iter().copied())
    .map(str::to_owned)
    .collect()
}

#[test]
fn receipt_flag_is_an_exact_two_verb_opt_in() {
    assert!(matches!(
        receipt_args(&args(&["reconcile"])).unwrap().3,
        ReceiptAction::Reconcile
    ));
    let id = "abcd0123456789abcdef0123456789ab";
    let ReceiptAction::Status(op) = receipt_args(&args(&["status", id])).unwrap().3 else {
        panic!("selected original operation was not preserved")
    };
    assert_eq!(op.as_str(), id);
    for rejected in [
        args(&["init"]),
        args(&["list"]),
        args(&["detail", id]),
        args(&["checkout", id, "main", "/unused"]),
        args(&["send", id]),
        args(&["prepare", "/unused"]),
        args(&["reconcile", id]),
        args(&["status"]),
        args(&["status", "not-an-id"]),
        args(&["status", id, "extra"]),
    ] {
        assert!(receipt_args(&rejected).is_err());
    }
    let mut no_opt_in = args(&["reconcile"]);
    no_opt_in[0] = "legacy".to_owned();
    assert!(receipt_args(&no_opt_in).is_err());
}
