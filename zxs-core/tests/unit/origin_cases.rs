use super::approved_origin;

#[test]
fn malformed_explicit_ports_hosts_and_paths_do_not_fall_back() {
    for spelling in [
        "https://name:70000",
        "https://name:bad",
        "https://name:",
        "https://name:0",
        "https://[name]:443",
        "https://[127.0.0.1]:443",
        "https://name#fragment",
        "https://name/path",
        "https://someone@name",
        "http://name",
        "https://name/?query=1",
    ] {
        assert!(approved_origin(spelling).is_err(), "{spelling}");
    }
    assert_eq!(
        approved_origin("https://ExAmPle.test").unwrap(),
        (
            "example.test".into(),
            443,
            "https://example.test:443".into()
        )
    );
    assert_eq!(
        approved_origin("https://[::1]:8443").unwrap(),
        ("::1".into(), 8443, "https://[::1]:8443".into())
    );
}
