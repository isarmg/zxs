#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    #[test]
    fn encoding_roundtrip() {
        let cases = [
            Config::DEFAULT,
            Config::DEFAULT.set_formatted_components(FC::None),
            Config::DEFAULT.set_formatted_components(FC::Date),
            Config::DEFAULT.set_formatted_components(FC::Time),
            Config::DEFAULT.set_formatted_components(FC::Offset),
            Config::DEFAULT.set_formatted_components(FC::DateTime),
            Config::DEFAULT.set_formatted_components(FC::DateTimeOffset),
            Config::DEFAULT.set_formatted_components(FC::TimeOffset),
            Config::DEFAULT.set_use_separators(false),
            Config::DEFAULT.set_use_separators(true),
            Config::DEFAULT.set_year_is_six_digits(false),
            Config::DEFAULT.set_year_is_six_digits(true),
            Config::DEFAULT.set_date_kind(DateKind::Calendar),
            Config::DEFAULT.set_date_kind(DateKind::Week),
            Config::DEFAULT.set_date_kind(DateKind::Ordinal),
            Config::DEFAULT.set_time_precision(TimePrecision::Hour {
                decimal_digits: None,
            }),
            Config::DEFAULT.set_time_precision(TimePrecision::Minute {
                decimal_digits: None,
            }),
            Config::DEFAULT.set_time_precision(TimePrecision::Second {
                decimal_digits: None,
            }),
            Config::DEFAULT.set_time_precision(TimePrecision::Hour {
                decimal_digits: NonZero::new(1),
            }),
            Config::DEFAULT.set_time_precision(TimePrecision::Minute {
                decimal_digits: NonZero::new(1),
            }),
            Config::DEFAULT.set_time_precision(TimePrecision::Second {
                decimal_digits: NonZero::new(1),
            }),
            Config::DEFAULT.set_offset_precision(OffsetPrecision::Hour),
            Config::DEFAULT.set_offset_precision(OffsetPrecision::Minute),
        ];
        for config in cases {
            let encoded = config.encode();
            let decoded = Config::decode(encoded);
            assert_eq!(config.formatted_components, decoded.formatted_components);
            assert_eq!(config.use_separators, decoded.use_separators);
            assert_eq!(config.year_is_six_digits, decoded.year_is_six_digits);
            assert_eq!(config.date_kind, decoded.date_kind);
            assert_eq!(config.time_precision, decoded.time_precision);
            assert_eq!(config.offset_precision, decoded.offset_precision);
        }
    }

    #[test]
    fn decode_fail() {
        let cases: [EncodedConfig; 7] = [
            0x07_00_00_00_00_00_00_00_00_00_00_00_00_00_00_00,
            0x00_02_00_00_00_00_00_00_00_00_00_00_00_00_00_00,
            0x00_00_02_00_00_00_00_00_00_00_00_00_00_00_00_00,
            0x00_00_00_03_00_00_00_00_00_00_00_00_00_00_00_00,
            0x00_00_00_00_03_00_00_00_00_00_00_00_00_00_00_00,
            0x00_00_00_00_00_00_02_00_00_00_00_00_00_00_00_00,
            0x00_00_00_00_00_00_00_01_00_00_00_00_00_00_00_00,
        ];
        for encoded in cases {
            assert!(
                std::panic::catch_unwind(|| Config::decode(encoded)).is_err(),
                "invalid encoded configuration {encoded:032x} unexpectedly decoded"
            );
        }
    }
}
