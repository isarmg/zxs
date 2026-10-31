#![warn(clippy::pedantic)]
#![allow(clippy::uninlined_format_args)]
use core::fmt::{Debug, Display, Formatter};
use core::ops::RangeInclusive;

const UNIT_SEP: &str = "bytes=";
/// Function that parses the content of a range header.
///
/// Follows the [spec here](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Range)
///
/// And [here](https://www.ietf.org/rfc/rfc2616.txt)
///
/// Will only accept bytes ranges, will update when [this spec](https://www.iana.org/assignments/http-parameters/http-parameters.xhtml) changes to allow other units.
///
/// Parses ranges strictly, as in the examples contained in the above specifications.
///
/// Ranges such as `bytes=0-15, 16-20, abc` will be rejected immediately.
///
/// It preserves the range ordering, the specification leaves it open to the server to determine whether
/// ranges that are out of order are correct or not, ie `bytes=20-30, 0-15`
///
/// # Example no trailing or leading whitespaces
/// ```
/// // Ok
/// assert!(http_range_header::parse_range_header("bytes=0-15").is_ok());
/// // Not allowed
/// assert!(http_range_header::parse_range_header("bytes=0-15 ").is_err());
/// // Also not allowed
/// assert!(http_range_header::parse_range_header("bytes= 0-15").is_err());
/// ```
///
/// # Example No leading whitespaces except in the case of separating multipart ranges
/// ```
/// // Ok, multipart with a leading whitespace after comma
/// assert!(http_range_header::parse_range_header("bytes=0-15, 20-30").is_ok());
/// // Ok multipart without leading whitespace after comma
/// assert!(http_range_header::parse_range_header("bytes=0-15,20-30").is_ok());
/// ```
///
/// # Example No negative values, no leading zeroes, no plus-sign
/// ```
/// // No negatives
/// assert!(http_range_header::parse_range_header("bytes=-12-15").is_err());
/// // No leading zeroes
/// assert!(http_range_header::parse_range_header("bytes=00-15").is_err());
/// // No plus sign
/// assert!(http_range_header::parse_range_header("bytes=+0-15").is_err());
/// ```
///
/// Makes two passes and parses ranges strictly. On the first pass, if any range is malformed returns an `Err`.
///
/// On the second pass if the ranges doesn't make sense (reversed range, range out of bounds, etc.) returns an `Err`.
/// # Example with a standard valid range
///
/// ```
/// let input = "bytes=0-15";
/// let file_size_bytes = 512;
/// let parsed_ranges = http_range_header::parse_range_header(input);
///
/// match parsed_ranges {
///     Ok(ranges) => {
///         match ranges.validate(file_size_bytes) {
///             Ok(valid_ranges) => {
///                 for range in valid_ranges {
///                     // do something with ranges
///                     assert_eq!(0..=15, range)
///                 }
///             }
///             Err(_err) => {
///                 // Do something when ranges doesn't make sense
///                 panic!("Weird range!")
///             }
///         }
///     }
///     Err(_err) => {
///         // Do something with malformed ranges
///         panic!("Malformed range!")
///     }
/// }
/// ```
///
/// The parser makes two passes, one without a known file-size, ensuring all ranges are syntactically correct.
/// The returned struct will through its `validate` method accept a file-size and figure out whether or not the
/// syntactically correct ranges actually makes sense in context
///
/// The range `bytes=0-20` on a file with 15 bytes will be accepted in the first pass as the content size is unknown.
/// On the second pass (`validate`) it will be truncated to `file_size - 1` as per [the spec](https://httpwg.org/specs/rfc9110.html#rfc.section.14.1.2).
/// # Example range truncates in `validate` because it exceedes
/// ```
/// let input = "bytes=0-20";
/// let file_size_bytes = 15;
/// let parsed_ranges = http_range_header::parse_range_header(input)
///     // Is syntactically correct
///     .unwrap();
/// let validated = parsed_ranges.validate(file_size_bytes).unwrap();
/// assert_eq!(vec![0..=14], validated);
/// ```
///
/// Range reversal and overlap is also checked in the second pass, the range `bytes=0-20, 5-10`
/// will become two syntactically correct ranges, but `validate` will return ann `Err`.
///
/// This is an opinionated implementation, [the spec](https://datatracker.ietf.org/doc/html/rfc7233)
/// allows a server to determine its implementation of overlapping ranges, this api currently does not allow it.
///
/// # Example multipart-range fails `validate` because of an overlap
/// ```
/// let input = "bytes=0-15, 10-20, 30-50";
/// let file_size_bytes = 512;
/// let parsed_ranges = http_range_header::parse_range_header(input)
///     // Is syntactically correct
///     .unwrap();
/// let validated = parsed_ranges.validate(file_size_bytes);
/// // Some ranges overlap, all valid ranges get truncated to 1 Err
/// assert!(validated.is_err());
/// ```
/// # Errors
/// Will return an error if the `range_header_value` cannot be strictly parsed into a range
/// per the http spec.
pub fn parse_range_header(
    range_header_value: &str,
) -> Result<ParsedRanges, RangeUnsatisfiableError> {
    const COMMA: char = ',';
    if let Some((prefix, indicated_range)) = range_header_value.split_once(UNIT_SEP) {
        if indicated_range.starts_with(char::is_whitespace) {
            return Err(RangeUnsatisfiableError::StartsWithWhitespace);
        }
        if !prefix.is_empty() {
            return Err(RangeUnsatisfiableError::DoesNotStartWithToken);
        }
        let mut last_err = None;
        let ranges = indicated_range
            .split(COMMA)
            .filter_map(|range| {
                if let Some(trimmed) = trim(range) {
                    match parse_inner(trimmed) {
                        Ok(parsed) => Some(parsed),
                        Err(e) => {
                            last_err = Some(e);
                            None
                        }
                    }
                } else {
                    last_err = Some(RangeUnsatisfiableError::IllegalWhitespace);
                    None
                }
            })
            .collect::<Vec<SyntacticallyCorrectRange>>();
        if let Some(last_err) = last_err {
            return Err(last_err);
        }
        if ranges.is_empty() {
            // Some other error should have been caught before we end up here
            Err(RangeUnsatisfiableError::Empty)
        } else {
            Ok(ParsedRanges::new(ranges))
        }
    } else {
        Err(RangeUnsatisfiableError::DoesNotStartWithToken)
    }
}

fn trim(s: &str) -> Option<&str> {
    if s.ends_with(char::is_whitespace) || s.match_indices(char::is_whitespace).count() > 1 {
        None
    } else {
        Some(s.trim())
    }
}

#[inline]
fn parse_inner(range: &str) -> Result<SyntacticallyCorrectRange, RangeUnsatisfiableError> {
    if let Some((start, end)) = range.split_once('-') {
        if start.is_empty() {
            if let Some(end) = strict_parse_u64(end) {
                if end == 0 {
                    return Err(RangeUnsatisfiableError::ZeroSuffix);
                }
                return Ok(SyntacticallyCorrectRange::new(
                    StartPosition::FromLast(end),
                    EndPosition::LastByte,
                ));
            }
            return Err(RangeUnsatisfiableError::BadEndOfRange);
        }
        if let Some(start) = strict_parse_u64(start) {
            if end.is_empty() {
                return Ok(SyntacticallyCorrectRange::new(
                    StartPosition::Index(start),
                    EndPosition::LastByte,
                ));
            }
            if let Some(end) = strict_parse_u64(end) {
                return Ok(SyntacticallyCorrectRange::new(
                    StartPosition::Index(start),
                    EndPosition::Index(end),
                ));
            }
            return Err(RangeUnsatisfiableError::BadEndOfRange);
        }
        return Err(RangeUnsatisfiableError::BadStartOfRange);
    }
    Err(RangeUnsatisfiableError::UnexpectedNumberOfDashes)
}

fn strict_parse_u64(s: &str) -> Option<u64> {
    if !s.starts_with('+') && (s.len() == 1 || !s.starts_with('0')) {
        return s.parse::<u64>().ok();
    }
    None
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ParsedRanges {
    pub ranges: Vec<SyntacticallyCorrectRange>,
}

impl ParsedRanges {
    fn new(ranges: Vec<SyntacticallyCorrectRange>) -> Self {
        ParsedRanges { ranges }
    }

    /// Validates a parsed range for a given file-size in bytes.
    /// # Errors
    /// If the range is invalid for the the file-size.
    pub fn validate(
        &self,
        file_size_bytes: u64,
    ) -> Result<Vec<RangeInclusive<u64>>, RangeUnsatisfiableError> {
        if file_size_bytes == 0 {
            return Err(RangeUnsatisfiableError::EmptyResource);
        }
        let len = self.ranges.len();
        let mut validated = Vec::with_capacity(len);
        for parsed in &self.ranges {
            let start = match parsed.start {
                StartPosition::Index(i) => i,
                StartPosition::FromLast(i) => {
                    if i > file_size_bytes {
                        return Err(RangeUnsatisfiableError::FileSuffixOutOfBounds);
                    }
                    file_size_bytes.saturating_sub(i)
                }
            };
            let end = match parsed.end {
                EndPosition::Index(i) => core::cmp::min(i, file_size_bytes.saturating_sub(1)),
                EndPosition::LastByte => file_size_bytes.saturating_sub(1),
            };

            let valid = RangeInclusive::new(start, end);
            validated.push(valid);
        }
        // False positive
        #[allow(clippy::match_same_arms)]
        match validate_ranges(validated.as_slice()) {
            RangeValidationResult::Valid => Ok(validated),
            RangeValidationResult::Overlapping => Err(RangeUnsatisfiableError::OverlappingRanges),
            RangeValidationResult::Reversed => Err(RangeUnsatisfiableError::RangeReversed),
        }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum RangeUnsatisfiableError {
    EmptyResource,
    OverlappingRanges,
    RangeReversed,
    FileSuffixOutOfBounds,
    IllegalWhitespace,
    StartsWithWhitespace,
    DoesNotStartWithToken,
    ZeroSuffix,
    BadStartOfRange,
    BadEndOfRange,
    UnexpectedNumberOfDashes,
    Empty,
}

impl Display for RangeUnsatisfiableError {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            RangeUnsatisfiableError::EmptyResource => {
                f.write_str("RangeUnsatisfiable: Empty resource has no byte ranges")
            }
            RangeUnsatisfiableError::OverlappingRanges => {
                f.write_str("RangeUnsatisfiable: Ranges overlap")
            }
            RangeUnsatisfiableError::RangeReversed => {
                f.write_str("RangeUnsatisfiable: Reversed range")
            }
            RangeUnsatisfiableError::FileSuffixOutOfBounds => f.write_str(
                "RangeUnsatisfiable: File suffix out of bounds (larger than file bytes)",
            ),
            RangeUnsatisfiableError::IllegalWhitespace => {
                f.write_str("RangeUnsatisfiable: Illegal whitespaces in range")
            }
            RangeUnsatisfiableError::StartsWithWhitespace => {
                f.write_str("RangeUnsatisfiable: Range starts with whitespace")
            }
            RangeUnsatisfiableError::DoesNotStartWithToken => f.write_fmt(format_args!(
                "RangeUnsatisfiable: Range does not start with token '{UNIT_SEP}'"
            )),
            RangeUnsatisfiableError::ZeroSuffix => {
                f.write_str("RangeUnsatisfiable: Range ends at 0")
            }
            RangeUnsatisfiableError::BadStartOfRange => {
                f.write_str("RangeUnsatisfiable: Unparseable start of range")
            }
            RangeUnsatisfiableError::BadEndOfRange => {
                f.write_str("RangeUnsatisfiable: Unparseable end of range")
            }
            RangeUnsatisfiableError::UnexpectedNumberOfDashes => {
                f.write_str("RangeUnsatisfiable: Unexpected number of dashes")
            }
            RangeUnsatisfiableError::Empty => f.write_str(
                "RangeUnsatisfiable: Failed to parse range fallback error, please file an issue",
            ),
        }
    }
}

impl std::error::Error for RangeUnsatisfiableError {}

enum RangeValidationResult {
    Valid,
    Overlapping,
    Reversed,
}

fn validate_ranges(ranges: &[RangeInclusive<u64>]) -> RangeValidationResult {
    let mut bounds = Vec::new();
    for range in ranges {
        let start = range.start();
        let end = range.end();
        if start > end {
            return RangeValidationResult::Reversed;
        } else if ranges.len() == 1 {
            return RangeValidationResult::Valid;
        }
        bounds.push((range.start(), range.end()));
    }
    for i in 0..bounds.len() {
        for j in i + 1..bounds.len() {
            if bounds[i].0 <= bounds[j].1 && bounds[j].0 <= bounds[i].1 {
                return RangeValidationResult::Overlapping;
            }
        }
    }
    RangeValidationResult::Valid
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct SyntacticallyCorrectRange {
    pub start: StartPosition,
    pub end: EndPosition,
}

impl SyntacticallyCorrectRange {
    fn new(start: StartPosition, end: EndPosition) -> Self {
        SyntacticallyCorrectRange { start, end }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum StartPosition {
    Index(u64),
    FromLast(u64),
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum EndPosition {
    Index(u64),
    LastByte,
}

#[cfg(test)]
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../test/foundation/http-range-header/unit/src/lib/tests_368.rs"
));
