#[cfg(test)]
mod test {
    use super::*;
    // Upstream property generator unavailable in the maintained offline graph.
    #[cfg(any())]
    use proptest::prelude::*;

    #[test]
    fn incremental_stripping_preserves_control_state_at_every_boundary() {
        for (input, expected) in [
            ("\x1b[31mred\x1b[0m", "red"),
            ("\x1b]0;title\x07text", "text"),
            ("\x1bPpayload\x1b\\text", "text"),
            ("é\x1b[32m中\x1b[0m🙂", "é中🙂"),
        ] {
            for split in input.char_indices().map(|(i, _)| i).chain([input.len()]) {
                let mut state = StripStr::new();
                let mut actual = String::new();
                for chunk in [&input[..split], &input[split..]] {
                    for printable in state.strip_next(chunk) {
                        actual.push_str(printable);
                    }
                }
                assert_eq!(actual, expected, "input={input:?}, split={split}");
            }
        }
    }

    /// Model based off full parser
    fn parser_strip(bytes: &[u8]) -> String {
        #[derive(Default)]
        struct Strip(String);
        impl Strip {
            fn with_capacity(capacity: usize) -> Self {
                Self(String::with_capacity(capacity))
            }
        }
        impl crate::parser::Perform for Strip {
            fn print(&mut self, c: char) {
                self.0.push(c);
            }

            fn execute(&mut self, byte: u8) {
                if byte.is_ascii_whitespace() {
                    self.0.push(byte as char);
                }
            }
        }

        let mut stripped = Strip::with_capacity(bytes.len());
        let mut parser = crate::parser::Parser::<crate::parser::DefaultCharAccumulator>::new();
        for byte in bytes {
            parser.advance(&mut stripped, *byte);
        }
        stripped.0
    }

    /// Model verifying incremental parsing
    fn strip_char(mut s: &str) -> String {
        let mut result = String::new();
        let mut state = StripStr::new();
        while !s.is_empty() {
            let mut indices = s.char_indices();
            indices.next(); // current
            let offset = indices.next().map(|(i, _)| i).unwrap_or_else(|| s.len());
            let (current, remainder) = s.split_at(offset);
            for printable in state.strip_next(current) {
                result.push_str(printable);
            }
            s = remainder;
        }
        result
    }

    /// Model verifying incremental parsing
    fn strip_byte(s: &[u8]) -> Vec<u8> {
        let mut result = Vec::new();
        let mut state = StripBytes::default();
        for start in 0..s.len() {
            let current = &s[start..=start];
            for printable in state.strip_next(current) {
                result.extend(printable);
            }
        }
        result
    }

    #[test]
    fn test_strip_bytes_multibyte() {
        let bytes = [240, 145, 141, 139];
        let expected = parser_strip(&bytes);
        let actual = String::from_utf8(strip_bytes(&bytes).into_vec()).unwrap();
        assert_eq!(expected, actual);
    }

    #[test]
    fn test_strip_byte_multibyte() {
        let bytes = [240, 145, 141, 139];
        let expected = parser_strip(&bytes);
        let actual = String::from_utf8(strip_byte(&bytes).clone()).unwrap();
        assert_eq!(expected, actual);
    }

    #[test]
    fn test_strip_str_del() {
        let input = std::str::from_utf8(&[0x7f]).unwrap();
        let expected = "";
        let actual = strip_str(input).to_string();
        assert_eq!(expected, actual);
    }

    #[test]
    fn test_strip_byte_del() {
        let bytes = [0x7f];
        let expected = "";
        let actual = String::from_utf8(strip_byte(&bytes).clone()).unwrap();
        assert_eq!(expected, actual);
    }

    #[test]
    fn test_strip_str_handles_broken_sequence() {
        // valid utf8: \xc3\xb6 then \x1b then \xf0\x9f\x98\x80
        let s = "ö\x1b😀hello😀goodbye";
        let mut it = strip_str(s);
        assert_eq!("ö", it.next().unwrap());
        assert_eq!("ello😀goodbye", it.next().unwrap());
    }

    #[cfg(any())]
    proptest! {
        #[test]
        #[cfg_attr(miri, ignore)]  // See https://github.com/AltSysrq/proptest/issues/253
        fn strip_str_no_escapes(s in "\\PC*") {
            let expected = parser_strip(s.as_bytes());
            let actual = strip_str(&s).to_string();
            assert_eq!(expected, actual);
        }

        #[test]
        #[cfg_attr(miri, ignore)]  // See https://github.com/AltSysrq/proptest/issues/253
        fn strip_char_no_escapes(s in "\\PC*") {
            let expected = parser_strip(s.as_bytes());
            let actual = strip_char(&s);
            assert_eq!(expected, actual);
        }

        #[test]
        #[cfg_attr(miri, ignore)]  // See https://github.com/AltSysrq/proptest/issues/253
        fn strip_bytes_no_escapes(s in "\\PC*") {
            dbg!(&s);
            dbg!(s.as_bytes());
            let expected = parser_strip(s.as_bytes());
            let actual = String::from_utf8(strip_bytes(s.as_bytes()).into_vec()).unwrap();
            assert_eq!(expected, actual);
        }

        #[test]
        #[cfg_attr(miri, ignore)]  // See https://github.com/AltSysrq/proptest/issues/253
        fn strip_byte_no_escapes(s in "\\PC*") {
            dbg!(&s);
            dbg!(s.as_bytes());
            let expected = parser_strip(s.as_bytes());
            let actual = String::from_utf8(strip_byte(s.as_bytes()).clone()).unwrap();
            assert_eq!(expected, actual);
        }
    }
}
