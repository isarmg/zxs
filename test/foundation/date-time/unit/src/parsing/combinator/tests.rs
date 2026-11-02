use super::ExactlyNDigits;
use crate::format_description::{BorrowedFormatItem, Component, modifier};
use crate::Time;

#[test]
fn nine_digits_checks_every_byte_before_numeric_conversion() {
    for position in 0..9 {
        for byte in u8::MIN..=u8::MAX {
            let mut input = *b"123456789";
            input[position] = byte;
            let parsed = ExactlyNDigits::<9>::parse(&input);
            if byte.is_ascii_digit() {
                let expected = core::str::from_utf8(&input)
                    .unwrap()
                    .parse::<u32>()
                    .unwrap();
                let parsed = parsed.expect("nine decimal digits must parse");
                assert!(parsed.0.is_empty());
                assert_eq!(parsed.1, expected);
            } else {
                assert!(
                    parsed.is_none(),
                    "accepted byte {byte:#x} at position {position}"
                );
            }
        }
    }
}

#[test]
fn fixed_nine_digit_subseconds_return_parse_errors_for_invalid_input() {
    let format = [
        BorrowedFormatItem::Component(Component::Hour24(modifier::Hour24::default())),
        BorrowedFormatItem::StringLiteral(":"),
        BorrowedFormatItem::Component(Component::Minute(modifier::Minute::default())),
        BorrowedFormatItem::StringLiteral(":"),
        BorrowedFormatItem::Component(Component::Second(modifier::Second::default())),
        BorrowedFormatItem::StringLiteral("."),
        BorrowedFormatItem::Component(Component::Subsecond(
            modifier::Subsecond::default().with_digits(modifier::SubsecondDigits::Nine),
        )),
    ];
    for (input, nanosecond) in [
        ("12:34:56.000000000", 0),
        ("12:34:56.123456789", 123_456_789),
        ("12:34:56.999999999", 999_999_999),
    ] {
        assert_eq!(
            Time::parse(input, &format[..]),
            Ok(Time::from_hms_nano(12, 34, 56, nanosecond).unwrap())
        );
    }
    for input in [
        "12:34:56.1234/6789",
        "12:34:56.12345678/",
        "12:34:56.\0\0\0\0\0\0\0\0\0",
        "12:34:56.12345678",
        "12:34:56.1234567890",
        "12:34:56.1234567é",
    ] {
        assert!(Time::parse(input, &format[..]).is_err(), "accepted {input:?}");
    }
}

#[test]
fn period_case_sensitivity_is_preserved_for_each_letter_case() {
    const CASES: [(&str, &str, u8); 8] = [
        ("09:30:00 am", "am", 9),
        ("09:30:00 aM", "aM", 9),
        ("09:30:00 Am", "Am", 9),
        ("09:30:00 AM", "AM", 9),
        ("09:30:00 pm", "pm", 21),
        ("09:30:00 pM", "pM", 21),
        ("09:30:00 Pm", "Pm", 21),
        ("09:30:00 PM", "PM", 21),
    ];
    const ANY_CASE: &[&str] = &["am", "aM", "Am", "AM", "pm", "pM", "Pm", "PM"];
    const CONFIGURATIONS: [(bool, bool, &[&str]); 4] = [
        (false, false, ANY_CASE),
        (false, true, &["am", "pm"]),
        (true, false, ANY_CASE),
        (true, true, &["AM", "PM"]),
    ];

    for (uppercase, case_sensitive, accepted) in CONFIGURATIONS {
        let format = [
            BorrowedFormatItem::Component(Component::Hour12(modifier::Hour12::default())),
            BorrowedFormatItem::StringLiteral(":"),
            BorrowedFormatItem::Component(Component::Minute(modifier::Minute::default())),
            BorrowedFormatItem::StringLiteral(":"),
            BorrowedFormatItem::Component(Component::Second(modifier::Second::default())),
            BorrowedFormatItem::StringLiteral(" "),
            BorrowedFormatItem::Component(Component::Period(
                modifier::Period::default()
                    .with_is_uppercase(uppercase)
                    .with_case_sensitive(case_sensitive),
            )),
        ];
        for (input, label, expected_hour) in CASES {
            let parsed = Time::parse(input, &format[..]);
            if accepted.contains(&label) {
                assert_eq!(parsed, Ok(Time::from_hms(expected_hour, 30, 0).unwrap()));
            } else {
                assert!(
                    parsed.is_err(),
                    "accepted {label:?} with uppercase={uppercase}, case_sensitive={case_sensitive}"
                );
            }
        }
    }
}
