use super::payload_args;
use zxs_core::session::ByteWindow;

fn arguments() -> Vec<String> {
    [
        "--payload-session",
        "https://example.invalid",
        "fixture",
        "/unused-private",
        "slice",
        "/unused-ordinary",
        "98304",
        "512",
        "/unused-output",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

#[test]
fn payload_cli_only_accepts_one_explicit_local_selection_and_span() {
    let exact = arguments();
    let command = payload_args(&exact).unwrap();
    assert_eq!(command.mapped, "/unused-ordinary");
    assert_eq!(command.output, "/unused-output");
    assert_eq!(command.window, ByteWindow::new(98304, 512).unwrap());
    for (index, value) in [
        (0, "--receipt-session"),
        (4, "status"),
        (4, "send"),
        (4, "checkout"),
        (5, "https://other.invalid/zxs/v1/payload"),
        (6, "+1"),
        (6, "18446744073709551615"),
        (7, "0"),
        (7, "8388609"),
    ] {
        let mut wrong = exact.clone();
        wrong[index] = value.to_owned();
        assert!(payload_args(&wrong).is_err());
    }
    let mut wrong = exact;
    wrong.push("extra".to_owned());
    assert!(payload_args(&wrong).is_err());
}
