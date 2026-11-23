//! One HTTP/1.1 request on one accepted TCP connection; no TLS or forwarding policy.
use crate::{
    headless_auth::{Account, AuthError},
    workspace_protocol::{self, Request},
};
use std::{
    io::{self, BufRead, BufReader, Read, Take, Write},
    net::TcpStream,
    time::{Duration, Instant},
};

const MAX_HEAD: usize = 16 * 1024;
const MAX_FIELDS: usize = 64;
const MAX_JSON: u64 = 64 * 1024;
pub(super) const IO_TIMEOUT: Duration = Duration::from_secs(60);

pub(crate) struct Body<'a>(Take<&'a mut dyn Read>);
impl Body<'_> {
    pub(crate) fn remaining(&self) -> u64 {
        self.0.limit()
    }
}
impl Read for Body<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}
pub(crate) enum Incoming<'a> {
    Empty,
    Blob(Body<'a>),
    Json(Body<'a>),
}
/// Exhaustively map R3 request variants. In particular branch creation is
/// a JSON mutation with an exact body, not a bodyless branch lookup.
fn body_for<'a>(request: &Request, source: &'a mut dyn Read) -> Result<Incoming<'a>, Problem> {
    match request {
        Request::SaveNode(save) => Ok(Incoming::Blob(Body(source.take(save.length)))),
        Request::Json { length, .. } | Request::CreateBranch { length, .. } => {
            if *length > MAX_JSON {
                return Err(Problem::TooLarge);
            }
            Ok(Incoming::Json(Body(source.take(*length))))
        }
        Request::List
        | Request::ListCompact { .. }
        | Request::Container(_)
        | Request::ContainerDetail { .. }
        | Request::Tags(_)
        | Request::NodeData { .. }
        | Request::BranchNodes { .. }
        | Request::BranchHistory { .. }
        | Request::Receipt { .. }
        | Request::Delete(_) => Ok(Incoming::Empty),
    }
}
impl Incoming<'_> {
    fn exhausted(&self) -> bool {
        match self {
            Self::Empty => true,
            Self::Blob(b) | Self::Json(b) => b.remaining() == 0,
        }
    }
}

/// Success variants require exact confirmed application evidence.
pub(crate) enum Outcome {
    Json(Vec<u8>),
    ReceiptAbsent,
    Node {
        length: u64,
        sha256: [u8; 32],
        body: Box<dyn Read>,
    },
    Failed(Problem),
}

#[derive(Clone, Copy)]
pub(crate) enum Problem {
    Malformed,
    Unauthorized,
    Denied,
    Missing,
    Conflict,
    TooLarge,
    InvalidBody,
    Unavailable,
}
impl Problem {
    fn status(self) -> &'static str {
        match self {
            Self::Malformed => "400 Bad Request",
            Self::Unauthorized => "401 Unauthorized",
            Self::Denied => "403 Forbidden",
            Self::Missing => "404 Not Found",
            Self::Conflict => "409 Conflict",
            Self::TooLarge => "413 Content Too Large",
            Self::InvalidBody => "422 Unprocessable Content",
            Self::Unavailable => "503 Service Unavailable",
        }
    }
}
impl From<AuthError> for Problem {
    fn from(value: AuthError) -> Self {
        match value {
            AuthError::Invalid => Self::Unauthorized,
            AuthError::Unavailable => Self::Unavailable,
        }
    }
}

struct Head<'a> {
    method: &'a str,
    target: &'a str,
    fields: Vec<(&'a str, &'a str)>,
}

fn read_head(
    input: &mut impl BufRead,
    before_read: impl Fn() -> io::Result<()>,
) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut ended = 0;
    loop {
        before_read()?;
        let available = input.fill_buf()?;
        if available.is_empty() {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let count = available.len().min(MAX_HEAD - bytes.len());
        for (offset, byte) in available[..count].iter().copied().enumerate() {
            bytes.push(byte);
            ended = match (ended, byte) {
                (0 | 2, b'\r') | (1 | 3, b'\n') => ended + 1,
                (_, b'\r') => 1,
                _ => 0,
            };
            if ended == 4 {
                // Body or extra bytes in the buffer remain unread.
                input.consume(offset + 1);
                return Ok(bytes);
            }
        }
        input.consume(count);
        if bytes.len() == MAX_HEAD {
            return Err(io::ErrorKind::InvalidData.into());
        }
    }
}

fn token(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_|~".contains(&byte) || byte == 0x60
}

fn head(raw: &[u8]) -> Result<Head<'_>, Problem> {
    let bad = Problem::Malformed;
    let text = std::str::from_utf8(raw).map_err(|_| bad)?;
    let mut lines = text.strip_suffix("\r\n\r\n").ok_or(bad)?.split("\r\n");
    let mut parts = lines.next().ok_or(bad)?.split(' ');
    let method = parts
        .next()
        .filter(|v| !v.is_empty() && v.bytes().all(token))
        .ok_or(bad)?;
    let target = parts
        .next()
        .filter(|v| v.starts_with('/') && v.bytes().all(|b| b.is_ascii_graphic()))
        .ok_or(bad)?;
    if parts.next() != Some("HTTP/1.1") || parts.next().is_some() {
        return Err(bad);
    }
    let mut fields = Vec::new();
    let mut hosts = 0;
    for line in lines {
        let (key, value) = line.split_once(':').ok_or(bad)?;
        if fields.len() == MAX_FIELDS
            || key.is_empty()
            || !key.bytes().all(token)
            || value.bytes().any(|b| (b < b' ' && b != b'\t') || b == 0x7f)
        {
            return Err(bad);
        }
        let value = value.trim_matches([' ', '\t']);
        if key.eq_ignore_ascii_case("host") {
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_graphic() && b != b',') {
                return Err(bad);
            }
            hosts += 1;
        }
        // This single-request transport accepts no transfer coding, trailers,
        // upgrades or 100-continue.
        if ["transfer-encoding", "trailer", "te", "upgrade", "expect"]
            .iter()
            .any(|name| key.eq_ignore_ascii_case(name))
        {
            return Err(bad);
        }
        // Preserve the exact Authorization fields, including duplicates.
        fields.push((key, value));
    }
    if hosts != 1 {
        return Err(bad);
    }
    Ok(Head {
        method,
        target,
        fields,
    })
}

/// Own exactly one accepted socket and close it after one HTTP request.
/// The canonical Content-Length delimits this body, not TCP EOF; no following
/// bytes can become another admitted request on this connection.
///
/// Dispatcher must decode one exact bounded JSON object or consume the full
/// streaming `SaveNode`, checking length and digest before selection.
/// DELETE delegates to the container store; the adapter does not create receipts.
pub(crate) fn serve_one(
    account: &Account,
    stream: TcpStream,
    stopping: &dyn Fn() -> bool,
    dispatch: impl FnOnce(Request, &mut Incoming<'_>, &dyn Fn() -> bool) -> Outcome,
) -> io::Result<()> {
    let result = serve_request(account, &stream, stopping, dispatch);
    drop(stream);
    result
}

fn serve_request(
    account: &Account,
    stream: &TcpStream,
    stopping: &dyn Fn() -> bool,
    dispatch: impl FnOnce(Request, &mut Incoming<'_>, &dyn Fn() -> bool) -> Outcome,
) -> io::Result<()> {
    let mut input = BufReader::new(stream);
    let mut output = stream;
    let deadline = Instant::now() + IO_TIMEOUT;
    // Bound the complete head, rather than granting a new idle interval
    // for every byte. Body reads keep the existing per-read idle timeout.
    let before_head_read = || {
        if stopping() {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        stream.set_read_timeout(Some(left))
    };
    let raw = match read_head(&mut input, before_head_read) {
        Ok(raw) => raw,
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof
            ) =>
        {
            return write_outcome(&mut output, Outcome::Failed(Problem::Malformed));
        }
        Err(e) => return Err(e),
    };
    if Instant::now() >= deadline {
        return Err(io::ErrorKind::TimedOut.into());
    }
    if stopping() {
        return Ok(());
    }
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    let head = match head(&raw) {
        Ok(head) => head,
        Err(p) => return write_outcome(&mut output, Outcome::Failed(p)),
    };
    let permit = match account.authenticate(&head.fields, stopping) {
        Ok(permit) => permit,
        Err(_) if stopping() => return Ok(()),
        Err(e) => return write_outcome(&mut output, Outcome::Failed(e.into())),
    };
    let request = match workspace_protocol::request(head.method, head.target, &head.fields) {
        Ok(request) => request,
        Err(workspace_protocol::Rejected::Route) => {
            return write_outcome(&mut output, Outcome::Failed(Problem::Missing));
        }
        Err(_) => return write_outcome(&mut output, Outcome::Failed(Problem::Malformed)),
    };
    let mut bounded = match body_for(&request, &mut input) {
        Ok(body) => body,
        Err(problem) => return write_outcome(&mut output, Outcome::Failed(problem)),
    };
    let outcome = match permit.with_admission(account, |live| {
        // This is the final admission cut. Once admitted, normal stop does
        // not revoke the original operation's permit before it can finish.
        if stopping() {
            return Outcome::Failed(Problem::Unavailable);
        }
        dispatch(request, &mut bounded, live)
    }) {
        Ok(result) if matches!(result, Outcome::Failed(_)) || bounded.exhausted() => result,
        Ok(_) => Outcome::Failed(Problem::Unavailable),
        Err(e) => Outcome::Failed(e.into()),
    };
    write_outcome(&mut output, outcome)
}

fn headers(
    output: &mut impl Write,
    status: &str,
    media: Option<&str>,
    length: u64,
    challenge: bool,
) -> io::Result<()> {
    headers_with_digest(output, status, media, length, challenge, None)
}

fn headers_with_digest(
    output: &mut impl Write,
    status: &str,
    media: Option<&str>,
    length: u64,
    challenge: bool,
    digest: Option<[u8; 32]>,
) -> io::Result<()> {
    write!(
        output,
        "HTTP/1.1 {status}\r\nConnection: close\r\nCache-Control: no-store\r\n"
    )?;
    if let Some(media) = media {
        write!(output, "Content-Type: {media}\r\n")?;
    }
    if challenge {
        output.write_all(b"WWW-Authenticate: Basic realm=\"zxs\", charset=\"UTF-8\"\r\n")?;
    }
    write!(output, "Content-Length: {length}\r\n")?;
    if let Some(digest) = digest {
        output.write_all(b"Zxs-Sha256: ")?;
        for byte in digest {
            write!(output, "{byte:02x}")?;
        }
        output.write_all(b"\r\n")?;
    }
    output.write_all(b"\r\n")
}

fn write_outcome(output: &mut impl Write, outcome: Outcome) -> io::Result<()> {
    match outcome {
        Outcome::Json(bytes) => {
            headers(
                output,
                "200 OK",
                Some("application/json"),
                bytes.len() as u64,
                false,
            )?;
            output.write_all(&bytes)
        }
        Outcome::ReceiptAbsent => {
            const ABSENT: &[u8] = br#"{"state":"not_found"}"#;
            headers(
                output,
                "404 Not Found",
                Some("application/json"),
                ABSENT.len() as u64,
                false,
            )?;
            output.write_all(ABSENT)
        }
        Outcome::Node {
            length,
            sha256,
            body,
        } => {
            headers_with_digest(
                output,
                "200 OK",
                Some("application/octet-stream"),
                length,
                false,
                Some(sha256),
            )?;
            if io::copy(&mut body.take(length), output)? != length {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            Ok(())
        }
        Outcome::Failed(problem) => headers(
            output,
            problem.status(),
            None,
            0,
            matches!(problem, Problem::Unauthorized),
        ),
    }
}

#[cfg(test)]
#[path = "../tests/unit/attached/headless_http/tests.rs"]
mod tests;
