use super::*;
use zeroize::Zeroize;

#[test]
fn admitted_secret_moves_its_owned_buffer_and_the_bearer_prefix_uses_erasable_scratch() {
    let value = String::from("private synthetic credential");
    let original = value.as_ptr();
    let mut secret: SessionSecret = json_codec::from_value(Value::String(value)).unwrap();
    assert_eq!(secret.expose().as_ptr(), original);
    let mut header = secret.bearer_header();
    assert_eq!(header.as_str(), "Bearer private synthetic credential");
    header.zeroize();
    assert!(header.is_empty());
    assert_eq!(secret.expose(), "private synthetic credential");
    secret.0.zeroize();
    assert!(secret.expose().is_empty());
}

#[test]
fn header_failures_only_describe_the_standard_browser_class_and_header_name() {
    for (header, cause, expected) in [
        (
            Header::Authorization,
            HeaderCause::TypeError,
            "browser rejected the Authorization header (TypeError)",
        ),
        (
            Header::Csrf,
            HeaderCause::DomException(18),
            "browser rejected the X-CSRF-Token header (DOM error 18)",
        ),
        (
            Header::Authorization,
            HeaderCause::Other,
            "browser rejected the Authorization header",
        ),
    ] {
        let failure = HeaderFailure { header, cause };
        assert_eq!(failure.to_string(), expected);
        assert!(std::error::Error::source(&failure).is_none());
    }
}
