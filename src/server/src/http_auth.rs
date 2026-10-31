//! HTTP authentication admission and the administrator session endpoints.
//!
//! Credential algorithms come directly from `authentication`;
//! persistent administrator/session state remains in `auth::AuthStore`.
use crate::{AdmittedServerConfig, ApiError, AppState, ServerConfig, auth::Identity, lock};
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
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const ATTEMPT_WINDOW: Duration = Duration::from_secs(60);
const MAX_ATTEMPT_KEYS: usize = 4096;
const MAX_ATTEMPTS: u32 = 10;
const MAX_PENDING_UNKNOWN_LOGINS: usize = 64;

enum LoginAccount {
    Administrator,
    Unknown(String),
}

impl LoginAccount {
    fn from_challenge(challenge: &crate::auth::LoginChallenge) -> Self {
        if challenge.is_known_account() {
            Self::Administrator
        } else {
            Self::Unknown(
                challenge
                    .canonical_username()
                    .expect("invariant: HTTP challenges contain a canonical username")
                    .to_owned(),
            )
        }
    }
}

/// Source windows move between partitions without resetting their count or age.
#[derive(Default)]
struct LoginSources {
    administrator: HashMap<IpAddr, Attempts>,
    unknown: HashMap<IpAddr, Attempts>,
}

impl LoginSources {
    fn charge(&mut self, ip: IpAddr, account: &LoginAccount, now: Instant) -> Result<(), ApiError> {
        let source = self
            .administrator
            .get(&ip)
            .or_else(|| self.unknown.get(&ip));
        if source.is_some_and(|attempt| attempt.count >= MAX_ATTEMPTS) {
            return Err(ApiError::limit(
                "too many login attempts; retry after 60 seconds",
            ));
        }
        let reserved =
            matches!(account, LoginAccount::Administrator) || self.administrator.contains_key(&ip);
        let destination = if reserved {
            &self.administrator
        } else {
            &self.unknown
        };
        if !destination.contains_key(&ip) && destination.len() == MAX_ATTEMPT_KEYS {
            return Err(ApiError::limit("login source admission capacity reached"));
        }
        let attempt = if reserved {
            // A known-name request promotes its source only after every check.
            // Later unknown-name requests share this exact window too.
            self.administrator.entry(ip).or_insert_with(|| {
                self.unknown.remove(&ip).unwrap_or(Attempts {
                    since: now,
                    count: 0,
                })
            })
        } else {
            self.unknown.entry(ip).or_insert(Attempts {
                since: now,
                count: 0,
            })
        };
        attempt.count += 1;
        Ok(())
    }
}

#[derive(Default)]
struct LoginAdmissionState {
    sources: LoginSources,
    administrator: Option<Attempts>,
    unknown: HashMap<String, Attempts>,
    pending_administrator: Arc<AtomicBool>,
    pending_unknown: HashMap<String, Arc<AtomicBool>>,
}

impl LoginAdmissionState {
    fn admit(
        &mut self,
        ip: IpAddr,
        account: &LoginAccount,
        now: Instant,
    ) -> Result<(PendingLogin, Duration), ApiError> {
        // Busy requests do not scan the retained counters or charge a source.
        let busy = match account {
            LoginAccount::Administrator => self.pending_administrator.load(Ordering::Acquire),
            LoginAccount::Unknown(username) => {
                // Cancellation releases only the owned slot. Reclaim names at
                // the next admission; active work is never evicted.
                self.pending_unknown
                    .retain(|_, active| active.load(Ordering::Acquire));
                self.pending_unknown.len() == MAX_PENDING_UNKNOWN_LOGINS
                    || self.pending_unknown.contains_key(username)
            }
        };
        if busy {
            return Err(ApiError::limit("login verification is busy"));
        }
        self.sources
            .administrator
            .retain(|_, attempt| now.duration_since(attempt.since) < ATTEMPT_WINDOW);
        self.sources
            .unknown
            .retain(|_, attempt| now.duration_since(attempt.since) < ATTEMPT_WINDOW);
        self.unknown
            .retain(|_, attempt| now.duration_since(attempt.since) < ATTEMPT_WINDOW);
        if self
            .administrator
            .as_ref()
            .is_some_and(|attempt| now.duration_since(attempt.since) >= ATTEMPT_WINDOW)
        {
            self.administrator = None;
        }
        if let LoginAccount::Unknown(username) = account
            && !self.unknown.contains_key(username)
            && self.unknown.len() == MAX_ATTEMPT_KEYS
        {
            return Err(ApiError::limit("unknown login admission capacity reached"));
        }
        // Account/pending and source capacity are checked before charging either.
        // Failed admission cannot amplify another account's pressure.
        self.sources.charge(ip, account, now)?;
        let attempt = match account {
            LoginAccount::Administrator => self.administrator.get_or_insert(Attempts {
                since: now,
                count: 0,
            }),
            LoginAccount::Unknown(username) => {
                self.unknown.entry(username.clone()).or_insert(Attempts {
                    since: now,
                    count: 0,
                })
            }
        };
        // Distributed account pressure cools only after an idle minute.
        attempt.since = now;
        attempt.count = attempt.count.saturating_add(1);
        let excess = attempt.count.saturating_sub(MAX_ATTEMPTS);
        let delay = if excess == 0 {
            Duration::ZERO
        } else {
            Duration::from_millis((250 * (1_u64 << (excess - 1).min(5))).min(6000))
        };
        // Claims are serialized by the admission mutex. The atomic release is
        // owned by the actual sleeper/worker, which need not hold that mutex.
        let active = match account {
            LoginAccount::Administrator => {
                self.pending_administrator.store(true, Ordering::Relaxed);
                Arc::clone(&self.pending_administrator)
            }
            LoginAccount::Unknown(username) => {
                let active = Arc::new(AtomicBool::new(true));
                self.pending_unknown
                    .insert(username.clone(), Arc::clone(&active));
                active
            }
        };
        Ok((PendingLogin { active }, delay))
    }
}

/// Arbitrary names/sources cannot consume the singleton administrator's key space.
/// The hash semaphore remains a separate bound shared by actual password work.
#[derive(Default)]
pub(crate) struct LoginAdmission {
    state: Mutex<LoginAdmissionState>,
}

impl LoginAdmission {
    fn admit(
        &self,
        ip: IpAddr,
        account: LoginAccount,
    ) -> Result<(PendingLogin, Duration), ApiError> {
        lock(&self.state).admit(ip, &account, Instant::now())
    }
}

/// The owner spans backoff and real hashing, including detached blocking work.
struct PendingLogin {
    active: Arc<AtomicBool>,
}

impl Drop for PendingLogin {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
    }
}

pub(crate) struct Attempts {
    since: Instant,
    count: u32,
}

/// Charge all keys atomically. Rejected requests never poison another budget.
fn admit_attempts<const N: usize>(
    attempts: &Mutex<HashMap<String, Attempts>>,
    keys: [String; N],
    capacity_message: &str,
    attempt_message: &str,
) -> Result<(), ApiError> {
    let mut attempts = lock(attempts);
    attempts.retain(|_, attempt| attempt.since.elapsed() < ATTEMPT_WINDOW);
    let additional = keys
        .iter()
        .filter(|key| !attempts.contains_key(*key))
        .count();
    if attempts.len().saturating_add(additional) > MAX_ATTEMPT_KEYS {
        return Err(ApiError::limit(capacity_message));
    }
    if keys.iter().any(|key| {
        attempts
            .get(key)
            .is_some_and(|attempt| attempt.count >= MAX_ATTEMPTS)
    }) {
        return Err(ApiError::limit(attempt_message));
    }
    for key in keys {
        let attempt = attempts.entry(key).or_insert(Attempts {
            since: Instant::now(),
            count: 0,
        });
        attempt.count += 1;
    }
    Ok(())
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
    config: &AdmittedServerConfig,
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
    let direct_origin;
    let expected = match &config.external_origin {
        Some(origin) => origin.0.as_str(),
        None => {
            let host = headers
                .get(header::HOST)
                .and_then(|host| host.to_str().ok())
                .ok_or_else(ApiError::forbidden)?;
            let parsed = url_encoding::address::Url::parse(&format!("http://{host}"))
                .map_err(|source| ApiError::forbidden().with_source(source))?;
            if headers.get_all(header::HOST).iter().count() != 1
                || parsed.host_str().is_none()
                || !parsed.username().is_empty()
                || parsed.password().is_some()
                || parsed.path() != "/"
                || parsed.query().is_some()
                || parsed.fragment().is_some()
                || host.contains('/')
                || host.contains('\\')
            {
                return Err(ApiError::forbidden());
            }
            direct_origin = parsed.origin().ascii_serialization();
            direct_origin.as_str()
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
    let identity = state.auth_reader.authenticate(&token)?;
    request.extensions_mut().insert(identity);
    request.extensions_mut().insert(SessionToken(token));
    Ok(next.run(request).await)
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

/// Retry can perform management I/O; keep its bounded job off the runtime.
pub(crate) async fn storage_retry_job(
    backend: Arc<dyn crate::Backend>,
    auth: Arc<Mutex<crate::auth::AuthStore>>,
    slots: Arc<tokio::sync::Semaphore>,
    identity: Identity,
) -> Result<crate::StorageStatus, ApiError> {
    let permit = slots
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("management capacity reached").with_source(source))?;
    crate::http_job::HttpBackendJob::new(backend, auth, permit)
        .run(move |backend| backend.retry_storage(&identity))
        .await
}

#[derive(Clone)]
pub(crate) struct SessionToken(String);

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct Login {
    username: String,
    password: String,
}

/// The blocking worker owns both admission resources through final publication.
async fn login_job(
    auth: Arc<Mutex<crate::auth::AuthStore>>,
    slots: Arc<tokio::sync::Semaphore>,
    pending: PendingLogin,
    challenge: crate::auth::LoginChallenge,
    password: String,
) -> Result<crate::auth::LoginResult, ApiError> {
    let permit = slots
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("password verification is busy").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _pending = pending;
        let _permit = permit;
        let verified = challenge.verify(&password)?;
        lock(&auth).finish_login(verified)
    })
    .await
    .map_err(ApiError::join)?
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
    let challenge = state.auth_reader.login_challenge(&body.username)?;
    let account = LoginAccount::from_challenge(&challenge);
    let (pending, delay) = state.login_admission.admit(ip, account)?;
    tokio::time::sleep(delay).await;
    let result = login_job(
        state.auth,
        state.password_slots,
        pending,
        challenge,
        body.password,
    )
    .await?;
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
        &state.password_attempts,
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
