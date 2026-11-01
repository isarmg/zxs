//! Shared deployment vectors consumed by auth and the actual server adapter.
pub(super) const ORIGIN_VECTORS: &[(Option<&str>, &str, &str, bool)] = &[
    (None, "192.0.2.20:7400", "http://192.0.2.20:7400", true),
    (None, "STORAGE.EXAMPLE:80", "http://storage.example", true),
    (None, "[2001:0db8::20]:80", "http://[2001:db8::20]", true),
    (None, "0.0.0.0:7400", "http://0.0.0.0:7400", true),
    (
        None,
        "storage.example:80",
        "http://storage.example:80",
        false,
    ),
    (
        None,
        "storage.example:7400",
        "https://storage.example:7400",
        false,
    ),
    (
        None,
        "storage.example:7400",
        "http://evil.example:7400",
        false,
    ),
    (None, "storage.example:7400", "null", false),
    (
        Some("HTTPS://EXAMPLE.COM:443"),
        "127.0.0.1:7400",
        "https://example.com",
        true,
    ),
    (
        Some("https://EXAMPLE.COM:8443"),
        "internal:7400",
        "https://example.com:8443",
        true,
    ),
    (
        Some("https://EXAMPLE.COM:443"),
        "internal:7400",
        "https://example.com:443",
        false,
    ),
    (
        Some("https://EXAMPLE.COM:443"),
        "internal:7400",
        "https://EXAMPLE.COM",
        false,
    ),
    (
        Some("https://EXAMPLE.COM:443"),
        "internal:7400",
        "http://example.com",
        false,
    ),
];
