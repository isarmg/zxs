//! Additive v1 upload wire only. No raw URL, legacy write, or token escapes.
use crate::{
    Result, hex, invalid,
    r3::{self, Digest, Id, Intent, Target},
};
use bytes::Bytes;
use http::{Method, Request, header};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

const UPLOADS: &str = "/zxs/v1/uploads";
pub(crate) const BLOCK: u64 = 8 * 1024 * 1024;
const MAX_FILE: u64 = 1 << 40;
pub(super) const JSON_LIMIT: u64 = 4096;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Digest", into = "Digest")]
pub(crate) struct UploadId(Digest);

impl TryFrom<Digest> for UploadId {
    type Error = &'static str;
    fn try_from(value: Digest) -> std::result::Result<Self, Self::Error> {
        if value.as_str().bytes().all(|b| b == b'0') {
            return Err("zero upload ID");
        }
        Ok(Self(value))
    }
}

impl From<UploadId> for Digest {
    fn from(id: UploadId) -> Self {
        id.0
    }
}

fn length_ok(intent: &Intent) -> Result<()> {
    intent.validate()?;
    if intent.body.length > MAX_FILE
        || (intent.body.length == 0 && intent.body.sha256.as_str() != hex(&Sha256::digest([])))
    {
        return Err(invalid("invalid frozen upload length or empty digest"));
    }
    Ok(())
}

pub(crate) fn valid_offset(offset: u64, total: u64) -> bool {
    total <= MAX_FILE && (offset == total || (offset < total && offset.is_multiple_of(BLOCK)))
}

pub(crate) fn block_length(offset: u64, total: u64) -> Result<usize> {
    if !valid_offset(offset, total) || offset >= total {
        return Err(invalid("not an original chunk boundary"));
    }
    Ok(usize::try_from(BLOCK.min(total - offset))?)
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Stage {
    Open,
    Complete,
    Committed,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Progress {
    pub(crate) upload_id: UploadId,
    container_id: Id,
    operation_id: Id,
    node_id: Id,
    target: Target,
    length: u64,
    sha256: Digest,
    pub(crate) next_offset: u64,
    pub(crate) state: Stage,
}

impl Progress {
    pub(super) fn decode(bytes: &[u8], original: &Intent, id: Option<&UploadId>) -> Result<Self> {
        length_ok(original)?;
        let got: Self = serde_json::from_slice(bytes)?;
        if id.is_some_and(|u| u != &got.upload_id)
            || got.container_id != original.container_id
            || got.operation_id != original.operation_id
            || got.node_id != original.node_id
            || got.target != original.target
            || got.length != original.body.length
            || got.sha256 != original.body.sha256
            || !valid_offset(got.next_offset, got.length)
            || (got.state == Stage::Open) != (got.next_offset < got.length)
        {
            return Err(invalid(
                "upload reply did not match the complete frozen selection",
            ));
        }
        Ok(got)
    }
}

pub(super) struct Frame {
    pub(super) request: Request<()>,
    pub(super) bytes: Bytes,
}

fn frame(
    intent: &Intent,
    method: Method,
    path: String,
    cid: bool,
    media: Option<&'static str>,
    digest: Option<String>,
    bytes: Bytes,
) -> Result<Frame> {
    length_ok(intent)?;
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::ACCEPT, "application/json")
        .header(header::ACCEPT_ENCODING, "identity")
        .header("zxs-op-id", intent.operation_id.as_str());
    if cid {
        request = request.header("zxs-container-id", intent.container_id.as_str());
    }
    if let Some(media) = media {
        request = request
            .header(header::CONTENT_TYPE, media)
            .header(header::CONTENT_LENGTH, bytes.len().to_string());
    }
    if let Some(digest) = digest {
        request = request.header("zxs-chunk-sha256", digest);
    }
    Ok(Frame {
        request: request.body(())?,
        bytes,
    })
}

#[derive(Serialize)]
struct Begin<'a> {
    container_id: &'a Id,
    node_id: &'a Id,
    target: &'a Target,
    length: u64,
    sha256: &'a Digest,
}

pub(super) fn begin(i: &Intent) -> Result<Frame> {
    let data = serde_json::to_vec(&Begin {
        container_id: &i.container_id,
        node_id: &i.node_id,
        target: &i.target,
        length: i.body.length,
        sha256: &i.body.sha256,
    })?;
    frame(
        i,
        Method::POST,
        UPLOADS.into(),
        false,
        Some("application/json"),
        None,
        data.into(),
    )
}

pub(super) fn status(i: &Intent, id: Option<&UploadId>) -> Result<Frame> {
    let path = id.map_or_else(|| UPLOADS.to_owned(), |u| format!("{UPLOADS}/{}", u.0));
    frame(i, Method::GET, path, true, None, None, Bytes::new())
}

pub(super) fn put(i: &Intent, id: &UploadId, offset: u64, data: Bytes) -> Result<Frame> {
    if data.len() != block_length(offset, i.body.length)? {
        return Err(invalid("not a complete original upload chunk"));
    }
    let digest = hex(&Sha256::digest(&data));
    frame(
        i,
        Method::PUT,
        format!("{UPLOADS}/{}?offset={offset}", id.0),
        true,
        Some("application/octet-stream"),
        Some(digest),
        data,
    )
}

pub(super) fn commit(i: &Intent, u: &UploadId) -> Result<Frame> {
    #[derive(Serialize)]
    struct Commit<'a> {
        upload_id: &'a UploadId,
    }
    frame(
        i,
        Method::POST,
        format!("{}/{}/commits", r3::ROOT, i.container_id),
        false,
        Some("application/json"),
        None,
        serde_json::to_vec(&Commit { upload_id: u })?.into(),
    )
}

#[cfg(test)]
#[path = "../../../tests/unit/upload_wire_cases.rs"]
mod tests;
