//! Strict HTTP/1.1 behind a deployment-managed frontend. No TLS or forwarding policy.
mod payload;
mod reuse;
#[cfg(test)]
pub(crate) mod upload;

pub(crate) use payload::PayloadReply;
use payload::{ContentRange, write_payload};
pub(super) use reuse::ReuseBudget;
use reuse::{BackendProfile, MAX_EXCHANGES, NEXT_HEAD, can_reuse, profile, session_outcome};

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
        Request::ListCompact { .. }
        | Request::ContainerDetail { .. }
        | Request::Tags(_)
        | Request::NodeData { .. }
        | Request::NodePayload(_)
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
    Payload(PayloadReply),
    Failed(Problem),
}

#[derive(Clone, Copy)]
pub(crate) enum Problem {
    Malformed,
    Unauthorized,
    BearerUnauthorized,
    LoginThrottled(u64),
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
            Self::Unauthorized | Self::BearerUnauthorized => "401 Unauthorized",
            Self::LoginThrottled(_) => "429 Too Many Requests",
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
) -> io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    let mut ended = 0;
    loop {
        before_read()?;
        let available = input.fill_buf()?;
        if available.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err(io::ErrorKind::UnexpectedEof.into())
            };
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
                // Another head cannot start until this exact exchange settles.
                input.consume(offset + 1);
                return Ok(Some(bytes));
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
        // Neither transport profile accepts transfer coding, trailers,
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

/// One bounded reader serves the accepted socket through complete sequential v1
/// exchanges. A legacy Basic request still performs one exchange and closes.
pub(crate) fn serve_connection(
    account: &Account,
    stream: &TcpStream,
    stopping: &dyn Fn() -> bool,
    budget: &ReuseBudget,
    dispatch: impl Fn(Request, &mut Incoming<'_>, &dyn Fn() -> bool) -> Outcome,
) -> io::Result<()> {
    let mut input = BufReader::new(stream);
    let mut output = stream;
    let mut reuse_slot = None;
    for served in 0..MAX_EXCHANGES {
        let initial = served == 0;
        let raw = match next_head(&mut input, stream, stopping, initial) {
            Ok(Some(raw)) => raw,
            Ok(None) if !initial => return Ok(()),
            Ok(None) => return write_problem(&mut output, Problem::Malformed),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof
                ) =>
            {
                return write_problem(&mut output, Problem::Malformed);
            }
            Err(error) => return Err(error),
        };
        if stopping() {
            return Ok(());
        }
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        let head = match head(&raw) {
            Ok(head) => head,
            Err(problem) => return write_problem(&mut output, problem),
        };
        let mode = profile(&head);
        let outcome = match mode {
            BackendProfile::Login => session_outcome(account, &head, stopping),
            BackendProfile::Legacy | BackendProfile::Bearer => {
                let Some(outcome) = resource(account, &head, &mut input, mode, stopping, &dispatch)
                else {
                    // Preserve the legacy cut: a stop during authentication
                    // closes quietly rather than admitting or reporting new work.
                    return Ok(());
                };
                outcome
            }
        };
        let eligible = mode != BackendProfile::Legacy
            && served + 1 < MAX_EXCHANGES
            && !stopping()
            && !matches!(outcome, Outcome::Failed(_))
            && can_reuse(&head);
        if eligible && reuse_slot.is_none() {
            reuse_slot = budget.try_hold();
        }
        let keep = eligible && reuse_slot.is_some();
        write_outcome(&mut output, outcome, keep)?;
        output.flush()?;
        if !keep {
            return Ok(());
        }
    }
    Ok(())
}

fn next_head(
    input: &mut impl BufRead,
    stream: &TcpStream,
    stopping: &dyn Fn() -> bool,
    initial: bool,
) -> io::Result<Option<Vec<u8>>> {
    let deadline = Instant::now() + if initial { IO_TIMEOUT } else { NEXT_HEAD };
    let before = || {
        if stopping() {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        stream.set_read_timeout(Some(left))
    };
    let raw = read_head(input, before)?;
    if Instant::now() >= deadline {
        return Err(io::ErrorKind::TimedOut.into());
    }
    Ok(raw)
}

fn resource(
    account: &Account,
    head: &Head<'_>,
    reader: &mut dyn Read,
    mode: BackendProfile,
    stopping: &dyn Fn() -> bool,
    dispatch: &impl Fn(Request, &mut Incoming<'_>, &dyn Fn() -> bool) -> Outcome,
) -> Option<Outcome> {
    let bearer = mode == BackendProfile::Bearer;
    let auth_problem = |error: AuthError| {
        if bearer && error == AuthError::Invalid {
            Problem::BearerUnauthorized
        } else {
            error.into()
        }
    };
    let permit = if bearer {
        account.authenticate_bearer(&head.fields, stopping)
    } else {
        account.authenticate(&head.fields, stopping)
    };
    let permit = match permit {
        Ok(permit) => permit,
        Err(_) if stopping() => return None,
        Err(error) => return Some(Outcome::Failed(auth_problem(error))),
    };
    let frame = if bearer && workspace_protocol::is_payload_target(head.target) {
        workspace_protocol::payload_request(head.method, head.target, &head.fields)
    } else {
        workspace_protocol::request(head.method, head.target, &head.fields)
    };
    let request = match frame {
        Ok(request) => request,
        Err(workspace_protocol::Rejected::Route) => return Some(Outcome::Failed(Problem::Missing)),
        Err(_) => return Some(Outcome::Failed(Problem::Malformed)),
    };
    if bearer && !matches!(request, Request::Receipt { .. } | Request::NodePayload(_)) {
        // v1 authorizes exact read-only receipts and the distinct immutable
        // payload path. All other current R3 routes remain Basic-only.
        return Some(Outcome::Failed(Problem::Denied));
    }
    let mut bounded = match body_for(&request, reader) {
        Ok(body) => body,
        Err(problem) => return Some(Outcome::Failed(problem)),
    };
    Some(
        match permit.with_admission(account, |live| {
            // No new work on stop. An already admitted operation can finish and
            // report its exact receipt if it completed with live authorization.
            if stopping() {
                return Outcome::Failed(Problem::Unavailable);
            }
            dispatch(request, &mut bounded, live)
        }) {
            Ok(result) if matches!(result, Outcome::Failed(_)) || bounded.exhausted() => result,
            Ok(_) => Outcome::Failed(Problem::Unavailable),
            Err(error) => Outcome::Failed(auth_problem(error)),
        },
    )
}

#[derive(Clone, Copy, Default)]
struct HeaderExtras {
    challenge: Option<&'static str>,
    retry_after: Option<u64>,
    digest: Option<[u8; 32]>,
    etag: Option<[u8; 32]>,
    content_range: Option<ContentRange>,
    accept_ranges: bool,
}

fn headers(
    output: &mut impl Write,
    status: &str,
    media: Option<&str>,
    length: u64,
    keep: bool,
    extras: HeaderExtras,
) -> io::Result<()> {
    let connection = if keep { "keep-alive" } else { "close" };
    write!(
        output,
        "HTTP/1.1 {status}\r\nConnection: {connection}\r\nCache-Control: no-store\r\n"
    )?;
    if let Some(media) = media {
        write!(output, "Content-Type: {media}\r\n")?;
    }
    if let Some(challenge) = extras.challenge {
        write!(output, "WWW-Authenticate: {challenge}\r\n")?;
    }
    if let Some(seconds) = extras.retry_after {
        write!(output, "Retry-After: {seconds}\r\n")?;
    }
    write!(output, "Content-Length: {length}\r\n")?;
    if let Some(digest) = extras.digest {
        output.write_all(b"Zxs-Sha256: ")?;
        hex_digest(output, digest)?;
        output.write_all(b"\r\n")?;
    }
    if extras.accept_ranges {
        output.write_all(b"Accept-Ranges: bytes\r\n")?;
    }
    if let Some(tag) = extras.etag {
        output.write_all(b"ETag: \"")?;
        hex_digest(output, tag)?;
        output.write_all(b"\"\r\n")?;
    }
    if let Some(range) = extras.content_range {
        match range {
            ContentRange::Interval { start, end, total } => {
                write!(output, "Content-Range: bytes {start}-{end}/{total}\r\n")?;
            }
            ContentRange::NoneAt { total } => {
                write!(output, "Content-Range: bytes */{total}\r\n")?;
            }
        }
    }
    output.write_all(b"\r\n")
}

fn hex_digest(output: &mut impl Write, digest: [u8; 32]) -> io::Result<()> {
    for byte in digest {
        write!(output, "{byte:02x}")?;
    }
    Ok(())
}

fn write_problem(output: &mut impl Write, problem: Problem) -> io::Result<()> {
    let mut extras = HeaderExtras::default();
    match problem {
        Problem::Unauthorized => extras.challenge = Some("Basic realm=\"zxs\", charset=\"UTF-8\""),
        Problem::BearerUnauthorized => extras.challenge = Some("Bearer"),
        Problem::LoginThrottled(seconds) => extras.retry_after = Some(seconds),
        _ => {}
    }
    headers(output, problem.status(), None, 0, false, extras)
}

fn write_outcome(output: &mut impl Write, outcome: Outcome, keep: bool) -> io::Result<()> {
    match outcome {
        Outcome::Json(bytes) => {
            headers(
                output,
                "200 OK",
                Some("application/json"),
                bytes.len() as u64,
                keep,
                HeaderExtras::default(),
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
                keep,
                HeaderExtras::default(),
            )?;
            output.write_all(ABSENT)
        }
        Outcome::Node {
            length,
            sha256,
            body,
        } => {
            headers(
                output,
                "200 OK",
                Some("application/octet-stream"),
                length,
                keep,
                HeaderExtras {
                    digest: Some(sha256),
                    ..HeaderExtras::default()
                },
            )?;
            if io::copy(&mut body.take(length), output)? != length {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            Ok(())
        }
        Outcome::Payload(reply) => write_payload(output, reply, keep),
        Outcome::Failed(problem) => write_problem(output, problem),
    }
}

#[cfg(test)]
#[path = "../tests/unit/attached/headless_http/tests.rs"]
mod tests;
