//! HTTP authentication admission and the administrator session endpoints.
//!
//! Credential algorithms come directly from `authentication`;
//! persistent administrator/session state remains in `auth::AuthStore`.
use crate::{ApiError, AppState, ServerConfig, auth::Identity, lock};
use data_serialization::Deserialize;
use http_routing::{
    Json,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use json_codec::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const ATTEMPT_WINDOW: Duration = Duration::from_secs(60);
const MAX_ATTEMPT_KEYS: usize = 4096;
const MAX_ATTEMPTS: u32 = 10;
const MAX_PENDING_LOGINS: usize = 64;

/// At most one verifier or sleeper per canonical account, without reserving global hash slots.
struct PendingLogin {
    pending: Arc<Mutex<HashSet<String>>>,
    account: String,
}

impl PendingLogin {
    fn acquire(pending: &Arc<Mutex<HashSet<String>>>, username: &str) -> Result<Self, ApiError> {
        let account = login_username_key(username)?;
        let mut active = lock(pending);
        if active.len() >= MAX_PENDING_LOGINS || active.contains(&account) {
            return Err(ApiError::limit("login verification is busy"));
        }
        active.insert(account.clone());
        Ok(Self {
            pending: pending.clone(),
            account,
        })
    }
}

impl Drop for PendingLogin {
    fn drop(&mut self) {
        {
            let mut active = self.pending.lock().expect("service state lock poisoned");
            active.remove(&self.account);
        }
    }
}

pub(crate) struct Attempts {
    since: Instant,
    count: u32,
}

fn login_username_key(username: &str) -> Result<String, ApiError> {
    // Rate limiting must identify the same account as password verification,
    // including the canonical policy's accepted outer ASCII spaces.
    // Validate before retaining any input in pending/counter maps. The shared
    // policy bounds raw candidate bytes before normalization can allocate.
    let canonical = authentication::normalize_administrator_username(username)
        .map_err(|source| ApiError::unauthorized().with_source(source))?;
    Ok(format!("user:{canonical}"))
}

/// Charge all keys atomically. Rejected requests never poison another account's budget.
fn charge_attempts<const N: usize>(
    attempts: &Mutex<HashMap<String, Attempts>>,
    keys: [(String, Option<u32>); N],
    capacity_message: &str,
    attempt_message: &str,
) -> Result<[u32; N], ApiError> {
    let mut attempts = lock(attempts);
    attempts.retain(|_, attempt| attempt.since.elapsed() < ATTEMPT_WINDOW);
    let additional = keys
        .iter()
        .filter(|(key, _)| !attempts.contains_key(key))
        .count();
    if attempts.len().saturating_add(additional) > MAX_ATTEMPT_KEYS {
        return Err(ApiError::limit(capacity_message));
    }
    if keys.iter().any(|(key, limit)| {
        limit.is_some_and(|limit| {
            attempts
                .get(key)
                .is_some_and(|attempt| attempt.count >= limit)
        })
    }) {
        return Err(ApiError::limit(attempt_message));
    }
    Ok(keys.map(|(key, limit)| {
        let attempt = attempts.entry(key).or_insert(Attempts {
            since: Instant::now(),
            count: 0,
        });
        attempt.count = attempt.count.saturating_add(1);
        if limit.is_none() {
            // Distributed account pressure cools down only after an idle minute,
            // rather than allowing a new burst at every fixed-window boundary.
            attempt.since = Instant::now();
        }
        attempt.count
    }))
}

fn admit_attempts<const N: usize>(
    attempts: &Mutex<HashMap<String, Attempts>>,
    keys: [String; N],
    capacity_message: &str,
    attempt_message: &str,
) -> Result<(), ApiError> {
    charge_attempts(
        attempts,
        keys.map(|key| (key, Some(MAX_ATTEMPTS))),
        capacity_message,
        attempt_message,
    )?;
    Ok(())
}

fn admit_login(
    attempts: &Mutex<HashMap<String, Attempts>>,
    ip: IpAddr,
    username: &str,
) -> Result<Duration, ApiError> {
    let account = login_username_key(username)?;
    let counts = charge_attempts(
        attempts,
        [
            (format!("ip:{ip}"), Some(MAX_ATTEMPTS)),
            (format!("source:{ip}:{account}"), Some(MAX_ATTEMPTS)),
            (account, None),
        ],
        "login admission capacity reached",
        "too many login attempts; retry after 60 seconds",
    )?;
    // Account-wide pressure slows distributed guesses but cannot hard-lock a victim.
    // A single bounded pending-account guard spans delay and verification.
    // Global password slots are reserved for hashing, never for this sleep.
    let excess = counts[2].saturating_sub(MAX_ATTEMPTS);
    Ok(if excess == 0 {
        Duration::ZERO
    } else {
        Duration::from_millis((250 * (1_u64 << (excess - 1).min(5))).min(6000))
    })
}

fn client_ip(request: &Request, config: &ServerConfig) -> Result<IpAddr, ApiError> {
    let peer = request
        .extensions()
        .get::<http_routing::extract::ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip());
    // Missing transport metadata must never authenticate a forwarding header.
    let Some(peer) = peer else {
        return if config.trusted_proxy_ips.is_empty() {
            Ok(IpAddr::from([127, 0, 0, 1]))
        } else {
            Err(ApiError::invalid(
                "trusted proxy requires transport peer metadata",
            ))
        };
    };
    if !config.trusted_proxy_ips.contains(&peer) {
        return Ok(peer);
    }
    let values = request.headers().get_all("x-forwarded-for");
    let mut values = values.iter();
    let value = values
        .next()
        .ok_or_else(|| ApiError::invalid("trusted proxy must set X-Forwarded-For"))?;
    if values.next().is_some() {
        return Err(ApiError::invalid(
            "trusted proxy must overwrite X-Forwarded-For with one IP",
        ));
    }
    let ip = value
        .to_str()
        .map_err(ApiError::invalid_source)?
        .parse::<IpAddr>()
        .map_err(|source| {
            ApiError::invalid("trusted proxy must overwrite X-Forwarded-For with one IP")
                .with_source(source)
        })?;
    // Unify IPv4 and IPv4-mapped IPv6 spellings so they cannot multiply a budget.
    Ok(match ip {
        IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(ip)),
        ip => ip,
    })
}

pub(crate) fn same_origin(
    headers: &HeaderMap,
    config: &ServerConfig,
    required: bool,
) -> Result<(), ApiError> {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return if required {
            Err(ApiError::forbidden())
        } else {
            Ok(())
        };
    };
    if headers.get_all(header::ORIGIN).iter().count() != 1 {
        return Err(ApiError::forbidden());
    }
    let origin = origin
        .to_str()
        .map_err(|source| ApiError::forbidden().with_source(source))?;
    let expected = match config.normalized_external_origin()? {
        Some(origin) => origin,
        None => {
            let host = headers
                .get(header::HOST)
                .and_then(|host| host.to_str().ok())
                .ok_or_else(ApiError::forbidden)?;
            let parsed = url_encoding::address::Url::parse(&format!("http://{host}"))
                .map_err(|source| ApiError::forbidden().with_source(source))?;
            if !matches!(
                parsed.host_str(),
                Some("localhost") | Some("127.0.0.1") | Some("[::1]") | Some("::1")
            ) {
                return Err(ApiError::forbidden());
            }
            format!("http://{host}")
        }
    };
    if origin != expected {
        return Err(ApiError::forbidden());
    }
    if let Some(site) = headers.get("sec-fetch-site")
        && site != "same-origin"
    {
        return Err(ApiError::forbidden());
    }
    Ok(())
}

fn request_token(headers: &HeaderMap) -> Result<(String, bool), ApiError> {
    if let Some(value) = headers.get(header::AUTHORIZATION) {
        if headers.get_all(header::AUTHORIZATION).iter().count() != 1 {
            return Err(ApiError::unauthorized());
        }
        let value = value
            .to_str()
            .map_err(|source| ApiError::unauthorized().with_source(source))?;
        let (scheme, token) = value.split_once(' ').ok_or_else(ApiError::unauthorized)?;
        // Only the scheme is case-insensitive. Never normalize credentials or
        // accept an empty token, extra fields, or a duplicate Authorization.
        if !scheme.eq_ignore_ascii_case("Bearer")
            || token.is_empty()
            || token.bytes().any(|byte| byte.is_ascii_whitespace())
        {
            return Err(ApiError::unauthorized());
        }
        return Ok((token.to_owned(), false));
    }
    if headers.get_all(header::COOKIE).iter().count() != 1 {
        return Err(ApiError::unauthorized());
    }
    let cookie = headers
        .get(header::COOKIE)
        .and_then(|cookie| cookie.to_str().ok())
        .ok_or_else(ApiError::unauthorized)?;
    Ok((
        authentication::parse_cookie_value(cookie, "zxs_session")
            .ok_or_else(ApiError::unauthorized)?
            .to_owned(),
        true,
    ))
}

pub(crate) async fn authorize(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let (token, cookie) = request_token(request.headers())?;
    let changing = !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    same_origin(request.headers(), &state.config, cookie && changing)?;
    if cookie && changing {
        let csrf = request
            .headers()
            .get("x-csrf-token")
            .and_then(|value| value.to_str().ok())
            .ok_or_else(ApiError::forbidden)?;
        if request.headers().get_all("x-csrf-token").iter().count() != 1
            || !authentication::token_matches_hash(
                csrf,
                &authentication::token_hash(
                    &authentication::derive_csrf_token(&token)
                        .map_err(|source| ApiError::forbidden().with_source(source))?,
                ),
            )
        {
            return Err(ApiError::forbidden());
        }
    }
    let identity = authenticate(state.auth, state.authorization_slots, token.clone()).await?;
    request.extensions_mut().insert(identity);
    request.extensions_mut().insert(SessionToken(token));
    Ok(next.run(request).await)
}
// Bound jobs before scheduling, including time waiting for the credential lock.
// Ownership moves into the job so canceled HTTP futures cannot free its slot.
async fn authenticate(
    auth: Arc<Mutex<crate::auth::AuthStore>>,
    slots: Arc<tokio::sync::Semaphore>,
    credential: String,
) -> Result<Identity, ApiError> {
    if !authentication::is_token_shape(&credential) {
        return Err(ApiError::unauthorized());
    }
    auth_job(slots, move || lock(&auth).authenticate(&credential)).await
}

async fn auth_job<T: Send + 'static>(
    slots: Arc<tokio::sync::Semaphore>,
    job: impl FnOnce() -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    let permit = slots
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("authentication capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        job()
    })
    .await
    .map_err(ApiError::join)?
}

/// Retry revalidates the current account through AuthorizedBackend. That can
/// wait for an auth mutation/fsync, so the entire call belongs off the runtime.
pub(crate) async fn storage_retry_job(
    backend: Arc<dyn crate::Backend>,
    slots: Arc<tokio::sync::Semaphore>,
    identity: Identity,
) -> Result<Value, ApiError> {
    let permit = slots
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("management capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        // A disconnected caller does not free admission while the job is queued
        // or blocked on the authority lock.
        let _permit = permit;
        backend.retry_storage(&identity)
    })
    .await
    .map_err(ApiError::join)?
}

#[derive(Clone)]
pub(crate) struct SessionToken(String);

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct Login {
    username: String,
    password: String,
}
pub(crate) async fn login(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, ApiError> {
    same_origin(request.headers(), &state.config, false)?;
    let ip = client_ip(&request, &state.config)?;
    // Legal JSON Unicode escapes can use six bytes per decoded password byte.
    let bytes = crate::http_boundary::read_body(request, 8 * 1024).await?;
    let body: Login = storage_contract::decode_strict(&bytes).map_err(ApiError::invalid_source)?;
    let pending = PendingLogin::acquire(&state.pending_logins, &body.username)?;
    let delay = admit_login(&state.attempts, ip, &body.username)?;
    tokio::time::sleep(delay).await;
    let permit = state
        .password_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("password verification is busy").with_source(source))?;
    let auth = state.auth.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _pending = pending;
        let _permit = permit;
        let challenge = lock(&auth).login_challenge(&body.username)?;
        let verified = challenge.verify(&body.password)?;
        lock(&auth).finish_login(verified)
    })
    .await
    .map_err(ApiError::join)??;
    let cookie = format!(
        "zxs_session={}; Path=/api; HttpOnly; SameSite=Strict; Max-Age={}{}",
        result.token,
        crate::auth::SESSION_TTL_SECONDS,
        if state.config.secure_cookie() {
            "; Secure"
        } else {
            ""
        }
    );
    let mut response = Json(result).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie)
            .expect("invariant: generated session cookie is a valid header"),
    );
    Ok(response)
}

pub(crate) async fn me(
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    http_routing::Extension(token): http_routing::Extension<SessionToken>,
) -> Result<Json<Value>, ApiError> {
    let csrf_token =
        authentication::derive_csrf_token(&token.0).map_err(ApiError::invalid_source)?;
    Ok(Json(
        json!({"principal":identity.principal,"csrf_token":csrf_token}),
    ))
}
pub(crate) async fn logout(
    State(state): State<AppState>,
    http_routing::Extension(token): http_routing::Extension<SessionToken>,
) -> Result<Response, ApiError> {
    let auth = state.auth.clone();
    auth_job(state.authorization_slots, move || {
        lock(&auth).logout(&token.0)
    })
    .await?;
    let mut response = Json(json!({"logged_out":true})).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static("zxs_session=; Path=/api; HttpOnly; SameSite=Strict; Max-Age=0"),
    );
    Ok(response)
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct PasswordChange {
    current_password: String,
    new_password: String,
}

pub(crate) async fn change_password(
    State(state): State<AppState>,
    http_routing::Extension(identity): http_routing::Extension<Identity>,
    http_routing::Extension(token): http_routing::Extension<SessionToken>,
    request: Request,
) -> Result<Response, ApiError> {
    let ip = client_ip(&request, &state.config)?;
    let body: PasswordChange = storage_contract::decode_strict(
        &crate::http_boundary::read_body(request, 16 * 1024).await?,
    )
    .map_err(ApiError::invalid_source)?;
    admit_attempts(
        &state.attempts,
        [
            format!("password:ip:{ip}"),
            format!("password:user:{}", identity.principal),
        ],
        "password admission capacity reached",
        "too many password changes; retry after 60 seconds",
    )?;
    let permit = state
        .password_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("password verification is busy").with_source(source))?;
    let auth = state.auth.clone();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let challenge = lock(&auth).password_change_challenge(&token.0)?;
        let prepared = challenge.prepare(&body.current_password, &body.new_password)?;
        lock(&auth).finish_password_change(prepared)
    })
    .await
    .map_err(ApiError::join)??;
    let mut response =
        Json(json!({"changed":true,"reauthentication_required":true})).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static("zxs_session=; Path=/api; HttpOnly; SameSite=Strict; Max-Age=0"),
    );
    Ok(response)
}
#[cfg(test)]
#[path = "../../../test/server/unit/http_auth/tests.rs"]
mod tests;

/// Public bootstrap attempts can consume only their own source's rate budget.
/// Backend work still has the bounded management semaphore, but an invalid
/// capability must not exhaust a fixed-window quota shared with other sources.
#[cfg(feature = "core")]
pub(crate) fn setup_admission(state: &AppState, request: &Request) -> Result<(), ApiError> {
    let ip = client_ip(request, &state.config)?;
    admit_attempts(
        &state.setup_attempts,
        [format!("setup:ip:{ip}")],
        "setup admission capacity reached",
        "too many setup attempts; retry after 60 seconds",
    )
}

/// Called only after ordinary session/CSRF and administrator authorization.
/// A separate map isolates both counters and key capacity from public traffic.
#[cfg(feature = "core")]
pub(crate) fn recovery_admission(state: &AppState, request: &Request) -> Result<(), ApiError> {
    let ip = client_ip(request, &state.config)?;
    admit_attempts(
        &state.recovery_attempts,
        [format!("recovery:ip:{ip}"), "recovery:global".into()],
        "recovery admission capacity reached",
        "too many recovery attempts; retry after 60 seconds",
    )
}
