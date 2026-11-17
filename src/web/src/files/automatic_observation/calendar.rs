use date_time::{Date, Month, PrimitiveDateTime, Time};
use storage_contract::Timestamp;

/// HTML datetime-local supplies no zone. The control explicitly names UTC.
pub(super) fn parse(text: &str) -> Option<Timestamp> {
    let b = text.as_bytes();
    if !matches!(b.len(), 16 | 19)
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || (b.len() == 19 && b[16] != b':')
    {
        return None;
    }
    fn digits(bytes: &[u8]) -> Option<u16> {
        bytes.iter().try_fold(0, |n, b| {
            b.is_ascii_digit().then(|| n * 10 + u16::from(*b - b'0'))
        })
    }
    let date = Date::from_calendar_date(
        i32::from(digits(&b[..4])?),
        Month::try_from(u8::try_from(digits(&b[5..7])?).ok()?).ok()?,
        u8::try_from(digits(&b[8..10])?).ok()?,
    )
    .ok()?;
    let second = if b.len() == 19 {
        u8::try_from(digits(&b[17..19])?).ok()?
    } else {
        0
    };
    let time = Time::from_hms(
        u8::try_from(digits(&b[11..13])?).ok()?,
        u8::try_from(digits(&b[14..16])?).ok()?,
        second,
    )
    .ok()?;
    let unix = PrimitiveDateTime::new(date, time)
        .assume_utc()
        .unix_timestamp();
    Some(Timestamp::from_unix_seconds(u64::try_from(unix).ok()?))
}
