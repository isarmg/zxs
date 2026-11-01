#[cfg(test)]
mod test {
    use super::*;
    // Upstream property generator unavailable in the maintained offline graph.
    #[cfg(any())]
    use proptest::prelude::*;

    struct ShortWriter(Vec<u8>);

    impl std::io::Write for ShortWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let written = bytes.len().min(1);
            self.0.extend_from_slice(&bytes[..written]);
            Ok(written)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn strip_after_short_write(input: &[u8], completion: &[u8]) -> Vec<u8> {
        let mut output = ShortWriter(Vec::new());
        let mut state = StripBytes::new();
        let accepted = write(&mut output, &mut state, input).unwrap();
        assert_eq!(accepted, 1);
        write_all(&mut output, &mut state, &input[accepted..]).unwrap();
        write_all(&mut output, &mut state, completion).unwrap();
        output.0
    }

    #[test]
    fn short_write_replays_only_the_consumed_prefix() {
        assert_eq!(strip_after_short_write(b"ab\x1b[", b"31mcd"), b"abcd");
    }

    #[test]
    fn short_write_preserves_partial_utf8_before_escape() {
        assert_eq!(
            strip_after_short_write("éx\x1b[".as_bytes(), b"31mafter"),
            "éxafter".as_bytes()
        );
    }

    #[cfg(any())]
    proptest! {
        #[test]
        #[cfg_attr(miri, ignore)]  // See https://github.com/AltSysrq/proptest/issues/253
        fn write_all_no_escapes(s in "\\PC*") {
            let buffer = Vec::new();
            let mut stream = StripStream::new(buffer);
            stream.write_all(s.as_bytes()).unwrap();
            let buffer = stream.into_inner();
            let actual = std::str::from_utf8(buffer.as_ref()).unwrap();
            assert_eq!(s, actual);
        }

        #[test]
        #[cfg_attr(miri, ignore)]  // See https://github.com/AltSysrq/proptest/issues/253
        fn write_byte_no_escapes(s in "\\PC*") {
            let buffer = Vec::new();
            let mut stream = StripStream::new(buffer);
            for byte in s.as_bytes() {
                stream.write_all(&[*byte]).unwrap();
            }
            let buffer = stream.into_inner();
            let actual = std::str::from_utf8(buffer.as_ref()).unwrap();
            assert_eq!(s, actual);
        }

        #[test]
        #[cfg_attr(miri, ignore)]  // See https://github.com/AltSysrq/proptest/issues/253
        fn write_all_random(s in any::<Vec<u8>>()) {
            let buffer = Vec::new();
            let mut stream = StripStream::new(buffer);
            stream.write_all(s.as_slice()).unwrap();
            let buffer = stream.into_inner();
            if let Ok(actual) = std::str::from_utf8(buffer.as_ref()) {
                for char in actual.chars() {
                    assert!(!char.is_ascii() || !char.is_control() || char.is_ascii_whitespace(), "{:?} -> {:?}: {:?}", String::from_utf8_lossy(&s), actual, char);
                }
            }
        }

        #[test]
        #[cfg_attr(miri, ignore)]  // See https://github.com/AltSysrq/proptest/issues/253
        fn write_byte_random(s in any::<Vec<u8>>()) {
            let buffer = Vec::new();
            let mut stream = StripStream::new(buffer);
            for byte in s.as_slice() {
                stream.write_all(&[*byte]).unwrap();
            }
            let buffer = stream.into_inner();
            if let Ok(actual) = std::str::from_utf8(buffer.as_ref()) {
                for char in actual.chars() {
                    assert!(!char.is_ascii() || !char.is_control() || char.is_ascii_whitespace(), "{:?} -> {:?}: {:?}", String::from_utf8_lossy(&s), actual, char);
                }
            }
        }
    }
}
