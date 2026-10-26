use super::*;
use std::error::Error as _;

#[test]
fn all_codes_roundtrip_exact_wire_spelling() {
    let mut codes = std::collections::BTreeSet::new();
    for &code in ErrorCode::ALL {
        assert!(codes.insert(code.as_str()));
        let encoded = json_codec::to_value(code).unwrap();
        assert_eq!(encoded, code.as_str());
        assert_eq!(json_codec::from_value::<ErrorCode>(encoded).unwrap(), code);
    }
    assert!(json_codec::from_str::<ErrorCode>(r#""FUTURE_UNKNOWN_CODE""#).is_err());
}

#[test]
fn concrete_sources_survive_contract_conversion_and_retry_policy() {
    let error = Error::from(std::io::Error::from_raw_os_error(28)).retryable();
    assert_eq!(error.code(), ErrorCode::NoSpace);
    assert!(error.is_retryable());
    assert_eq!(
        error
            .source()
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .unwrap()
            .raw_os_error(),
        Some(28)
    );
    assert!(error.to_string().starts_with("NO_SPACE:"));
    let source = json_codec::from_str::<json_codec::Value>("{").unwrap_err();
    let line = source.line();
    let error = Error::from(source);
    assert_eq!(error.code(), ErrorCode::InvalidJson);
    assert_eq!(
        error
            .source()
            .unwrap()
            .downcast_ref::<json_codec::Error>()
            .unwrap()
            .line(),
        line
    );
    let error = crate::parse_u64_decimal("18446744073709551616").unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidInteger);
    assert!(error.source().unwrap().is::<std::num::ParseIntError>());
    let error = Error::new(ErrorCode::InvalidRequest, "external validation failure");
    assert!(error.source().is_none());
    assert!(!error.is_retryable());
}

#[test]
fn error_codes_accept_only_their_wire_strings() {
    for code in ErrorCode::ALL {
        let encoded = format!("\"{}\"", code.as_str());
        assert_eq!(json_codec::to_string(code).unwrap(), encoded);
        assert_eq!(json_codec::from_str::<ErrorCode>(&encoded).unwrap(), *code);
        let object = format!("{{{}:null}}", encoded);
        assert!(json_codec::from_str::<ErrorCode>(&object).is_err());
    }
    for invalid in ["\"UNRECOGNIZED_ERROR\"", "null", "0", "true", "[]"] {
        assert!(json_codec::from_str::<ErrorCode>(invalid).is_err());
    }
}
