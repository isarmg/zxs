//! The additive upload wire. No legacy R3 verb or body is admitted here.
use super::{json::Parser, reply};
use crate::{
    container_store::{
        Body, Mutation, Target,
        upload::{Authorize, CHUNK, Chunk, Failure, Intent, MAX_BODY, Scope, Uploads},
    },
    hash::Sha256,
    headless_http::{Outcome, Problem},
    workspace_protocol::{Branch, Id, Rejected, decimal, field, hex, required_header},
};
use std::{
    fmt,
    io::{self, Read},
};

const ROOT: &str = "/zxs/v1/uploads";
const CONTAINERS: &str = "/zxs/v1/containers/";
const MAX_JSON: u64 = 4096;
const OP: &str = "zxs-op-id";
const CONTAINER: &str = "zxs-container-id";
const CHUNK_DIGEST: &str = "zxs-chunk-sha256";

#[derive(Clone, Copy)]
pub(crate) enum Frame {
    Begin {
        op: Id,
        length: u64,
    },
    Status {
        mutation: Mutation,
        upload: Option<[u8; 32]>,
    },
    Put(Chunk),
    Commit {
        mutation: Mutation,
        length: u64,
    },
}

impl Frame {
    pub(crate) fn extent(self) -> u64 {
        match self {
            Self::Begin { length, .. } | Self::Commit { length, .. } => length,
            Self::Put(chunk) => chunk.length,
            Self::Status { .. } => 0,
        }
    }

    pub(crate) fn raw(self) -> bool {
        matches!(self, Self::Put(_))
    }
}

fn commit_leaf(path: &str) -> Option<&str> {
    let (id, tail) = path.strip_prefix(CONTAINERS)?.split_once('/')?;
    (tail == "commits").then_some(id)
}

pub(crate) fn is_target(target: &str) -> bool {
    let path = target.split('?').next().unwrap_or_default();
    path == ROOT
        || path
            .strip_prefix(ROOT)
            .is_some_and(|tail| tail.starts_with('/'))
        || commit_leaf(path).is_some()
}

fn malformed(_: Rejected) -> Problem {
    Problem::Malformed
}

fn upload_id(value: &str) -> Result<[u8; 32], Problem> {
    let id = hex(value).map_err(malformed)?;
    if id == [0; 32] {
        return Err(Problem::Malformed);
    }
    Ok(id)
}

fn mutation(container: Id, op: Id) -> Mutation {
    Mutation {
        container: container.bytes(),
        request: op.bytes(),
    }
}

#[derive(Clone, Copy)]
enum Route<'a> {
    Begin,
    Status(Option<&'a str>),
    Put(&'a str, &'a str),
    Commit(&'a str),
}

fn route<'a>(method: &str, target: &'a str) -> Result<Route<'a>, Problem> {
    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (target, None),
    };
    if path == ROOT {
        if query.is_some() {
            return Err(Problem::Malformed);
        }
        return match method {
            "POST" => Ok(Route::Begin),
            "GET" => Ok(Route::Status(None)),
            _ => Err(Problem::Missing),
        };
    }
    if let Some(container) = commit_leaf(path) {
        if query.is_some() {
            return Err(Problem::Malformed);
        }
        return if method == "POST" {
            Ok(Route::Commit(container))
        } else {
            Err(Problem::Missing)
        };
    }
    let id = path
        .strip_prefix(ROOT)
        .and_then(|part| part.strip_prefix('/'))
        .ok_or(Problem::Missing)?;
    match method {
        "GET" if query.is_none() => Ok(Route::Status(Some(id))),
        "GET" => Err(Problem::Malformed),
        "PUT" => Ok(Route::Put(
            id,
            query
                .and_then(|raw| raw.strip_prefix("offset="))
                .ok_or(Problem::Malformed)?,
        )),
        _ => Err(Problem::Missing),
    }
}

fn allowed_headers(route: Route<'_>, headers: &[(&str, &str)]) -> Result<(), Problem> {
    for &(name, _) in headers {
        if [
            "transfer-encoding",
            "content-encoding",
            "range",
            "content-range",
            "expect",
            "trailer",
            "te",
            "upgrade",
        ]
        .iter()
        .any(|restricted| name.eq_ignore_ascii_case(restricted))
        {
            return Err(Problem::Malformed);
        }
        if name
            .as_bytes()
            .get(..4)
            .is_some_and(|front| front.eq_ignore_ascii_case(b"zxs-"))
            && !(name.eq_ignore_ascii_case(OP)
                || (matches!(route, Route::Status(_) | Route::Put(..))
                    && name.eq_ignore_ascii_case(CONTAINER))
                || (matches!(route, Route::Put(..)) && name.eq_ignore_ascii_case(CHUNK_DIGEST)))
        {
            return Err(Problem::Malformed);
        }
    }
    Ok(())
}

fn body_length(
    media: Option<&str>,
    length: Option<u64>,
    expected: &str,
    max: u64,
) -> Result<u64, Problem> {
    if media != Some(expected) {
        return Err(Problem::Malformed);
    }
    let length = length
        .filter(|length| *length > 0)
        .ok_or(Problem::Malformed)?;
    if length > max {
        return Err(Problem::TooLarge);
    }
    Ok(length)
}

pub(crate) fn frame(
    method: &str,
    target: &str,
    headers: &[(&str, &str)],
) -> Result<Frame, Problem> {
    let route = route(method, target)?;
    allowed_headers(route, headers)?;
    let length = field(headers, "content-length")
        .map_err(malformed)?
        .map(decimal)
        .transpose()
        .map_err(malformed)?;
    let media = field(headers, "content-type").map_err(malformed)?;
    let id = |name| Id::parse(required_header(headers, name)?);
    let op = id(OP).map_err(malformed)?;
    let json = || body_length(media, length, "application/json", MAX_JSON);
    match route {
        Route::Begin => Ok(Frame::Begin {
            op,
            length: json()?,
        }),
        Route::Status(id_text) => {
            if length.unwrap_or_default() != 0 || media.is_some() {
                return Err(Problem::Malformed);
            }
            Ok(Frame::Status {
                mutation: mutation(id(CONTAINER).map_err(malformed)?, op),
                upload: id_text.map(upload_id).transpose()?,
            })
        }
        Route::Put(upload, offset) => Ok(Frame::Put(Chunk::new(
            mutation(id(CONTAINER).map_err(malformed)?, op),
            upload_id(upload)?,
            decimal(offset).map_err(malformed)?,
            body_length(media, length, "application/octet-stream", CHUNK)?,
            hex(required_header(headers, CHUNK_DIGEST).map_err(malformed)?).map_err(malformed)?,
        ))),
        Route::Commit(container) => Ok(Frame::Commit {
            mutation: mutation(Id::parse(container).map_err(malformed)?, op),
            length: json()?,
        }),
    }
}

fn object(
    parser: &mut Parser<'_>,
    mut property: impl FnMut(&mut Parser<'_>, &str) -> Option<()>,
) -> Option<()> {
    parser.need('{')?;
    if parser.take('}') {
        return Some(());
    }
    loop {
        let key = parser.string()?;
        parser.need(':')?;
        property(parser, &key)?;
        if parser.take('}') {
            return Some(());
        }
        parser.need(',')?;
    }
}

fn once<T>(destination: &mut Option<T>, value: T) -> Option<()> {
    destination.replace(value).is_none().then_some(())
}

fn save_target(parser: &mut Parser<'_>) -> Option<Target> {
    let (mut kind, mut name, mut branch, mut head, mut from) = (None, None, None, None, None);
    object(parser, |p, key| match key {
        "kind" => once(&mut kind, p.string()?),
        "name" => once(&mut name, p.string()?),
        "branch" => {
            let label = p.string()?;
            Branch::parse(&label).ok()?;
            once(&mut branch, label)
        }
        "expected_head" => once(&mut head, Id::parse(&p.string()?).ok()?.bytes()),
        "from_node" => once(&mut from, Id::parse(&p.string()?).ok()?.bytes()),
        _ => None,
    })?;
    match (kind.as_deref()?, name, branch, head, from) {
        ("first_main", Some(name), None, None, None) => {
            crate::display::component(name.as_bytes(), crate::display::MAX).ok()?;
            Some(Target::FirstMain { name })
        }
        ("branch", None, Some(name), Some(expected_head), None) => Some(Target::Branch {
            name,
            expected_head,
        }),
        ("historical_main", None, Some(new_branch), None, Some(original))
            if new_branch != "main" =>
        {
            Some(Target::HistoricalMain {
                new_branch,
                original,
            })
        }
        _ => None,
    }
}

fn begin_json(raw: &str, scope: Scope, op: Id) -> Option<Intent> {
    let mut parser = Parser::new(raw);
    let (mut container, mut node, mut target, mut length, mut sha) = (None, None, None, None, None);
    object(&mut parser, |p, key| match key {
        "container_id" => once(&mut container, Id::parse(&p.string()?).ok()?),
        "node_id" => once(&mut node, Id::parse(&p.string()?).ok()?.bytes()),
        "target" => once(&mut target, save_target(p)?),
        "length" => once(&mut length, p.unsigned()?),
        "sha256" => once(&mut sha, hex(&p.string()?).ok()?),
        _ => None,
    })?;
    if !parser.space().is_empty() {
        return None;
    }
    Some(Intent {
        scope,
        mutation: mutation(container?, op),
        node: node?,
        target: target?,
        body: Body {
            length: length?,
            sha256: sha?,
        },
    })
}

fn commit_json(raw: &str) -> Option<[u8; 32]> {
    let mut parser = Parser::new(raw);
    let mut id = None;
    object(&mut parser, |p, key| {
        if key == "upload_id" {
            once(&mut id, upload_id(&p.string()?).ok()?)
        } else {
            None
        }
    })?;
    if !parser.space().is_empty() {
        return None;
    }
    id
}

#[derive(Debug)]
struct BodyIngress;

impl fmt::Display for BodyIngress {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str("upload ingress exceeded its body boundary")
    }
}
impl std::error::Error for BodyIngress {}

pub(crate) fn failed_ingress(kind: io::ErrorKind) -> io::Error {
    io::Error::new(kind, BodyIngress)
}

fn io_failure(error: io::Error) -> Problem {
    if error
        .get_ref()
        .is_some_and(<dyn std::error::Error + Send + Sync>::is::<BodyIngress>)
    {
        return Problem::InvalidBody;
    }
    match Failure::from(error) {
        Failure::Expired => Problem::BearerUnauthorized,
        _ => Problem::Unavailable,
    }
}

fn failed(error: Failure) -> Problem {
    match error {
        Failure::Io(error) => io_failure(error),
        Failure::Absent => Problem::Missing,
        Failure::Conflict => Problem::Conflict,
        Failure::OldEpoch => Problem::Denied,
        Failure::InvalidBody => Problem::InvalidBody,
        Failure::Expired => Problem::BearerUnauthorized,
    }
}

fn control(reader: &mut impl Read, length: u64) -> Result<String, Problem> {
    let mut bytes = Vec::new();
    reader
        .take(length + 1)
        .read_to_end(&mut bytes)
        .map_err(io_failure)?;
    if bytes.len() as u64 != length {
        return Err(Problem::InvalidBody);
    }
    String::from_utf8(bytes).map_err(|_| Problem::Malformed)
}

pub(crate) fn exchange<'store>(
    bind: impl FnOnce() -> io::Result<Uploads<'store>>,
    scope: Scope,
    frame: Frame,
    reader: &mut impl Read,
    authorize: Authorize<'_>,
) -> Result<Outcome, Problem> {
    authorize().map_err(io_failure)?;
    let lane = || bind().map_err(io_failure);
    match frame {
        Frame::Begin { op, length } => {
            let raw = control(reader, length)?;
            let intent = begin_json(&raw, scope, op).ok_or(Problem::Malformed)?;
            if intent.body.length > MAX_BODY {
                return Err(Problem::TooLarge);
            }
            if intent.body.length == 0 && intent.body.sha256 != Sha256::new().finish() {
                return Err(Problem::InvalidBody);
            }
            reply::upload(lane()?.begin(&intent, authorize).map_err(failed)?)
        }
        Frame::Status { mutation, upload } => {
            let progress = lane()?
                .status(mutation, upload, authorize)
                .map_err(|error| {
                    // A generic route 404 (including an older server) proves nothing.
                    // Only the authorized exact lookup can acknowledge absence.
                    match error {
                        Failure::Absent => Problem::UploadAbsent,
                        other => failed(other),
                    }
                })?;
            reply::upload(progress)
        }
        Frame::Put(chunk) => reply::upload(lane()?.put(chunk, reader, authorize).map_err(failed)?),
        Frame::Commit { mutation, length } => {
            let raw = control(reader, length)?;
            let upload = commit_json(&raw).ok_or(Problem::Malformed)?;
            let committed = lane()?
                .commit(mutation, upload, authorize)
                .map_err(failed)?;
            // Auxiliary retirement cannot replace the original receipt or
            // add fields to its existing wire DTO.
            reply::project(super::dispatch::Reply::Mutation(committed.into_receipt()))
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/upload_frames.rs"]
mod tests;
