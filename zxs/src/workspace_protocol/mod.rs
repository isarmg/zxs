//! ZXS/1 application requests. The transport owns authenticated TLS and
//! exact HTTP message framing; this module neither listens nor opens storage.
const ROOT: &str = "/zxs/v1/containers";

mod types;
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
fn hex<const N: usize>(s: &str) -> Result<[u8;N], Rejected> {
    if s.len() != N * 2 { return Err(Rejected::Value); }
    let mut value = [0;N];
    for (out, pair) in value.iter_mut().zip(s.as_bytes().chunks_exact(2)) {
        *out = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Ok(value)
}
// Initial logical name enters the control headers as ASCII hex of its UTF-8.
fn name_hex(s: &str) -> Result<String, Rejected> {
    if s.is_empty() || s.len() % 2 != 0 { return Err(Rejected::Value); }
    let bytes = s.as_bytes().chunks_exact(2)
        .map(|pair| Ok((nibble(pair[0])? << 4) | nibble(pair[1])?))
        .collect::<Result<Vec<_>, Rejected>>()?;
    let name = String::from_utf8(bytes).map_err(|_| Rejected::Value)?;
    if name == "." || name == ".." || name.chars().any(|ch| ch.is_control() || matches!(ch, '/' | '\\')) {
        return Err(Rejected::Value);
    }
    Ok(name)
}

fn decimal(s: &str) -> Result<u64, Rejected> {
    if s.is_empty() || (s.len() != 1 && s.starts_with('0'))
        || !s.bytes().all(|b| b.is_ascii_digit()) { return Err(Rejected::Value); }
    s.parse().map_err(|_| Rejected::Value)
}

// The adapter must not silently merge duplicates, normalize targets or pass
// trailers. It bounds headers/body syntax before dispatch and authorizes IDs.
fn field<'a>(headers: &[(&str, &'a str)], name: &str) -> Result<Option<&'a str>, Rejected> {
    let mut found = headers.iter().filter(|(n, _)| n.eq_ignore_ascii_case(name));
    let value = found.next().map(|(_, value)| *value);
    if found.next().is_some() { return Err(Rejected::Header); }
    Ok(value)
}
pub(crate) fn request(method: &str, target: &str, headers: &[(&str,&str)]) -> Result<Request, Rejected> {
    for h in ["transfer-encoding", "content-encoding", "content-range", "range"] {
        if field(headers,h)?.is_some() { return Err(Rejected::Header); }
    }
    let length = field(headers,"content-length")?.map(decimal).transpose()?;
    if matches!(method, "GET" | "DELETE") && length.unwrap_or_default() != 0 { return Err(Rejected::Header); }
    if target == ROOT { return if method == "GET" { Ok(Request::List) } else { Err(Rejected::Route) }; }
    let rest = target.strip_prefix(ROOT).and_then(|s| s.strip_prefix('/')).ok_or(Rejected::Route)?;
    let mut path = rest.split('/');
    let container = Id::parse(path.next().ok_or(Rejected::Route)?)?;
    let (a,b,c) = (path.next(),path.next(),path.next());
    if path.next().is_some() { return Err(Rejected::Route); }

    let required = |name| field(headers,name)?.ok_or(Rejected::Header);
    let op = || Id::parse(required("zxs-operation")?);
    let metadata = || -> Result<MetadataMutation, Rejected> { Ok(MetadataMutation {
        container, operation: op()?, revision: decimal(required("zxs-base-generation")?)?,
    }) };
    let json = |mutation| -> Result<Request, Rejected> {
        if required("content-type")? != "application/json" { return Err(Rejected::Header); }
        let scope = metadata()?;
        if scope.revision == 0 { return Err(Rejected::Value); }
        Ok(Request::Json { scope, mutation, length: length.filter(|n| *n != 0).ok_or(Rejected::Header)? })
    };

    match (method,a,b,c) {
        ("GET",None,None,None) => Ok(Request::Container(container)),
        ("GET",Some("tags"),None,None) => Ok(Request::Tags(container)),
        ("GET",Some("nodes"),Some(node),Some("data")) =>
            Ok(Request::NodeData { container, node: Id::parse(node)? }),
        ("GET",Some("branches"),Some(branch),Some("nodes")) =>
            Ok(Request::BranchNodes { container, branch: Branch::parse(branch)? }),
        ("GET",Some("operations"),Some(operation),None) =>
            Ok(Request::Receipt { container, operation: Id::parse(operation)? }),
        ("DELETE",None,None,None) => {
            let scope = metadata()?;
            if scope.revision == 0 { return Err(Rejected::Value); }
            Ok(Request::Delete(scope))
        }
        ("PUT",Some("name"),None,None) => json(JsonMutation::Rename),
        ("PUT",Some("tags"),None,None) => json(JsonMutation::ReplaceTags),
        ("PUT",Some("branches"),Some(new),None) => {
            if required("content-type")? != "application/json"
                || field(headers,"zxs-base-generation")?.is_some() {
                return Err(Rejected::Header);
            }
            Ok(Request::CreateBranch { container, operation: op()?, branch: Id::parse(new)?,
                length: length.filter(|n| *n != 0).ok_or(Rejected::Header)? })
        }
        ("POST",Some("branches"),Some(branch),Some("nodes")) => {
            if required("content-type")? != "application/octet-stream" { return Err(Rejected::Header); }
            let branch = Branch::parse(branch)?;
            let head = field(headers,"zxs-base-node")?;
            let name = field(headers,"zxs-initial-name")?;
            let fork = field(headers,"zxs-fork-from-main")?;
            let target = match (branch, head, name, fork) {
                (Branch::Main, Some("0"), Some(name), None) =>
                    Target::FirstMain { name: name_hex(name)? },
                (Branch::Named(new_branch), None, None, Some(original)) =>
                    Target::HistoricalMain { new_branch, original: Id::parse(original)? },
                (branch, Some(head), None, None) =>
                    Target::Branch { branch, expected_head: Id::parse(head)? },
                _ => return Err(Rejected::Header),
            };
            Ok(Request::SaveNode(SaveNode {
                container, operation: op()?, node: Id::parse(required("zxs-node")?)?, target,
                length: length.ok_or(Rejected::Header)?, sha256: hex(required("zxs-sha256")?)?,
            }))
        }
        _ => Err(Rejected::Route),
    }
}

#[cfg(test)]
#[path = "../../tests/unit/attached/workspace_protocol/tests.rs"]
mod tests;
