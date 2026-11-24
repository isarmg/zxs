//! Frozen read-only byte selection, separate from the deployed R3 /data parser.
use super::{Id, ROOT, Rejected, Request, decimal, field, hex};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Requested {
    Closed(u64, u64),
    From(u64),
    Suffix(u64),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IfRange {
    None,
    Exact([u8; 32]),
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PayloadRead {
    pub(crate) container: Id,
    pub(crate) node: Id,
    range: Option<Requested>,
    validator: IfRange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PayloadWindow {
    Full,
    Span(Span),
    Unsatisfiable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Span {
    start: u64,
    end: u64,
}

impl Span {
    pub(crate) fn bounds(self) -> (u64, u64) {
        (self.start, self.end)
    }

    pub(crate) fn length(self) -> u64 {
        self.end - self.start + 1
    }
}

impl PayloadRead {
    pub(crate) fn window(self, total: u64, sha: [u8; 32]) -> PayloadWindow {
        let Some(range) = self.range else {
            return PayloadWindow::Full;
        };
        if self.validator != IfRange::None && self.validator != IfRange::Exact(sha) {
            return PayloadWindow::Full;
        }
        let Some(last) = total.checked_sub(1) else {
            return PayloadWindow::Unsatisfiable;
        };
        let (start, end) = match range {
            Requested::Closed(start, end) if start <= last => (start, end.min(last)),
            Requested::From(start) if start <= last => (start, last),
            Requested::Suffix(count) if count != 0 => (total.saturating_sub(count), last),
            _ => return PayloadWindow::Unsatisfiable,
        };
        PayloadWindow::Span(Span { start, end })
    }
}

fn path(target: &str) -> Option<(&str, &str)> {
    let rest = target.strip_prefix(ROOT)?.strip_prefix('/')?;
    let mut parts = rest.split('/');
    match (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) {
        (Some(c), Some("nodes"), Some(n), Some("payload"), None) => Some((c, n)),
        _ => None,
    }
}

/// Detect the protected new target without reading any storage or using a
/// legacy credentials fallback. Exact typed IDs are checked after auth.
pub(crate) fn is_payload_target(target: &str) -> bool {
    path(target).is_some()
}

pub(crate) fn payload_request(
    method: &str,
    target: &str,
    headers: &[(&str, &str)],
) -> Result<Request, Rejected> {
    if method != "GET" {
        return Err(Rejected::Route);
    }
    let (container, node) = path(target).ok_or(Rejected::Route)?;
    let container = Id::parse(container)?;
    let node = Id::parse(node)?;
    for name in ["transfer-encoding", "content-encoding", "content-range"] {
        if field(headers, name)?.is_some() {
            return Err(Rejected::Header);
        }
    }
    if field(headers, "content-length")?
        .map(decimal)
        .transpose()?
        .unwrap_or_default()
        != 0
    {
        return Err(Rejected::Header);
    }
    let range = field(headers, "range")?
        .map(parse_range)
        .transpose()?
        .flatten();
    let validator = field(headers, "if-range")?.map_or(IfRange::None, validator);
    Ok(Request::NodePayload(PayloadRead {
        container,
        node,
        range,
        validator,
    }))
}

fn validator(value: &str) -> IfRange {
    value
        .strip_prefix('"')
        .and_then(|open| open.strip_suffix('"'))
        .and_then(|digits| hex(digits).ok())
        .map_or(IfRange::Other, IfRange::Exact)
}

fn parse_range(value: &str) -> Result<Option<Requested>, Rejected> {
    let (unit, extent) = value.split_once('=').ok_or(Rejected::Header)?;
    let token = |b: u8| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b);
    if unit.is_empty()
        || !unit.bytes().all(token)
        || extent.is_empty()
        || !extent.bytes().all(|b| b.is_ascii_graphic() && b != b',')
    {
        return Err(Rejected::Header);
    }
    if !unit.eq_ignore_ascii_case("bytes") {
        return Ok(None);
    }
    let (first, last) = extent.split_once('-').ok_or(Rejected::Header)?;
    let selected = match (first, last) {
        ("", tail) => Requested::Suffix(number(tail)?),
        (head, "") => Requested::From(number(head)?),
        (head, tail) => {
            let start = number(head)?;
            let end = number(tail)?;
            if end < start {
                return Err(Rejected::Value);
            }
            Requested::Closed(start, end)
        }
    };
    Ok(Some(selected))
}

fn number(value: &str) -> Result<u64, Rejected> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Rejected::Value);
    }
    value.parse().map_err(|_| Rejected::Value)
}

#[cfg(test)]
#[path = "../../tests/unit/payload_ranges.rs"]
mod tests;
