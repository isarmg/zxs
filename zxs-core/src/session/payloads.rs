//! One bounded span of one already-verified immutable remote node.
use super::{
    IDLE, Principal,
    receipts::{BoundBearer, ReadPurpose},
    single,
};
use crate::{
    Result, hex, invalid,
    r3::{Body, Digest, Id, ROOT},
};
use h2::RecvStream;
use http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header};
use sha2::{Digest as _, Sha256};
use std::{fmt, io::Write, sync::Arc, time::Duration};
use tokio::time::timeout;
use zeroize::Zeroizing;

const MAX_WINDOW: u64 = 8 << 20;
const WINDOW_DEADLINE: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ByteWindow {
    first: u64,
    last: u64,
}

impl ByteWindow {
    /// Bounds one request, not the length of the remote file.
    pub fn new(start: u64, length: u64) -> Result<Self> {
        if !(1..=MAX_WINDOW).contains(&length) {
            return Err(PayloadError::InvalidWindow.into());
        }
        Ok(Self {
            first: start,
            last: start
                .checked_add(length - 1)
                .ok_or(PayloadError::InvalidWindow)?,
        })
    }

    fn intersection(self, total: u64) -> Option<(u64, u64)> {
        if self.first >= total {
            return None;
        }
        let last = self.last.min(total - 1);
        Some((last, last - self.first + 1))
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PayloadError {
    InvalidWindow,
    MissingBaseline,
    Validator(&'static str),
    WholeFallback,
    Unsatisfiable(u64),
    ShortBody { expected: u64, seen: u64 },
    ExtraBody,
    Transport,
}

impl fmt::Display for PayloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidWindow => {
                f.write_str("select one nonempty slice of at most 8 MiB without offset overflow")
            }
            Self::MissingBaseline => {
                f.write_str("no confirmed remote baseline exists for that workspace path")
            }
            Self::Validator(name) => write!(f, "selected payload slice failed validator: {name}"),
            Self::WholeFallback => f.write_str(
                "server returned a whole body, not the requested slice; nothing published",
            ),
            Self::Unsatisfiable(total) => write!(
                f,
                "selected slice has no bytes in the original {total}-byte payload"
            ),
            Self::ShortBody { expected, seen } => write!(
                f,
                "selected payload stream supplied only {seen} of the expected {expected} slice bytes"
            ),
            Self::ExtraBody => f.write_str("selected payload exceeded the exact slice length"),
            Self::Transport => {
                f.write_str("selected payload transport did not complete; nothing published")
            }
        }
    }
}

impl std::error::Error for PayloadError {}

// Each local choice gets a separate private identity, even for equal wire fields.
// The approved choice owns the immutable wire request, never caller-supplied GET inputs.
#[derive(Clone)]
pub(crate) struct PayloadSelection(Arc<SelectedPayload>);

struct SelectedPayload {
    container: Id,
    node: Id,
    body: Body,
    window: ByteWindow,
}

impl PayloadSelection {
    pub(crate) fn new(container: Id, node: Id, body: Body, window: ByteWindow) -> Self {
        Self(Arc::new(SelectedPayload {
            container,
            node,
            body,
            window,
        }))
    }

    fn require_same(&self, requested: &Self) -> Result<()> {
        if !Arc::ptr_eq(&self.0, &requested.0) {
            return Err(invalid(
                "the payload session did not approve this exact slice choice",
            ));
        }
        Ok(())
    }
}

/// A separate capability for one exact original payload choice. It cannot query
/// receipts, read metadata, call legacy data, submit, or expose its credential.
pub struct PayloadSession {
    peer: BoundBearer,
    selection: PayloadSelection,
}

impl PayloadSession {
    pub(crate) async fn connect_bound(
        original: &Principal,
        owner: Arc<()>,
        selection: &PayloadSelection,
        spelling: &str,
        account: &str,
        prompt: impl FnOnce() -> Result<Zeroizing<String>>,
        still_bound: impl Fn() -> Result<()>,
    ) -> Result<Self> {
        let peer = BoundBearer::connect_bound(
            ReadPurpose::Payload,
            original,
            owner,
            spelling,
            account,
            prompt,
            still_bound,
        )
        .await?;
        Ok(Self {
            peer,
            selection: selection.clone(),
        })
    }

    pub(crate) fn require_choice(
        &self,
        owner: &Arc<()>,
        original: &Principal,
        requested: &PayloadSelection,
    ) -> Result<()> {
        if !self.peer.belongs_to(owner, original) {
            return Err(invalid(
                "the payload session has a different original workspace",
            ));
        }
        self.selection.require_same(requested)
    }

    pub(crate) async fn read_slice(
        &self,
        out: &mut impl Write,
        before_get: impl FnOnce() -> Result<()>,
    ) -> Result<Body> {
        let SelectedPayload {
            container,
            node,
            body: selected,
            window,
        } = &*self.selection.0;
        let window = *window;
        let reply = self
            .peer
            .read(
                slice_request(container, node, selected, window)?,
                before_get,
            )
            .await?
            .ok_or(PayloadError::Transport)?;
        let head = validate_head(reply.status(), reply.headers(), selected, window)?;
        let stream = reply.into_body();
        match head {
            Head::Part(length) => {
                let body = bounded_body(stream, length, out).await?;
                // A span does not prove the whole digest unless it covers the whole object.
                if window.first == 0 && length == selected.length && &body != selected {
                    return Err(PayloadError::Validator("whole-span SHA-256").into());
                }
                Ok(body)
            }
            Head::Empty => {
                bounded_body(stream, 0, std::io::sink()).await?;
                Err(PayloadError::Unsatisfiable(selected.length).into())
            }
            Head::Whole => Err(PayloadError::WholeFallback.into()),
        }
    }
}

fn strong_etag(selected: &Body) -> String {
    format!("\"{}\"", selected.sha256)
}

fn slice_request(
    container: &Id,
    node: &Id,
    selected: &Body,
    window: ByteWindow,
) -> Result<Request<()>> {
    Ok(Request::builder()
        .method(Method::GET)
        .uri(format!("{ROOT}/{container}/nodes/{node}/payload"))
        .header(header::ACCEPT, "application/octet-stream")
        .header(header::ACCEPT_ENCODING, "identity")
        .header(
            header::RANGE,
            format!("bytes={}-{}", window.first, window.last),
        )
        .header(header::IF_RANGE, strong_etag(selected))
        .body(())?)
}

#[derive(Debug, Eq, PartialEq)]
enum Head {
    Part(u64),
    Empty,
    Whole,
}

fn require_header(headers: &HeaderMap, name: &'static str, expected: &str) -> Result<()> {
    if single(headers, name).map(HeaderValue::as_bytes) != Some(expected.as_bytes()) {
        return Err(PayloadError::Validator(name).into());
    }
    Ok(())
}

fn forbid_header(headers: &HeaderMap, name: &'static str) -> Result<()> {
    if headers.contains_key(name) {
        return Err(PayloadError::Validator(name).into());
    }
    Ok(())
}

fn validate_head(
    status: StatusCode,
    headers: &HeaderMap,
    selected: &Body,
    window: ByteWindow,
) -> Result<Head> {
    if !matches!(
        status,
        StatusCode::OK | StatusCode::PARTIAL_CONTENT | StatusCode::RANGE_NOT_SATISFIABLE
    ) {
        return Err(PayloadError::Validator("range response status").into());
    }
    require_header(headers, "accept-ranges", "bytes")?;
    for name in ["content-encoding", "transfer-encoding", "trailer"] {
        forbid_header(headers, name)?;
    }

    if status == StatusCode::RANGE_NOT_SATISFIABLE {
        require_header(headers, "content-length", "0")?;
        require_header(
            headers,
            "content-range",
            &format!("bytes */{}", selected.length),
        )?;
        for name in ["etag", "content-type", "zxs-sha256"] {
            forbid_header(headers, name)?;
        }
        if window.intersection(selected.length).is_some() {
            return Err(PayloadError::Validator("416 for a satisfiable span").into());
        }
        return Ok(Head::Empty);
    }

    require_header(headers, "content-type", "application/octet-stream")?;
    require_header(headers, "etag", &strong_etag(selected))?;
    if status == StatusCode::OK {
        forbid_header(headers, "content-range")?;
        require_header(headers, "content-length", &selected.length.to_string())?;
        require_header(headers, "zxs-sha256", selected.sha256.as_str())?;
        return Ok(Head::Whole); // Never consume or publish whole-body fallback.
    }

    forbid_header(headers, "zxs-sha256")?;
    let (last, length) = window
        .intersection(selected.length)
        .ok_or(PayloadError::Validator("206 for an unsatisfiable span"))?;
    require_header(
        headers,
        "content-range",
        &format!("bytes {}-{last}/{}", window.first, selected.length),
    )?;
    require_header(headers, "content-length", &length.to_string())?;
    Ok(Head::Part(length))
}

struct ExactBody {
    expected: u64,
    seen: u64,
    sha: Sha256,
}

impl ExactBody {
    fn new(expected: u64) -> Self {
        Self {
            expected,
            seen: 0,
            sha: Sha256::new(),
        }
    }

    fn push(&mut self, bytes: &[u8], mut out: impl Write) -> Result<()> {
        let n = bytes.len() as u64;
        if n > self.expected - self.seen {
            return Err(PayloadError::ExtraBody.into());
        }
        out.write_all(bytes)?;
        self.sha.update(bytes);
        self.seen += n;
        Ok(())
    }

    fn interrupted(&self) -> PayloadError {
        if self.seen < self.expected {
            PayloadError::ShortBody {
                expected: self.expected,
                seen: self.seen,
            }
        } else {
            PayloadError::Transport
        }
    }

    fn finish(self) -> Result<Body> {
        if self.seen != self.expected {
            return Err(PayloadError::ShortBody {
                expected: self.expected,
                seen: self.seen,
            }
            .into());
        }
        Ok(Body {
            length: self.seen,
            sha256: Digest::parse(&hex(&self.sha.finalize()))?,
        })
    }
}

async fn bounded_body(mut stream: RecvStream, length: u64, mut out: impl Write) -> Result<Body> {
    timeout(WINDOW_DEADLINE, async {
        let mut measured = ExactBody::new(length);
        while let Some(part) = timeout(IDLE, stream.data())
            .await
            .map_err(|_| PayloadError::Transport)?
        {
            // h2 can surface a short Content-Length as a stream error rather than EOF.
            let part = part.map_err(|_| measured.interrupted())?;
            measured.push(&part, &mut out)?;
            stream.flow_control().release_capacity(part.len())?;
        }
        if timeout(IDLE, stream.trailers())
            .await
            .map_err(|_| PayloadError::Transport)??
            .is_some()
        {
            return Err(PayloadError::Validator("unsupported trailers").into());
        }
        measured.finish()
    })
    .await
    .map_err(|_| PayloadError::Transport)?
}

#[cfg(test)]
#[path = "../../tests/unit/payload_cases.rs"]
mod tests;
