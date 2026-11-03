use regex::automata::{
    dfa::{dense, Automaton},
    Input, MatchError,
};

fn dfa_with_quit_bytes(quit_bytes: &[u8]) -> dense::DFA<Vec<u32>> {
    let config = quit_bytes
        .iter()
        .fold(dense::Config::new(), |config, &byte| {
            config.quit(byte, true)
        });
    dense::Builder::new().configure(config).build("x").unwrap()
}

#[test]
fn asymmetric_quit_bytes_survive_serialization() {
    let quit_bytes = [0, 5, 127, 129, 200, 254];
    let original = dfa_with_quit_bytes(&quit_bytes);
    let (bytes, padding) = original.to_bytes_native_endian();
    let (decoded, consumed) = dense::DFA::from_bytes(&bytes).unwrap();
    assert_eq!(consumed, bytes.len());

    let (rewritten, rewritten_padding) = decoded.to_bytes_native_endian();
    assert_eq!(&bytes[padding..], &rewritten[rewritten_padding..]);

    for byte in 0..=255 {
        let haystack = [byte, b'x'];
        let input = Input::new(&haystack).span(1..2);
        let expected = original.try_search_fwd(&input);
        if quit_bytes.contains(&byte) {
            assert_eq!(expected, Err(MatchError::quit(byte, 0)));
        } else {
            assert!(expected.as_ref().unwrap().is_some());
        }
        assert_eq!(decoded.try_search_fwd(&input), expected, "byte {byte}");
    }
}

#[test]
fn quit_byte_bucket_boundaries_survive_serialization() {
    for byte in [0, 127, 128, 255] {
        let original = dfa_with_quit_bytes(&[byte]);
        let (bytes, padding) = original.to_bytes_native_endian();
        let (decoded, consumed) = dense::DFA::from_bytes(&bytes).unwrap();
        assert_eq!(consumed, bytes.len());
        let (rewritten, rewritten_padding) = decoded.to_bytes_native_endian();
        assert_eq!(
            &bytes[padding..],
            &rewritten[rewritten_padding..],
            "singleton quit byte {byte}",
        );
    }
}

#[test]
fn truncated_quit_byte_set_preserves_error() {
    let (bytes, _) = dfa_with_quit_bytes(&[0, 255]).to_bytes_native_endian();
    // The quit set is the final two native-endian u128 buckets in this format.
    let prefix_len = bytes.len() - 32;
    for retained_bytes in 0..32 {
        let error = dense::DFA::from_bytes(&bytes[..prefix_len + retained_bytes]).unwrap_err();
        assert_eq!(error.to_string(), "buffer is too small to read byte set");
    }
}

#[test]
fn quit_byte_set_consumes_only_its_32_bytes() {
    for quit_bytes in [Vec::new(), (0..=255).collect()] {
        let original = dfa_with_quit_bytes(&quit_bytes);
        let (mut bytes, padding) = original.to_bytes_native_endian();
        let serialized_len = bytes.len();
        bytes.extend_from_slice(&[0xA5; 19]);
        let (decoded, consumed) = dense::DFA::from_bytes(&bytes).unwrap();
        assert_eq!(consumed, serialized_len);
        assert_eq!(&bytes[consumed..], &[0xA5; 19]);
        let (rewritten, rewritten_padding) = decoded.to_bytes_native_endian();
        assert_eq!(&bytes[padding..consumed], &rewritten[rewritten_padding..],);
    }
}
