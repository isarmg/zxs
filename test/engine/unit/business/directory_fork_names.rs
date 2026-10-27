use super::fork_name;
#[test]
fn generated_fork_labels_preserve_portable_nfc_and_utf8_limits() {
    // A collision suffix can cut before a multibyte scalar or after an inner dot/space.
    for origin in [
        "文".repeat(42),
        format!("{}ab", "文".repeat(42)),
        format!("{} x", "x".repeat(125)),
        format!("{}.x", "x".repeat(125)),
        format!("{}é", "x".repeat(126)),
        "COM10".into(),
        "Main".into(),
        "é".into(),
    ] {
        storage_contract::BranchName::parse(&origin).unwrap();
        for number in [0, 1, 9, 10, 99, 100, u64::MAX] {
            let (name, key) = fork_name(&origin, number);
            let parsed = storage_contract::BranchName::parse(&name).unwrap();
            assert_eq!(parsed.key(), key);
            assert!(name.len() <= 128);
            if number == 0 {
                assert_eq!(name, origin);
            } else {
                assert!(name.ends_with(&format!("-{number}")));
                assert_ne!(key, "main");
            }
        }
    }
    assert_eq!(
        fork_name(&"文".repeat(42), 1).0,
        format!("{}-1", "文".repeat(42))
    );
    assert_eq!(
        fork_name(&format!("{}ab", "文".repeat(42)), 10).0,
        format!("{}-10", "文".repeat(41))
    );
}
