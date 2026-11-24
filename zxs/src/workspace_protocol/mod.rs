//! ZXS/1 application requests. The transport owns authenticated TLS and
//! exact HTTP message framing; this module neither listens nor opens storage.
const ROOT: &str = "/zxs/v1/containers";

mod ranges;
mod types;
pub(crate) use ranges::{PayloadRead, PayloadWindow, is_payload_target, payload_request};
pub(crate) use types::*;
pub(crate) mod dispatch;
pub(crate) mod http;

fn nibble(b: u8) -> Result<u8, Rejected> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        _ => Err(Rejected::Value),
    }
}
fn hex<const N: usize>(s: &str) -> Result<[u8; N], Rejected> {
    let (pairs, tail) = s.as_bytes().as_chunks::<2>();
    if pairs.len() != N || !tail.is_empty() {
        return Err(Rejected::Value);
    }
    let mut value = [0; N];
    for (out, [high, low]) in value.iter_mut().zip(pairs) {
        *out = (nibble(*high)? << 4) | nibble(*low)?;
    }
    Ok(value)
}
// Initial logical name enters the control headers as ASCII hex of its UTF-8.
fn name_hex(s: &str) -> Result<String, Rejected> {
    use crate::display::{MAX, component};

    let (pairs, tail) = s.as_bytes().as_chunks::<2>();
    if pairs.is_empty() || pairs.len() > MAX || !tail.is_empty() {
        return Err(Rejected::Value);
    }
    let bytes = pairs
        .iter()
        .map(|[high, low]| Ok((nibble(*high)? << 4) | nibble(*low)?))
        .collect::<Result<Vec<_>, Rejected>>()?;
    component(&bytes, MAX)
        .map(str::to_owned)
        .map_err(|_| Rejected::Value)
}

fn decimal(s: &str) -> Result<u64, Rejected> {
    if s.is_empty()
        || (s.len() != 1 && s.starts_with('0'))
        || !s.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(Rejected::Value);
    }
    s.parse().map_err(|_| Rejected::Value)
}

fn list_request(
    method: &str,
    target: &str,
    headers: &[(&str, &str)],
) -> Result<Option<Request>, Rejected> {
    let view = field(headers, "zxs-view")?;
    let after = field(headers, "zxs-after")?;
    if target == ROOT && method == "GET" {
        return match (view, after) {
            (Some("compact"), after) => Ok(Some(Request::ListCompact {
                after: after.map(Id::parse).transpose()?,
            })),
            _ => Err(Rejected::Header),
        };
    }
    if view.is_some() || after.is_some() {
        return Err(Rejected::Header);
    }
    if target == ROOT {
        return Err(Rejected::Route);
    }
    Ok(None)
}

// The adapter must not silently merge duplicates, normalize targets or pass
// trailers. It bounds headers/body syntax before dispatch and authorizes IDs.
fn field<'a>(headers: &[(&str, &'a str)], name: &str) -> Result<Option<&'a str>, Rejected> {
    let mut found = headers.iter().filter(|(n, _)| n.eq_ignore_ascii_case(name));
    let value = found.next().map(|(_, value)| *value);
    if found.next().is_some() {
        return Err(Rejected::Header);
    }
    Ok(value)
}

fn required_header<'a>(headers: &[(&str, &'a str)], name: &str) -> Result<&'a str, Rejected> {
    field(headers, name)?.ok_or(Rejected::Header)
}

fn selected_read_request(
    method: &str,
    target: &str,
    headers: &[(&str, &str)],
) -> Result<Option<Request>, Rejected> {
    let catalog = list_request(method, target, headers)?;
    let history = history_request(method, target, headers)?;
    let detail = detail_request(method, target, headers)?;
    Ok(catalog.or(history).or(detail))
}

fn detail_request(
    method: &str,
    target: &str,
    headers: &[(&str, &str)],
) -> Result<Option<Request>, Rejected> {
    let mode = field(headers, "zxs-detail")?;
    let sequence = field(headers, "zxs-detail-sequence")?;
    let after = field(headers, "zxs-detail-after")?;
    if mode.is_none() && sequence.is_none() && after.is_none() {
        return Ok(None);
    }
    if mode != Some("paged") || method != "GET" || (after.is_some() && sequence.is_none()) {
        return Err(Rejected::Header);
    }
    let id = target
        .strip_prefix(ROOT)
        .and_then(|suffix| suffix.strip_prefix('/'))
        .ok_or(Rejected::Header)?;
    let sequence = sequence.map(decimal).transpose()?;
    if sequence == Some(0) {
        return Err(Rejected::Value);
    }
    Ok(Some(Request::ContainerDetail {
        container: Id::parse(id)?,
        sequence,
        after: after.map(Branch::parse).transpose()?,
    }))
}

fn history_request(
    method: &str,
    target: &str,
    headers: &[(&str, &str)],
) -> Result<Option<Request>, Rejected> {
    let head = field(headers, "zxs-history-head")?;
    let next = field(headers, "zxs-history-next")?;
    if head.is_none() && next.is_none() {
        return Ok(None);
    }
    let rest = target
        .strip_prefix(ROOT)
        .and_then(|part| part.strip_prefix('/'))
        .filter(|_| method == "GET")
        .ok_or(Rejected::Header)?;
    let mut path = rest.split('/');
    let (Some(container), Some("branches"), Some(branch), Some("nodes"), None) = (
        path.next(),
        path.next(),
        path.next(),
        path.next(),
        path.next(),
    ) else {
        return Err(Rejected::Header);
    };
    let head = Id::parse(head.ok_or(Rejected::Header)?)?;
    let next = next.map(Id::parse).transpose()?;
    if next == Some(head) {
        return Err(Rejected::Value);
    }
    Ok(Some(Request::BranchHistory {
        container: Id::parse(container)?,
        branch: Branch::parse(branch)?,
        head,
        next,
    }))
}

pub(crate) fn request(
    method: &str,
    target: &str,
    headers: &[(&str, &str)],
) -> Result<Request, Rejected> {
    for h in [
        "transfer-encoding",
        "content-encoding",
        "content-range",
        "range",
    ] {
        if field(headers, h)?.is_some() {
            return Err(Rejected::Header);
        }
    }
    let length = field(headers, "content-length")?.map(decimal).transpose()?;
    if matches!(method, "GET" | "DELETE") && length.unwrap_or_default() != 0 {
        return Err(Rejected::Header);
    }
    if let Some(read) = selected_read_request(method, target, headers)? {
        return Ok(read);
    }
    let rest = target
        .strip_prefix(ROOT)
        .and_then(|s| s.strip_prefix('/'))
        .ok_or(Rejected::Route)?;
    let mut path = rest.split('/');
    let container = Id::parse(path.next().ok_or(Rejected::Route)?)?;
    let (a, b, c) = (path.next(), path.next(), path.next());
    if path.next().is_some() {
        return Err(Rejected::Route);
    }

    let required = |name| required_header(headers, name);
    let op = || Id::parse(required("zxs-operation")?);
    let metadata = || -> Result<MetadataMutation, Rejected> {
        Ok(MetadataMutation {
            container,
            operation: op()?,
            revision: decimal(required("zxs-base-generation")?)?,
        })
    };
    let json = |mutation| -> Result<Request, Rejected> {
        if required("content-type")? != "application/json" {
            return Err(Rejected::Header);
        }
        let scope = metadata()?;
        if scope.revision == 0 {
            return Err(Rejected::Value);
        }
        Ok(Request::Json {
            scope,
            mutation,
            length: length.filter(|n| *n != 0).ok_or(Rejected::Header)?,
        })
    };

    match (method, a, b, c) {
        ("GET", Some("tags"), None, None) => Ok(Request::Tags(container)),
        ("GET", Some("nodes"), Some(node), Some("data")) => Ok(Request::NodeData {
            container,
            node: Id::parse(node)?,
        }),
        ("GET", Some("operations"), Some(operation), None) => Ok(Request::Receipt {
            container,
            operation: Id::parse(operation)?,
        }),
        ("DELETE", None, None, None) => {
            let scope = metadata()?;
            if scope.revision == 0 {
                return Err(Rejected::Value);
            }
            Ok(Request::Delete(scope))
        }
        ("PUT", Some("name"), None, None) => json(JsonMutation::Rename),
        ("PUT", Some("tags"), None, None) => json(JsonMutation::ReplaceTags),
        ("PUT", Some("branches"), Some(new), None) => {
            if required("content-type")? != "application/json"
                || field(headers, "zxs-base-generation")?.is_some()
            {
                return Err(Rejected::Header);
            }
            Ok(Request::CreateBranch {
                container,
                operation: op()?,
                branch: Id::parse(new)?,
                length: length.filter(|n| *n != 0).ok_or(Rejected::Header)?,
            })
        }
        ("POST", Some("branches"), Some(branch), Some("nodes")) => {
            save_request(container, branch, headers, length)
        }
        _ => Err(Rejected::Route),
    }
}

fn save_request(
    container: Id,
    branch: &str,
    headers: &[(&str, &str)],
    length: Option<u64>,
) -> Result<Request, Rejected> {
    let required = |name| required_header(headers, name);
    if required("content-type")? != "application/octet-stream" {
        return Err(Rejected::Header);
    }
    let branch = Branch::parse(branch)?;
    let head = field(headers, "zxs-base-node")?;
    let name = field(headers, "zxs-initial-name")?;
    let fork = field(headers, "zxs-fork-from-main")?;
    let target = match (branch, head, name, fork) {
        (Branch::Main, Some("0"), Some(name), None) => Target::FirstMain {
            name: name_hex(name)?,
        },
        (Branch::Named(new_branch), None, None, Some(original)) => Target::HistoricalMain {
            new_branch,
            original: Id::parse(original)?,
        },
        (branch, Some(head), None, None) => Target::Branch {
            branch,
            expected_head: Id::parse(head)?,
        },
        _ => return Err(Rejected::Header),
    };
    Ok(Request::SaveNode(SaveNode {
        container,
        operation: Id::parse(required("zxs-operation")?)?,
        node: Id::parse(required("zxs-node")?)?,
        target,
        length: length.ok_or(Rejected::Header)?,
        sha256: hex(required("zxs-sha256")?)?,
    }))
}

#[cfg(test)]
#[path = "../../tests/unit/attached/workspace_protocol/tests.rs"]
mod tests;
