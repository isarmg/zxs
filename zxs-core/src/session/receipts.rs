//! The opt-in Bearer capability has no catalogue, download, or mutation methods.
use super::{
    H2Channel, IDLE, Origin, Principal, approved_origin, basic_header, collect_bounded_json,
    decode_r3_reply, single, validate_account, validate_password,
};
use crate::{
    Result, invalid,
    r3::{Id, Intent, Lookup, ROOT},
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

/// Receipt-only capability for one selected workspace and verified channel.
pub struct ReceiptSession {
    peer: BoundBearer,
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
        let peer = BoundBearer::connect_bound(
            ReadPurpose::Receipt,
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

    pub(crate) fn require_live(&self) -> Result<()> {
        self.peer.require_live()
    }

    pub(crate) async fn lookup(
        &self,
        original: &Intent,
        before_get: impl FnOnce() -> Result<()>,
    ) -> Result<Lookup> {
        let Some(reply) = self.peer.read(original.lookup()?, before_get).await? else {
            return Ok(Lookup::Unknown);
        };
        Ok(decode_r3_reply(original, reply, true).await)
    }
}

#[derive(Clone, Copy)]
pub(super) enum ReadPurpose {
    Receipt,
    Payload,
}

impl ReadPurpose {
    fn require_peer_identity(self, saved: &Principal, current: &Principal) -> Result<()> {
        let allowed = match self {
            // Receipt recovery uses current native trust on the same endpoint.
            // The live TLS check is separate and always required.
            Self::Receipt => {
                saved.origin() == current.origin() && saved.account() == current.account()
            }
            Self::Payload => saved == current,
        };
        if !allowed {
            return Err(invalid(
                "the original client authority or native trust changed",
            ));
        }
        Ok(())
    }

    fn name(self) -> &'static str {
        match self {
            Self::Receipt => "receipt",
            Self::Payload => "payload",
        }
    }

    pub(super) fn require_route(self, request: &Request<()>) -> Result<()> {
        let uri = request.uri();
        if request.method() != Method::GET
            || uri.authority().is_some()
            || uri.scheme().is_some()
            || uri.query().is_some()
            || request.headers().contains_key(header::CONTENT_LENGTH)
        {
            return Err(invalid(
                "Bearer capability only admits its bodyless on-origin GET",
            ));
        }
        let rest = uri
            .path()
            .strip_prefix(ROOT)
            .and_then(|p| p.strip_prefix('/'))
            .ok_or_else(|| invalid("not a typed original Bearer read route"))?;
        let mut parts = rest.split('/');
        let accepted = match (
            self,
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) {
            (Self::Receipt, Some(c), Some("operations"), Some(o), None, None) => {
                Id::parse(c).is_ok() && Id::parse(o).is_ok()
            }
            (Self::Payload, Some(c), Some("nodes"), Some(n), Some("payload"), None) => {
                Id::parse(c).is_ok() && Id::parse(n).is_ok()
            }
            _ => false,
        };
        if !accepted {
            return Err(invalid(
                "not an allowed route for this selected Bearer capability",
            ));
        }
        Ok(())
    }
}

// Scheme admission does not edit or weaken the exact prior local selection.
struct SelectedWorkspace {
    owner: Arc<()>,
    saved: Principal,
}

impl SelectedWorkspace {
    fn matches(&self, owner: &Arc<()>, original: &Principal) -> bool {
        Arc::ptr_eq(&self.owner, owner) && &self.saved == original
    }
}

pub(super) struct BoundBearer {
    channel: H2Channel,
    token: Bearer,
    original: SelectedWorkspace,
    purpose: ReadPurpose,
}

impl BoundBearer {
    pub(super) async fn connect_bound(
        purpose: ReadPurpose,
        original: &Principal,
        owner: Arc<()>,
        spelling: &str,
        account: &str,
        prompt: impl FnOnce() -> Result<Zeroizing<String>>,
        still_bound: impl Fn() -> Result<()>,
    ) -> Result<Self> {
        let origin = bound_origin(original, spelling, account)?;
        still_bound()?;
        // Preflight the selected authority, then obtain a same-channel native TLS
        // chain/hostname and ALPN h2 proof before prompting for any credential.
        let channel = H2Channel::connect(origin, account, |current| {
            purpose.require_peer_identity(original, current)
        })
        .await?;
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
            original: SelectedWorkspace {
                owner,
                saved: original.clone(),
            },
            purpose,
        })
    }

    pub(super) fn belongs_to(&self, owner: &Arc<()>, original: &Principal) -> bool {
        self.original.matches(owner, original)
            && self
                .purpose
                .require_peer_identity(original, &self.channel.principal)
                .is_ok()
    }

    fn require_live(&self) -> Result<()> {
        self.token.require_live(Instant::now())
    }

    // None is transport uncertainty, never evidence for either read purpose.
    pub(super) async fn read(
        &self,
        request: Request<()>,
        before_get: impl FnOnce() -> Result<()>,
    ) -> Result<Option<Response<RecvStream>>> {
        self.purpose.require_route(&request)?;
        self.require_live()?;
        let mut refused = None;
        let reply = self
            .channel
            .request(request, &self.token.auth, None, || {
                if let Err(error) = before_get().and_then(|()| self.require_live()) {
                    refused = Some(error);
                    return Err(invalid("local read authorization refused before sending"));
                }
                Ok(())
            })
            .await;
        if let Some(error) = refused {
            return Err(error);
        }
        let Ok(reply) = reply else {
            return Ok(None);
        };
        if matches!(
            reply.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) {
            self.token.rejected.store(true, Ordering::Relaxed);
            let name = self.purpose.name();
            let message = if reply.status() == StatusCode::UNAUTHORIZED
                && single(reply.headers(), "www-authenticate").map(HeaderValue::as_bytes)
                    == Some(&b"Bearer"[..])
            {
                format!(
                    "the {name} session was rejected or expired; reconnect explicitly, with no fallback"
                )
            } else {
                format!("the {name} request did not receive its authorized scheme; no fallback")
            };
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, message).into());
        }
        Ok(Some(reply))
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
