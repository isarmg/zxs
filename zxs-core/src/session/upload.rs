//! One explicit upload action on the existing verified channel. No retry or new login here.
use super::{
    ExistingBasicSession, IDLE, collect_bounded_json, has_trailers,
    receipts::{Bearer, issue_token},
    single,
};
use crate::{
    Result, invalid,
    r3::{self, Intent, Lookup},
};
use bytes::Bytes;
use h2::RecvStream;
use http::{HeaderMap, HeaderValue, Response, StatusCode};
use std::{future::poll_fn, sync::atomic::Ordering, time::Instant};
use tokio::time::{timeout, timeout_at};

mod wire;
use wire::{Frame, JSON_LIMIT};
pub(crate) use wire::{Progress, Stage, UploadId, block_length, valid_offset};

pub(crate) struct Exchange<'a> {
    basic: &'a ExistingBasicSession,
    token: Bearer,
}

impl<'a> Exchange<'a> {
    pub(crate) async fn login(
        basic: &'a ExistingBasicSession,
        guard: impl Fn() -> Result<()>,
    ) -> Result<Self> {
        guard()?;
        let token = issue_token(&basic.session.channel, &basic.session.auth, guard).await?;
        Ok(Self { basic, token })
    }

    pub(crate) fn live(&self) -> Result<()> {
        self.token.require_live(Instant::now())
    }

    async fn send(
        &self,
        Frame { request, mut bytes }: Frame,
        guard: impl Fn() -> Result<()>,
        verifying_full_file: bool,
    ) -> Result<Response<RecvStream>> {
        let guard = || {
            guard()?;
            self.live()
        };
        let ordinary_wait = match *request.method() {
            http::Method::PUT => std::time::Duration::from_secs(210),
            http::Method::POST => std::time::Duration::from_secs(90),
            _ => IDLE,
        };
        let channel = &self.basic.session.channel;
        let (reply, mut tx) = channel
            .open_stream(request, &self.token.auth, bytes.is_empty(), &guard)
            .await?;
        while !bytes.is_empty() {
            self.live()?;
            tx.reserve_capacity(bytes.len());
            let grant = if tx.capacity() == 0 {
                timeout(IDLE, poll_fn(|cx| tx.poll_capacity(cx)))
                    .await?
                    .ok_or_else(|| invalid("upload stream ended before the bounded body"))??
            } else {
                tx.capacity()
            };
            if grant == 0 {
                continue;
            }
            let part = bytes.split_to(grant.min(bytes.len()));
            // Captured chunks are immutable; recheck custody before the last
            // bytes and END_STREAM, not just before waiting for H2 readiness.
            self.live()?;
            if bytes.is_empty() {
                guard()?;
            }
            tx.send_data(part, bytes.is_empty())?;
        }
        // A full-file server verification can exceed a normal idle interval.
        // Its only outer budget is this exact login, not a renewed deadline.
        let deadline = if verifying_full_file {
            self.token.deadline
        } else {
            self.token.deadline.min(Instant::now() + ordinary_wait)
        };
        let response = timeout_at(deadline.into(), reply).await??;
        guard()?;
        if response.status() == StatusCode::UNAUTHORIZED {
            self.token.rejected.store(true, Ordering::Relaxed);
            return Err(invalid(
                "upload authorization expired or refused; explicitly reconnect, no fallback",
            ));
        }
        if response.status() == StatusCode::FORBIDDEN {
            return Err(invalid(
                "upload is outside the current password epoch; retain the original",
            ));
        }
        let h = response.headers();
        if single(h, "cache-control").map(HeaderValue::as_bytes) != Some(&b"no-store"[..])
            || [
                "content-encoding",
                "transfer-encoding",
                "content-range",
                "trailer",
                "www-authenticate",
            ]
            .iter()
            .any(|name| h.contains_key(*name))
        {
            return Err(invalid(
                "upload response has unverified framing or cache policy",
            ));
        }
        Ok(response)
    }

    pub(crate) async fn receipt(
        &self,
        i: &Intent,
        guard: impl Fn() -> Result<()>,
    ) -> Result<Lookup> {
        let reply = self
            .send(
                Frame {
                    request: i.lookup()?,
                    bytes: Bytes::new(),
                },
                &guard,
                false,
            )
            .await?;
        let status = reply.status();
        let result = match status {
            StatusCode::OK | StatusCode::NOT_FOUND => {
                let bytes = json(reply).await?;
                r3::outcome(i, status, &bytes, true)
            }
            StatusCode::CONFLICT => {
                require_empty(reply).await?;
                Lookup::Conflict
            }
            _ => Lookup::Unknown,
        };
        guard()?;
        self.live()?;
        Ok(result)
    }

    async fn progress(
        &self,
        req: Frame,
        i: &Intent,
        u: Option<&UploadId>,
        may_be_missing: bool,
        guard: impl Fn() -> Result<()>,
    ) -> Result<Option<Progress>> {
        let reply = self.send(req, &guard, false).await?;
        let got = match reply.status() {
            StatusCode::OK => Some(Progress::decode(&json(reply).await?, i, u)?),
            StatusCode::NOT_FOUND if may_be_missing => {
                require_missing_proof(reply.headers())?;
                require_empty(reply).await?;
                None
            }
            other => {
                return Err(
                    format!("upload returned HTTP {other}; retain the same original").into(),
                );
            }
        };
        guard()?;
        self.live()?;
        Ok(got)
    }

    pub(crate) async fn status(
        &self,
        i: &Intent,
        u: Option<&UploadId>,
        guard: impl Fn() -> Result<()>,
    ) -> Result<Option<Progress>> {
        self.progress(wire::status(i, u)?, i, u, u.is_none(), guard)
            .await
    }

    pub(crate) async fn begin(
        &self,
        i: &Intent,
        guard: impl Fn() -> Result<()>,
    ) -> Result<Progress> {
        self.progress(wire::begin(i)?, i, None, false, guard)
            .await?
            .ok_or_else(|| invalid("start returned no upload"))
    }

    pub(crate) async fn put(
        &self,
        i: &Intent,
        p: &Progress,
        data: Bytes,
        guard: impl Fn() -> Result<()>,
    ) -> Result<Progress> {
        let end = p.next_offset + data.len() as u64;
        let req = wire::put(i, &p.upload_id, p.next_offset, data)?;
        let next = self
            .progress(req, i, Some(&p.upload_id), false, guard)
            .await?
            .ok_or_else(|| invalid("chunk returned no upload"))?;
        if next.next_offset != end || next.state == Stage::Committed {
            return Err(invalid(
                "chunk did not confirm exactly the next original boundary",
            ));
        }
        Ok(next)
    }

    pub(crate) async fn commit(
        &self,
        i: &Intent,
        u: &UploadId,
        guard: impl Fn() -> Result<()>,
    ) -> Result<Lookup> {
        let reply = self.send(wire::commit(i, u)?, &guard, true).await?;
        // A 409 on this route can denote a retired registration, not a
        // permanent operation conflict. Only the exact R3 receipt can settle.
        let found = if reply.status() == StatusCode::OK {
            r3::outcome(i, StatusCode::OK, &json(reply).await?, false)
        } else {
            return Err(format!(
                "publish returned HTTP {}; check the original receipt",
                reply.status()
            )
            .into());
        };
        guard()?;
        self.live()?;
        Ok(found)
    }
}

fn require_missing_proof(h: &HeaderMap) -> Result<()> {
    if single(h, "zxs-upload-status").map(HeaderValue::as_bytes) != Some(&b"not-found"[..]) {
        return Err(invalid(
            "no authenticated upload-absence proof; route unavailable or unconfirmed",
        ));
    }
    Ok(())
}

fn canonical_length(h: &HeaderMap) -> Result<u64> {
    let value = single(h, "content-length")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| invalid("missing exact upload response length"))?;
    let n = value.parse::<u64>()?;
    if n.to_string() != value {
        return Err(invalid("noncanonical upload response length"));
    }
    Ok(n)
}

async fn json(reply: Response<RecvStream>) -> Result<Vec<u8>> {
    canonical_length(reply.headers())?;
    timeout(IDLE, collect_bounded_json(reply, JSON_LIMIT)).await?
}

async fn require_empty(reply: Response<RecvStream>) -> Result<()> {
    if canonical_length(reply.headers())? != 0 || reply.headers().contains_key("content-type") {
        return Err(invalid("not an exact empty upload response"));
    }
    timeout(IDLE, async {
        let mut body = reply.into_body();
        while let Some(data) = body.data().await {
            if !data?.is_empty() {
                return Err(invalid("unexpected upload error body"));
            }
        }
        if has_trailers(&mut body).await? {
            return Err(invalid("unexpected upload error trailers"));
        }
        Ok(())
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::{HeaderMap, HeaderValue, require_missing_proof};

    #[test]
    fn unqualified_not_found_is_never_permission_to_start() {
        let mut fields = HeaderMap::new();
        assert!(require_missing_proof(&fields).is_err());
        fields.insert("zxs-upload-status", HeaderValue::from_static("not-found"));
        assert!(require_missing_proof(&fields).is_ok());
        fields.append("zxs-upload-status", HeaderValue::from_static("not-found"));
        assert!(require_missing_proof(&fields).is_err());
        fields.insert("zxs-upload-status", HeaderValue::from_static("absent"));
        assert!(require_missing_proof(&fields).is_err());
    }
}
