#[test]
fn hex_strtof() {
    macro_rules! f {
            ($a:tt) => (f!(@mk $a, None, None));
            ($a:tt p $e:tt) => (f!(@mk $a, None, Some($e.into())));
            ($a:tt . $b:tt) => (f!(@mk $a, Some($b.into()), None));
            ($a:tt . $b:tt p $e:tt) => (f!(@mk $a, Some($b.into()), Some($e.into())));
            (@mk $a:tt, $b:expr, $e:expr) => (crate::lexer::Float::Val {
                hex: true,
                integral: $a.into(),
                fractional: $b,
                exponent: $e
            });
        }
    assert_eq!(super::strtof(&f!("0")), Some(0));
    assert_eq!(super::strtof(&f!("0" . "0")), Some(0));
    assert_eq!(super::strtof(&f!("0" . "0" p "2354")), Some(0));
    assert_eq!(super::strtof(&f!("-0")), Some(1 << 31));
    assert_eq!(super::strtof(&f!("f32")), Some(0x45732000));
    assert_eq!(super::strtof(&f!("0" . "f32")), Some(0x3f732000));
    assert_eq!(super::strtof(&f!("1" . "2")), Some(0x3f900000));
    assert_eq!(
        super::strtof(&f!("0" . "00000100000000000" p "-126")),
        Some(0)
    );
    assert_eq!(
        super::strtof(&f!("1" . "fffff4" p "-106")),
        Some(0x0afffffa)
    );
    assert_eq!(super::strtof(&f!("fffff98" p "-133")), Some(0x0afffffa));
    assert_eq!(super::strtof(&f!("0" . "081" p "023")), Some(0x48810000));
    assert_eq!(
        super::strtof(&f!("1" . "00000100000000000" p "-50")),
        Some(0x26800000)
    );
}
