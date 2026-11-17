use super::*;
use zeroize::Zeroize;

#[test]
fn password_input_keeps_exact_bytes_without_imposing_another_policy() {
    for text in ["", " short ", "密码", "a\nb", " leading and trailing "] {
        let encoded = json_codec::to_string(text).unwrap();
        let input: SecretPassword = json_codec::from_str(&encoded).unwrap();
        assert_eq!(input.expose(), text);
        assert_eq!(format!("{input:?}"), "SecretPassword([REDACTED])");
    }
    for invalid in ["null", "123", "true", "[]", "{}"] {
        assert!(json_codec::from_str::<SecretPassword>(invalid).is_err());
    }
}

#[test]
fn ownership_moves_the_original_buffer_and_zeroization_clears_it() {
    let text = "private-owned-password".to_owned();
    let original = text.as_ptr();
    let mut input = SecretPassword::from(text);
    assert_eq!(input.expose().as_ptr(), original);
    input.0.zeroize();
    assert!(input.expose().is_empty());
}
