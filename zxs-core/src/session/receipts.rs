//! The opt-in Bearer capability has no catalogue, download, or mutation methods.
use super::{
    H2Channel, IDLE, Origin, Principal, approved_origin, basic_header, collect_bounded_json,
    decode_r3_reply, single, validate_account, validate_password,
};
use crate::{
    Result, invalid,
    r3::{Intent, Lookup},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use h2::RecvStream;
use http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode, header};
use serde::Deserialize;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::time::timeout;
use zeroize::Zeroizing;

const SESSIONS: &str = "/zxs/v1/sessions";
const SESSION_LIMIT: u64 = 512;
const SESSION_SECONDS: u64 = 900;

/// Bound in memory to one open private workspace and one verified HTTP/2 channel.
/// It cannot be passed to the legacy read/write Session APIs.
pub struct ReceiptSession {
    channel: H2Channel,
    token: Bearer,
    owner: Arc<()>,
}

impl ReceiptSession {
    pub(crate) async fn connect_bound(
        original: &Principal,
        owner: Arc<()>,
        spelling: &str,
        account: &str,
        prompt: impl FnOnce() -> Result<Zeroizing<String>>,
        still_bound: impl Fn() -> Result<()>,
    ) -> Result<Self> {
        let origin = bound_origin(original, spelling, account)?;
        still_bound()?;
        // Compare the original trust-set digest before TCP. Prompt only after
        // the same selected channel has passed TLS hostname/chain and ALPN h2.
        let channel = H2Channel::connect(origin, account, Some(original)).await?;
        still_bound()?;
        let basic = prompt_bound_basic(prompt, &still_bound, |password| {
            basic_header(account, password)
        })?;
        let began = Instant::now();
        let response = channel
            .request(login_request()?, &basic, None, &still_bound)
            .await?;
        drop(basic);
        let token = timeout(IDLE, accept_login(response, began)).await??;
        token.require_live(Instant::now())?;
        still_bound()?;
        Ok(Self {
            channel,
            token,
            owner,
        })
    }

    pub(crate) fn belongs_to(&self, owner: &Arc<()>, original: &Principal) -> bool {
        Arc::ptr_eq(&self.owner, owner) && &self.channel.principal == original
    }

    pub(crate) fn require_live(&self) -> Result<()> {
        self.token.require_live(Instant::now())
    }

    pub(crate) async fn lookup(
        &self,
        original: &Intent,
        before_get: impl FnOnce() -> Result<()>,
    ) -> Result<Lookup> {
        self.require_live()?;
        let mut refused = None;
        let reply = self
            .channel
            .request(original.lookup()?, &self.token.auth, None, || {
                if let Err(error) = before_get().and_then(|()| self.require_live()) {
                    refused = Some(error);
                    return Err(invalid(
                        "local receipt authorization refused before sending",
                    ));
                }
                Ok(())
            })
            .await;
        if let Some(error) = refused {
            return Err(error);
        }
        let Ok(reply) = reply else {
            return Ok(Lookup::Unknown);
        };
        if matches!(
            reply.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) {
            self.token.rejected.store(true, Ordering::Relaxed);
            if reply.status() == StatusCode::UNAUTHORIZED
                && single(reply.headers(), "www-authenticate").map(HeaderValue::as_bytes)
                    == Some(&b"Bearer"[..])
            {
                return Err(invalid(
                    "the receipt session was rejected or expired; reconnect explicitly, with no fallback",
                ));
            }
            return Err(invalid(
                "the receipt request did not receive its authorized scheme; no fallback",
            ));
        }
        // Same exact R3 receipt and exact absence validator, never a SaveNode.
        Ok(decode_r3_reply(original, reply, true).await)
    }
}

fn prompt_bound_basic(
    prompt: impl FnOnce() -> Result<Zeroizing<String>>,
    still_bound: impl FnOnce() -> Result<()>,
    construct: impl FnOnce(&str) -> Result<HeaderValue>,
) -> Result<HeaderValue> {
    let password = prompt()?;
    // The interactive prompt can block while the user revokes or swaps the
    // selected workspace. Do not even construct Basic after that change.
    still_bound()?;
    validate_password(&password)?;
    construct(&password)
}

fn bound_origin(original: &Principal, spelling: &str, account: &str) -> Result<Origin> {
    let origin = approved_origin(spelling)?;
    validate_account(account)?;
    if origin.2 != original.origin() || account != original.account() {
        return Err(invalid(
            "select the original workspace HTTPS authority and account before logging in",
        ));
    }
    Ok(origin)
}

fn login_request() -> Result<Request<()>> {
    Ok(Request::builder()
        .method(Method::POST)
        .uri(SESSIONS)
        .header(header::CONTENT_LENGTH, "0")
        .header(header::ACCEPT, "application/json")
        .header(header::ACCEPT_ENCODING, "identity")
        .body(())?)
}

fn check_login_header(status: StatusCode, headers: &HeaderMap) -> Result<()> {
    if status == StatusCode::OK {
        if single(headers, "cache-control").map(HeaderValue::as_bytes) != Some(&b"no-store"[..]) {
            return Err(invalid("session reply did not prove no-store"));
        }
        return Ok(());
    }
    if single(headers, "content-length").map(HeaderValue::as_bytes) != Some(&b"0"[..]) {
        return Err(invalid(
            "session login was refused without its exact framing",
        ));
    }
    match status {
        StatusCode::UNAUTHORIZED
            if single(headers, "www-authenticate")
                .and_then(|h| h.to_str().ok())
                .is_some_and(|s| s == "Basic" || s.starts_with("Basic ")) =>
        {
            Err(invalid("the selected server rejected its Basic login"))
        }
        StatusCode::TOO_MANY_REQUESTS => {
            let seconds = single(headers, "retry-after")
                .and_then(|h| h.to_str().ok())
                .and_then(|s| s.parse::<u8>().ok().filter(|n| n.to_string() == s))
                .filter(|n| (1..=60).contains(n))
                .ok_or_else(|| invalid("session login has no exact bounded Retry-After"))?;
            Err(
                format!("session login is limited; explicitly try again in {seconds} seconds")
                    .into(),
            )
        }
        StatusCode::SERVICE_UNAVAILABLE => {
            Err(invalid("the selected server session is unavailable"))
        }
        _ => Err(invalid(
            "the selected server did not admit the receipt session; no fallback",
        )),
    }
}

struct Bearer {
    auth: HeaderValue,
    deadline: Instant,
    rejected: AtomicBool,
}

impl Bearer {
    fn require_live(&self, now: Instant) -> Result<()> {
        if self.rejected.load(Ordering::Relaxed) || now >= self.deadline {
            return Err(invalid(
                "the receipt session is no longer active; reconnect explicitly, with no fallback",
            ));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginReply<'a> {
    access_token: &'a str,
    token_type: &'a str,
    expires_in: u64,
}

fn decode_token(body: &[u8], began: Instant) -> Result<Bearer> {
    let malformed = || invalid("invalid receipt session credential shape");
    let proof: LoginReply<'_> = serde_json::from_slice(body).map_err(|_| malformed())?;
    if proof.token_type != "Bearer"
        || proof.expires_in != SESSION_SECONDS
        || proof.access_token.len() != 43
    {
        return Err(malformed());
    }
    let bytes = Zeroizing::new(
        URL_SAFE_NO_PAD
            .decode(proof.access_token)
            .map_err(|_| malformed())?,
    );
    if bytes.len() != 32
        || bytes.iter().all(|&b| b == 0)
        || *Zeroizing::new(URL_SAFE_NO_PAD.encode(&*bytes)) != proof.access_token
    {
        return Err(malformed());
    }
    let mut field = Zeroizing::new(String::from("Bearer "));
    field.push_str(proof.access_token);
    let mut auth = HeaderValue::from_str(&field).map_err(|_| malformed())?;
    auth.set_sensitive(true);
    let deadline = began
        .checked_add(Duration::from_secs(proof.expires_in))
        .ok_or_else(malformed)?;
    Ok(Bearer {
        auth,
        deadline,
        rejected: AtomicBool::new(false),
    })
}

async fn accept_login(reply: Response<RecvStream>, began: Instant) -> Result<Bearer> {
    check_login_header(reply.status(), reply.headers())?;
    let body = Zeroizing::new(collect_bounded_json(reply, SESSION_LIMIT).await?);
    decode_token(&body, began)
}

#[cfg(test)]
#[path = "../../tests/unit/receipt_session_cases.rs"]
mod tests;
