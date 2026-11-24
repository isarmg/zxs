use super::cancel_prepared_args;
use std::path::Path;

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn cancellation_requires_exact_absolute_local_grammar_not_an_online_account() {
    let op = "abababababababababababababababab";
    let args = argv(&["/private/state", "cancel-prepared", op]);
    let (path, original) = cancel_prepared_args(&args).unwrap().unwrap();
    assert_eq!(path, Path::new("/private/state"));
    assert_eq!(original.as_str(), op);

    for command in ["list", "status", "send", "checkout", "init"] {
        let online = argv(&[
            "https://storage.invalid",
            "cancel-prepared",
            "/private/state",
            command,
        ]);
        assert!(cancel_prepared_args(&online).unwrap().is_none());
    }
    for wrong in [
        argv(&["/private/state", "cancel-prepared"]),
        argv(&["/private/state", "cancel-prepared", "not-an-op"]),
        argv(&["/private/state", "cancel-prepared", op, "extra"]),
        argv(&[
            "https://storage.invalid",
            "owner",
            "/private/state",
            "cancel-prepared",
            op,
        ]),
    ] {
        assert!(cancel_prepared_args(&wrong).is_err());
    }
    let relative = argv(&["private/state", "cancel-prepared", op]);
    assert!(cancel_prepared_args(&relative).unwrap().is_none());
}
