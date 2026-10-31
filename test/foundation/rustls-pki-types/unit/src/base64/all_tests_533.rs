#[cfg(all(test, feature = "alloc"))]
mod tests {
    use alloc::vec::Vec;

    use super::*;

    #[test]
    fn decode_test() {
        assert_eq!(
            decode(b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"),
            b"\x00\x10\x83\x10\x51\x87\x20\x92\x8b\x30\xd3\x8f\x41\x14\x93\x51\x55\x97\
              \x61\x96\x9b\x71\xd7\x9f\x82\x18\xa3\x92\x59\xa7\xa2\x9a\xab\xb2\xdb\xaf\
              \xc3\x1c\xb3\xd3\x5d\xb7\xe3\x9e\xbb\xf3\xdf\xbf"
        );
        assert_eq!(decode(b"aGVsbG8="), b"hello");
        assert_eq!(decode(b"aGVsbG8gd29ybGQ="), b"hello world");
        assert_eq!(decode(b"aGVsbG8gd29ybGQh"), b"hello world!");
        assert_eq!(decode(b"////"), b"\xff\xff\xff");
        assert_eq!(decode(b"++++"), b"\xfb\xef\xbe");
        assert_eq!(decode(b"AAAA"), b"\x00\x00\x00");
        assert_eq!(decode(b"AAA="), b"\x00\x00");
        assert_eq!(decode(b"AA=="), b"\x00");

        // like our previous use of rust-base64, we don't require padding
        // if the encoding is otherwise valid given the length
        assert_eq!(decode(b"AAA"), b"\x00\x00");
        assert_eq!(decode(b"AA"), b"\x00");

        assert_eq!(decode(b""), b"");
    }

    #[test]
    fn decode_errors() {
        let mut buf = [0u8; 6];

        // illegal trailing padding
        assert_eq!(
            decode_both(b"A===", &mut buf),
            Err(Error::InvalidTrailingPadding)
        );
        assert_eq!(
            decode_both(b"====", &mut buf),
            Err(Error::InvalidTrailingPadding)
        );
        assert_eq!(
            decode_both(b"A==", &mut buf),
            Err(Error::InvalidTrailingPadding)
        );
        assert_eq!(
            decode_both(b"AA=", &mut buf),
            Err(Error::InvalidTrailingPadding)
        );
        assert_eq!(
            decode_both(b"A", &mut buf),
            Err(Error::InvalidTrailingPadding)
        );

        // padding before final block
        assert_eq!(
            decode_both(b"=AAAAA==", &mut buf),
            Err(Error::PrematurePadding)
        );
        assert_eq!(
            decode_both(b"A=AAAA==", &mut buf),
            Err(Error::PrematurePadding)
        );
        assert_eq!(
            decode_both(b"AA=AAA==", &mut buf),
            Err(Error::PrematurePadding)
        );
        assert_eq!(
            decode_both(b"AAA=AA==", &mut buf),
            Err(Error::PrematurePadding)
        );

        // illegal inputs
        assert_eq!(
            decode_both(b"%AAA", &mut buf),
            Err(Error::InvalidCharacter(b'%'))
        );
        assert_eq!(
            decode_both(b"A%AA", &mut buf),
            Err(Error::InvalidCharacter(b'%'))
        );
        assert_eq!(
            decode_both(b"AA%A", &mut buf),
            Err(Error::InvalidCharacter(b'%'))
        );
        assert_eq!(
            decode_both(b"AAA%", &mut buf),
            Err(Error::InvalidCharacter(b'%'))
        );

        // output sizing
        assert_eq!(decode_both(b"am9lIGJw", &mut [0u8; 7]), Ok(&b"joe bp"[..]));
        assert_eq!(decode_both(b"am9lIGJw", &mut [0u8; 6]), Ok(&b"joe bp"[..]));
        assert_eq!(
            decode_both(b"am9lIGJw", &mut [0u8; 5]),
            Err(Error::InsufficientOutputSpace)
        );
        assert_eq!(
            decode_both(b"am9lIGJw", &mut [0u8; 4]),
            Err(Error::InsufficientOutputSpace)
        );
        assert_eq!(
            decode_both(b"am9lIGJw", &mut [0u8; 3]),
            Err(Error::InsufficientOutputSpace)
        );

        // output sizing is not pessimistic when padding is valid
        assert_eq!(decode_both(b"am9=", &mut [0u8; 2]), Ok(&b"jo"[..]));
        assert_eq!(decode_both(b"am==", &mut [0u8; 1]), Ok(&b"j"[..]));
        assert_eq!(decode_both(b"am9", &mut [0u8; 2]), Ok(&b"jo"[..]));
        assert_eq!(decode_both(b"am", &mut [0u8; 1]), Ok(&b"j"[..]));
    }

    #[test]
    fn check_models() {
        fn u8_broadcast8_model(x: u8) -> u8 {
            match x & 0x80 {
                0x80 => 0xff,
                _ => 0x00,
            }
        }

        fn u8_broadcast16_model(x: u16) -> u8 {
            match x & 0x8000 {
                0x8000 => 0xff,
                _ => 0x00,
            }
        }

        fn u8_nonzero_model(x: u8) -> u8 {
            match x {
                0 => 0xff,
                _ => 0x00,
            }
        }

        fn u8_equals_model(x: u8, y: u8) -> u8 {
            match x == y {
                true => 0xff,
                false => 0x00,
            }
        }

        fn u8_in_range_model(x: u8, y: u8, z: u8) -> u8 {
            match (y..=z).contains(&x) {
                true => 0xff,
                false => 0x00,
            }
        }

        for x in u8::MIN..=u8::MAX {
            assert_eq!(u8_broadcast8(x), u8_broadcast8_model(x));
            assert_eq!(u8_nonzero(x), u8_nonzero_model(x));
            assert_eq!(CodePoint::decode_secret(x), CodePoint::decode_public(x));

            for y in u8::MIN..=u8::MAX {
                assert_eq!(u8_equals(x, y), u8_equals_model(x, y));

                let v = (x as u16) | ((y as u16) << 8);
                assert_eq!(u8_broadcast16(v), u8_broadcast16_model(v));

                for z in y..=u8::MAX {
                    if z - y == 255 {
                        continue;
                    }
                    assert_eq!(u8_in_range(x, y, z), u8_in_range_model(x, y, z));
                }
            }
        }
    }

    #[cfg(all(feature = "std", target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn codepoint_decode_secret_does_not_branch_or_index_on_secret_input() {
        // this is using the same theory as <https://github.com/agl/ctgrind>
        mod sdk {
            include!(env!("ZXS_PKI_VALGRIND_BINDINGS"));
        }

        // SAFETY: this queries the real Valgrind SDK without memory arguments.
        assert_ne!(
            unsafe { sdk::zxs_valgrind_running() },
            0,
            "Valgrind is required"
        );
        let mut input = [b'a'];
        // SAFETY: input is a live initialized array; only the SDK's definedness
        // shadow is changed, not Rust's initialized bytes or their lifetime.
        unsafe { sdk::zxs_valgrind_make_undefined(input.as_mut_ptr().cast(), input.len()) };
        core::hint::black_box(CodePoint::decode_secret(input[0]));
        // SAFETY: restore the same live array after the secret-dependent probe.
        unsafe { sdk::zxs_valgrind_make_defined(input.as_mut_ptr().cast(), input.len()) };
    }

    #[track_caller]
    fn decode(input: &[u8]) -> Vec<u8> {
        let length = decoded_length(input.len());

        let mut v = alloc::vec![0u8; length];
        let used = decode_both(input, &mut v).unwrap().len();
        v.truncate(used);

        v
    }

    fn decode_both<'a>(input: &'_ [u8], output: &'a mut [u8]) -> Result<&'a [u8], Error> {
        let mut output_copy = output.to_vec();
        let r_pub = decode_public(input, &mut output_copy);

        let r_sec = decode_secret(input, output);

        assert_eq!(r_pub, r_sec);

        r_sec
    }
}
