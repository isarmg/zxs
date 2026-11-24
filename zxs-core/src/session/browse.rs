//! Opt-in metadata only, behind the same held workspace and verified H2 channel.
use super::{
    IDLE, Principal, collect_bounded_json, has_trailers,
    receipts::{BoundBearer, ReadPurpose},
    single,
};
use crate::{
    Result, invalid,
    r3::{self, Branch, Catalog, Detail, History, Id, MAX_JSON, ROOT},
};
use h2::RecvStream;
use http::{HeaderMap, HeaderValue, Request, Response, StatusCode, header};
use serde::{Deserialize, de::IgnoredAny};
use std::{collections::BTreeSet, fmt, sync::Arc};
use tokio::time::timeout;
use zeroize::Zeroizing;

const PAGE_ROWS: usize = 256;

#[derive(Debug, Eq, PartialEq)]
pub enum BrowseError {
    Missing,
    SelectionChanged,
    Unavailable,
    Transport,
    InvalidReply,
}

impl fmt::Display for BrowseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Missing => "the selected container was not found",
            Self::SelectionChanged => "the original metadata selection changed; select it again",
            Self::Unavailable => "the service could not verify the selected metadata",
            Self::Transport => "the original Browse exchange did not complete",
            Self::InvalidReply => "the response does not prove the selected metadata",
        })
    }
}

impl std::error::Error for BrowseError {}

/// Explicit Workspace-bound metadata session. No public raw request, token,
/// receipt, payload, checkout, mutation or reconnect capability.
pub struct BrowseSession {
    peer: BoundBearer,
}

impl BrowseSession {
    pub(crate) async fn connect_bound(
        original: &Principal,
        owner: Arc<()>,
        spelling: &str,
        account: &str,
        prompt: impl FnOnce() -> Result<Zeroizing<String>>,
        still_bound: impl Fn() -> Result<()>,
    ) -> Result<Self> {
        let peer = BoundBearer::connect_bound(
            ReadPurpose::Browse,
            original,
            owner,
            spelling,
            account,
            prompt,
            still_bound,
        )
        .await?;
        Ok(Self { peer })
    }

    pub(crate) fn belongs_to(&self, owner: &Arc<()>, original: &Principal) -> bool {
        self.peer.belongs_to(owner, original)
    }

    async fn get<T>(
        &self,
        request: Metadata<'_>,
        guard: impl Fn() -> Result<()>,
        decode_reply: impl FnOnce(&[u8]) -> Result<T>,
    ) -> Result<T> {
        let request = request.build()?;
        guard()?;
        let response = self
            .peer
            .read(request, &guard)
            .await?
            .ok_or(BrowseError::Transport)?;
        let decoded = match timeout(IDLE, receive(response)).await {
            Ok(Ok(bytes)) => decode_reply(&bytes),
            Ok(Err(error)) => Err(error),
            Err(error) => Err(error.into()),
        };
        guard()?;
        self.peer.require_live()?;
        decoded
    }

    pub(crate) async fn catalog(
        &self,
        after: Option<&Id>,
        guard: impl Fn() -> Result<()>,
    ) -> Result<Catalog> {
        self.get(Metadata::Catalog(after), guard, |bytes| {
            catalog_page(bytes, after)
        })
        .await
    }

    pub(crate) async fn detail(
        &self,
        container: &Id,
        sequence: Option<u64>,
        after: Option<&Branch>,
        guard: impl Fn() -> Result<()>,
    ) -> Result<Detail> {
        self.get(
            Metadata::Detail {
                container,
                sequence,
                after,
            },
            guard,
            |bytes| detail_page(bytes, container, sequence, after),
        )
        .await
    }

    pub(crate) async fn history(
        &self,
        container: &Id,
        branch: &Branch,
        head: &Id,
        next: Option<&Id>,
        guard: impl Fn() -> Result<()>,
    ) -> Result<History> {
        self.get(
            Metadata::History {
                container,
                branch,
                head,
                next,
            },
            guard,
            |bytes| history_page(bytes, head, next),
        )
        .await
    }

    pub(crate) async fn tags(
        &self,
        container: &Id,
        guard: impl Fn() -> Result<()>,
    ) -> Result<Vec<String>> {
        self.get(Metadata::Tags(container), guard, tags_reply).await
    }
}

enum Metadata<'a> {
    Catalog(Option<&'a Id>),
    Detail {
        container: &'a Id,
        sequence: Option<u64>,
        after: Option<&'a Branch>,
    },
    History {
        container: &'a Id,
        branch: &'a Branch,
        head: &'a Id,
        next: Option<&'a Id>,
    },
    Tags(&'a Id),
}

fn sequence_ok(sequence: u64) -> bool {
    sequence != 0 && sequence != u64::MAX
}

fn put(request: &mut Request<()>, name: &'static str, value: impl fmt::Display) -> Result<()> {
    request
        .headers_mut()
        .insert(name, HeaderValue::from_str(&value.to_string())?);
    Ok(())
}

impl Metadata<'_> {
    fn build(self) -> Result<Request<()>> {
        match self {
            Self::Catalog(after) => {
                let mut out = r3::json_request(ROOT.to_owned())?;
                put(&mut out, "zxs-view", "compact")?;
                if let Some(after) = after {
                    put(&mut out, "zxs-after", after)?;
                }
                Ok(out)
            }
            Self::Detail {
                container,
                sequence,
                after,
            } => {
                if (after.is_some() && sequence.is_none())
                    || sequence.is_some_and(|n| !sequence_ok(n))
                {
                    return Err(invalid(
                        "browse detail needs a valid original revision for its cursor",
                    ));
                }
                let mut out = r3::json_request(format!("{ROOT}/{container}"))?;
                put(&mut out, "zxs-detail", "paged")?;
                if let Some(sequence) = sequence {
                    put(&mut out, "zxs-detail-sequence", sequence)?;
                }
                if let Some(after) = after {
                    put(&mut out, "zxs-detail-after", after)?;
                }
                Ok(out)
            }
            Self::History {
                container,
                branch,
                head,
                next,
            } => {
                if next == Some(head) {
                    return Err(invalid(
                        "browse history cursor must differ from its original head",
                    ));
                }
                let mut out =
                    r3::json_request(format!("{ROOT}/{container}/branches/{branch}/nodes"))?;
                put(&mut out, "zxs-history-head", head)?;
                if let Some(next) = next {
                    put(&mut out, "zxs-history-next", next)?;
                }
                Ok(out)
            }
            Self::Tags(container) => r3::json_request(format!("{ROOT}/{container}/tags")),
        }
    }
}

fn read<V>(
    fields: &HeaderMap,
    key: &'static str,
    parse: impl FnOnce(&str) -> Result<V>,
) -> Result<Option<V>> {
    if !fields.contains_key(key) {
        return Ok(None);
    }
    let raw = single(fields, key).ok_or_else(|| invalid("duplicate Browse control header"))?;
    Ok(Some(parse(raw.to_str()?)?))
}

/// Defence in depth at the shared Bearer wire boundary: only the actual closed
/// constructors' exact requests can obtain a Browse Authorization header.
pub(super) fn require_request(request: &Request<()>) -> Result<()> {
    let path = request
        .uri()
        .path()
        .strip_prefix(ROOT)
        .ok_or_else(|| invalid("Browse does not authorize this route"))?;
    let fields = request.headers();
    let expected = if path.is_empty() {
        let after = read(fields, "zxs-after", Id::parse)?;
        Metadata::Catalog(after.as_ref()).build()?
    } else {
        let rest = path
            .strip_prefix('/')
            .ok_or_else(|| invalid("not a Browse target"))?;
        let parts: Vec<_> = rest.splitn(5, '/').collect();
        match parts.as_slice() {
            [c] => {
                let container = Id::parse(c)?;
                let sequence = read(fields, "zxs-detail-sequence", |raw| Ok(raw.parse::<u64>()?))?;
                let after = read(fields, "zxs-detail-after", Branch::parse)?;
                Metadata::Detail {
                    container: &container,
                    sequence,
                    after: after.as_ref(),
                }
                .build()?
            }
            [c, "branches", b, "nodes"] => {
                let container = Id::parse(c)?;
                let branch = Branch::parse(b)?;
                let head = read(fields, "zxs-history-head", Id::parse)?
                    .ok_or_else(|| invalid("Browse needs its original history head"))?;
                let next = read(fields, "zxs-history-next", Id::parse)?;
                Metadata::History {
                    container: &container,
                    branch: &branch,
                    head: &head,
                    next: next.as_ref(),
                }
                .build()?
            }
            [c, "tags"] => Metadata::Tags(&Id::parse(c)?).build()?,
            _ => return Err(invalid("Browse does not authorize this route")),
        }
    };
    if expected.uri() != request.uri()
        || expected.method() != request.method()
        || expected.headers() != fields
    {
        return Err(invalid("Browse requires exactly its typed bodyless GET"));
    }
    Ok(())
}

fn reply_status(status: StatusCode, fields: &HeaderMap) -> Result<Option<BrowseError>> {
    if single(fields, "cache-control").map(HeaderValue::as_bytes) != Some(&b"no-store"[..])
        || fields.contains_key(header::WWW_AUTHENTICATE)
    {
        return Err(BrowseError::InvalidReply.into());
    }
    let problem = match status {
        StatusCode::OK => return Ok(None),
        StatusCode::NOT_FOUND => BrowseError::Missing,
        StatusCode::CONFLICT => BrowseError::SelectionChanged,
        StatusCode::SERVICE_UNAVAILABLE => BrowseError::Unavailable,
        _ => return Err(BrowseError::InvalidReply.into()),
    };
    if single(fields, "content-length").map(HeaderValue::as_bytes) != Some(&b"0"[..])
        || [
            header::CONTENT_TYPE,
            header::CONTENT_RANGE,
            header::CONTENT_ENCODING,
            header::TRANSFER_ENCODING,
            header::TRAILER,
        ]
        .iter()
        .any(|h| fields.contains_key(h))
    {
        return Err(BrowseError::InvalidReply.into());
    }
    Ok(Some(problem))
}

async fn receive(response: Response<RecvStream>) -> Result<Vec<u8>> {
    if let Some(problem) = reply_status(response.status(), response.headers())? {
        let mut stream = response.into_body();
        while let Some(part) = stream.data().await {
            if !part?.is_empty() {
                return Err(BrowseError::InvalidReply.into());
            }
        }
        if has_trailers(&mut stream).await? {
            return Err(BrowseError::InvalidReply.into());
        }
        return Err(problem.into());
    }
    collect_bounded_json(response, MAX_JSON).await
}

// Match the existing R3 DTOs exactly without narrowing older Basic decoders:
// nullable canonical fields in the Browse response must not be omitted.
#[derive(Deserialize)]
struct RequiredAfter {
    #[serde(rename = "next_after")]
    _cursor: IgnoredAny,
}

#[derive(Deserialize)]
struct RequiredNext {
    #[serde(rename = "next")]
    _cursor: IgnoredAny,
    #[serde(rename = "nodes")]
    _nodes: Vec<RequiredParent>,
}

#[derive(Deserialize)]
struct RequiredParent {
    #[serde(rename = "parent_node")]
    _parent: IgnoredAny,
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    Ok(serde_json::from_slice(bytes).map_err(|_| BrowseError::InvalidReply)?)
}

fn page_cursor<T, K: Ord>(
    rows: &[T],
    after: Option<&K>,
    next: Option<&K>,
    key: impl Fn(&T) -> &K,
) -> bool {
    rows.len() <= PAGE_ROWS
        && rows.windows(2).all(|p| key(&p[0]) < key(&p[1]))
        && after.is_none_or(|after| rows.first().is_none_or(|row| after < key(row)))
        && next == rows.last().filter(|_| rows.len() == PAGE_ROWS).map(key)
}

fn catalog_page(bytes: &[u8], after: Option<&Id>) -> Result<Catalog> {
    decode::<RequiredAfter>(bytes)?;
    let page: Catalog = decode(bytes)?;
    if !page.items.iter().all(|row| sequence_ok(row.sequence))
        || !page_cursor(&page.items, after, page.next_after.as_ref(), |row| {
            &row.container_id
        })
    {
        return Err(BrowseError::InvalidReply.into());
    }
    Ok(page)
}

fn detail_page(
    bytes: &[u8],
    container: &Id,
    sequence: Option<u64>,
    after: Option<&Branch>,
) -> Result<Detail> {
    decode::<RequiredAfter>(bytes)?;
    let page: Detail = decode(bytes)?;
    if &page.container_id != container
        || !sequence_ok(page.sequence)
        || sequence.is_some_and(|n| n != page.sequence)
        || !valid_tags(&page.tags)
        || !page_cursor(&page.heads, after, page.next_after.as_ref(), |row| {
            &row.branch
        })
    {
        return Err(BrowseError::InvalidReply.into());
    }
    Ok(page)
}

fn history_page(bytes: &[u8], head: &Id, next: Option<&Id>) -> Result<History> {
    decode::<RequiredNext>(bytes)?;
    let page: History = decode(bytes)?;
    let first = next.unwrap_or(head);
    let mut seen = BTreeSet::new();
    if &page.head != head
        || page.nodes.first().is_none_or(|node| &node.node_id != first)
        || page.nodes.len() > PAGE_ROWS
        || page.nodes.iter().any(|node| {
            !sequence_ok(node.sequence)
                || node.parent_node.as_ref() == Some(&node.node_id)
                || !seen.insert(&node.node_id)
        })
        || (next.is_some() && seen.contains(head))
        || page.nodes.windows(2).any(|pair| {
            pair[0].parent_node.as_ref() != Some(&pair[1].node_id)
                || pair[0].sequence <= pair[1].sequence
        })
        || page.next.as_ref() != page.nodes.last().and_then(|node| node.parent_node.as_ref())
        || page.next.as_ref().is_some_and(|cursor| {
            page.nodes.len() != PAGE_ROWS || cursor == head || seen.contains(cursor)
        })
    {
        return Err(BrowseError::InvalidReply.into());
    }
    Ok(page)
}

fn valid_tags(tags: &[String]) -> bool {
    tags.iter()
        .all(|tag| !tag.is_empty() && !tag.contains('\0'))
}

fn tags_reply(bytes: &[u8]) -> Result<Vec<String>> {
    let tags: Vec<String> = decode(bytes)?;
    if !valid_tags(&tags) {
        return Err(BrowseError::InvalidReply.into());
    }
    Ok(tags)
}

#[cfg(test)]
#[path = "../../tests/unit/browse_cases.rs"]
mod tests;
