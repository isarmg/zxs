use js_sys::futures::JsFuture;
use js_sys::{Reflect, Uint8Array};
use json_codec::{Value, json};
use std::cell::RefCell;
use wasm_bindgen::{JsCast, JsValue};
use web_interface::i18n::t;
use web_sys::{RequestCredentials, RequestInit, RequestMode, RequestRedirect, Response};

mod response;

// Token and CSRF belong to one credential generation. A response from a
// previous login may not clear or replace the current login's credentials.
// Deliberately no Debug: this state contains authentication secrets.
#[derive(Clone, Default)]
struct Credentials {
    token: Option<String>,
    csrf: Option<String>,
    generation: u64,
}
impl Credentials {
    fn replace(&mut self, token: Option<String>, csrf: Option<String>) {
        self.generation += 1;
        self.token = token;
        self.csrf = csrf;
    }
    fn replace_if_current(
        &mut self,
        generation: u64,
        token: Option<String>,
        csrf: Option<String>,
    ) -> bool {
        if self.generation != generation {
            return false;
        }
        self.replace(token, csrf);
        true
    }
    fn revoke(&mut self, generation: u64) -> bool {
        if self.generation != generation {
            return false;
        }
        self.replace(None, None);
        true
    }
}
thread_local! { static CREDENTIALS: RefCell<Credentials> = RefCell::new(Credentials::default()); }

#[derive(Clone, Debug)]
pub struct Error {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub status: u16,
    pub request_id: Option<String>,
}

impl Error {
    /// After a mutation request, only a client-error response proves rejection.
    /// A malformed success response can follow a completed publication.
    pub(crate) fn mutation_outcome_unknown(&self) -> bool {
        storage_contract::client_protocol::mutation_outcome_unknown(self.status)
    }

    fn with_status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} · {}{}",
            self.code,
            match self.code.as_str() {
                "DIRECTORY_SCOPE_REQUIRED" => t(
                    "请选择具体文件夹；此硬盘根目录包含多个已有目录。",
                    "Select a specific folder; this disk root contains multiple existing namespaces."
                ),
                "DIRECTORY_BRANCH_INCOMPLETE" => t(
                    "所选目录分支的引用不完整，未发布此次切换。",
                    "The selected folder branch has incomplete references. The switch was not published."
                ),
                _ => self.message.clone(),
            },
            if self.retryable {
                t(
                    "（可重试；提交操作请先查询原操作状态）",
                    " (retryable; check the original save status before retrying)",
                )
            } else {
                String::new()
            }
        )
    }
}

fn error(code: &str, message: impl Into<String>) -> Error {
    Error {
        code: code.into(),
        message: message.into(),
        retryable: false,
        status: 0,
        request_id: None,
    }
}

pub fn session_changed() -> Error {
    error(
        "SESSION_CHANGED",
        t(
            "会话已改变，操作已停止。",
            "The session changed, so the operation was stopped.",
        ),
    )
}

pub fn clear_session() {
    CREDENTIALS.with(|saved| saved.borrow_mut().replace(None, None));
}

// Stop later stages of in-flight operations before logout reaches the server.
// Keep the credential until revocation is confirmed so failed logout can retry.
pub(crate) fn invalidate_session_work() {
    CREDENTIALS.with(|saved| saved.borrow_mut().generation += 1);
}

pub fn operation_id() -> String {
    fresh_id("op")
}

fn fresh_id(prefix: &str) -> String {
    let mut bytes = [0u8; 16];
    if let Some(crypto) = web_sys::window().and_then(|w| w.crypto().ok())
        && crypto.get_random_values_with_u8_array(&mut bytes).is_ok()
    {
        return format!(
            "{prefix}_{}",
            bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
        );
    }
    // The server rejects empty operation IDs. Do not silently substitute weak IDs.
    String::new()
}

pub fn segment(value: &str) -> String {
    // Match encodeURIComponent for UTF-8 Rust strings without depending on a
    // browser runtime. Contract validation uses the same path encoding.
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
    encoded
}

fn validate_path(path: &str) -> Result<(), Error> {
    storage_contract::validate_api_path(path).map_err(|_| {
        error(
            "INVALID_API_PATH",
            t(
                "仅允许本服务的 /api/ 业务接口",
                "Only this service’s /api/ API is allowed",
            ),
        )
    })
}

fn js_error(value: JsValue) -> Error {
    let message = value.as_string().unwrap_or_else(|| {
        t(
            "网络中断或系统操作失败；保存结果可能需要查询",
            "The connection was interrupted. Check whether the save completed.",
        )
    });
    error("TRANSPORT_ERROR", message)
}

/// Cancellation is shared by the listing owner and the in-flight GET only.
#[derive(Clone)]
pub(crate) struct ReadCancellation(
    std::sync::Arc<send_wrapper::SendWrapper<web_sys::AbortController>>,
);
impl ReadCancellation {
    pub(crate) fn new() -> Result<Self, Error> {
        web_sys::AbortController::new()
            .map(|controller| {
                Self(std::sync::Arc::new(send_wrapper::SendWrapper::new(
                    controller,
                )))
            })
            .map_err(js_error)
    }
    pub(crate) fn abort(&self) {
        self.0.abort();
    }
}
struct AbortOnDrop(ReadCancellation);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct Fetched {
    response: Response,
    credential_generation: u64,
    revoke_on_unauthorized: bool,
    _timeout: gloo_timers::callback::Timeout,
    _abort: AbortOnDrop,
}

async fn fetch(method: &str, path: &str, body: Option<Value>) -> Result<Fetched, Error> {
    fetch_authorized(method, path, body, None, None).await
}

async fn fetch_authorized(
    method: &str,
    path: &str,
    body: Option<Value>,
    bootstrap: Option<(&str, Option<&str>)>,
    cancellation: Option<ReadCancellation>,
) -> Result<Fetched, Error> {
    validate_path(path)?;
    let cancellation = cancellation.map(Ok).unwrap_or_else(ReadCancellation::new)?;
    let abort = AbortOnDrop(cancellation.clone());
    let controller = &*cancellation.0;
    let options = RequestInit::new();
    options.set_method(method);
    options.set_mode(RequestMode::SameOrigin);
    options.set_credentials(RequestCredentials::SameOrigin);
    options.set_redirect(RequestRedirect::Error);
    options.set_signal(Some(&controller.signal()));
    if let Some(body) = body {
        options.set_body(&JsValue::from_str(&body.to_string()));
    }
    let request = web_sys::Request::new_with_str_and_init(path, &options).map_err(js_error)?;
    request
        .headers()
        .set("Accept", "application/json")
        .map_err(js_error)?;
    request
        .headers()
        .set("Content-Type", "application/json")
        .map_err(js_error)?;
    if let Some((key, csrf)) = bootstrap {
        request
            .headers()
            .set("X-ZXS-Bootstrap", key)
            .map_err(js_error)?;
        if let Some(csrf) = csrf {
            request
                .headers()
                .set("X-CSRF-Token", csrf)
                .map_err(js_error)?;
        }
    }
    let credentials = CREDENTIALS.with(|saved| saved.borrow().clone());
    if bootstrap.is_none()
        && let Some(token) = credentials.token.as_ref()
    {
        request
            .headers()
            .set("Authorization", &format!("Bearer {token}"))
            .map_err(js_error)?;
    }
    if bootstrap.is_none()
        && !matches!(method, "GET" | "HEAD")
        && let Some(token) = credentials.csrf.as_ref()
    {
        request
            .headers()
            .set("X-CSRF-Token", token)
            .map_err(js_error)?;
    }
    let timer = gloo_timers::callback::Timeout::new(120_000, move || cancellation.abort());
    let window = web_sys::window().ok_or_else(|| {
        error(
            "NO_WINDOW",
            t("浏览器窗口不可用", "The browser window is unavailable"),
        )
    })?;
    let result = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(js_error);
    Ok(Fetched {
        response: result?.dyn_into().map_err(js_error)?,
        credential_generation: credentials.generation,
        revoke_on_unauthorized: bootstrap.is_none(),
        _timeout: timer,
        _abort: abort,
    })
}

async fn read_body(response: &Response, limit: usize) -> Result<Vec<u8>, Error> {
    read_body_inner(response, limit)
        .await
        .map_err(|error| error.with_status(response.status()))
}

async fn read_body_inner(response: &Response, limit: usize) -> Result<Vec<u8>, Error> {
    let Some(body) = response.body() else {
        return Ok(Vec::new());
    };
    let reader = body
        .get_reader()
        .dyn_into::<web_sys::ReadableStreamDefaultReader>()
        .map_err(|value| js_error(value.into()))?;
    let mut bytes = Vec::new();
    loop {
        let part = JsFuture::from(reader.read()).await.map_err(js_error)?;
        if Reflect::get(&part, &"done".into())
            .map_err(js_error)?
            .as_bool()
            .unwrap_or(false)
        {
            break;
        }
        let chunk = Uint8Array::new(&Reflect::get(&part, &"value".into()).map_err(js_error)?);
        if bytes.len().saturating_add(chunk.length() as usize) > limit {
            let _ = JsFuture::from(reader.cancel()).await;
            return Err(error(
                "RESPONSE_TOO_LARGE",
                t(
                    "响应超过界面安全上限，请缩小查询范围",
                    "The response is too large. Narrow the query.",
                ),
            ));
        }
        bytes.extend_from_slice(&chunk.to_vec());
    }
    reader.release_lock();
    Ok(bytes)
}

async fn decode(fetched: Fetched) -> Result<Value, Error> {
    let status = fetched.response.status();
    if status == 401 && fetched.revoke_on_unauthorized {
        CREDENTIALS.with(|saved| saved.borrow_mut().revoke(fetched.credential_generation));
    }
    if fetched
        .response
        .headers()
        .get("Content-Length")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<u64>().ok())
        .is_some_and(|v| v > storage_contract::client_protocol::MAX_RESPONSE_BYTES as u64)
    {
        return Err(response::from_protocol(
            storage_contract::client_protocol::response_too_large(status),
        ));
    }
    let bytes = read_body(
        &fetched.response,
        storage_contract::client_protocol::MAX_RESPONSE_BYTES,
    )
    .await
    .map_err(|error| {
        if error.code == "RESPONSE_TOO_LARGE" {
            response::from_protocol(storage_contract::client_protocol::response_too_large(
                status,
            ))
        } else {
            error.with_status(status)
        }
    })?;
    response::decode(status, &bytes)
}

pub async fn request(method: &str, path: &str, body: Option<Value>) -> Result<Value, Error> {
    validate_path(path)?;
    decode(fetch(method, path, body).await?).await
}

pub(crate) async fn cancellable_get(
    path: &str,
    cancellation: ReadCancellation,
) -> Result<Value, Error> {
    cancellable_request("GET", path, None, cancellation).await
}

pub(crate) async fn cancellable_request(
    method: &str,
    path: &str,
    body: Option<Value>,
    cancellation: ReadCancellation,
) -> Result<Value, Error> {
    decode(fetch_authorized(method, path, body, None, Some(cancellation)).await?).await
}

/// A bootstrap secret is scoped to the explicit same-origin setup endpoints and never saved.
pub(crate) async fn setup_request(
    method: &str,
    path: &str,
    body: Option<Value>,
    key: &str,
    csrf: Option<&str>,
) -> Result<Value, Error> {
    if !matches!(
        (method, path),
        ("GET", "/api/setup/status")
            | ("POST", "/api/setup/initialize")
            | ("POST", "/api/setup/initialize-preset")
            | ("POST", "/api/setup/complete-initialization")
            | ("POST", "/api/setup/preset-preview")
    ) || key.len() != 64
        || !key.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(error(
            "INVALID_SETUP_REQUEST",
            t(
                "请输入服务器本地提供的 64 位授权密钥。",
                "Enter the 64-character authorization key provided locally on the server.",
            ),
        ));
    }
    decode(fetch_authorized(method, path, body, Some((key, csrf)), None).await?).await
}

/// Public session identity. Authentication secrets never enter the UI model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Authenticated {
    pub principal: String,
}

pub(crate) async fn login(username: &str, password: &str) -> Result<Authenticated, Error> {
    let generation = CREDENTIALS.with(|saved| saved.borrow().generation);
    let result = request(
        "POST",
        "/api/auth/login",
        Some(json!({"username":username,"password":password})),
    )
    .await?;
    accept_login(generation, result)
}

fn validate_session(value: &Value, login: bool) -> Result<Authenticated, Error> {
    let fields: &[&str] = if login {
        &["principal", "csrf_token", "token", "expires_at"]
    } else {
        &["principal", "csrf_token"]
    };
    let Some(object) = value.as_object() else {
        return Err(invalid_session());
    };
    if object.len() != fields.len()
        || !fields
            .iter()
            .all(|field| value[*field].as_str().is_some_and(|text| !text.is_empty()))
        || (login
            && value["expires_at"].as_str().is_none_or(|expires| {
                !storage_contract::parse_u64_decimal(expires).is_ok_and(|expires| expires > 0)
            }))
    {
        return Err(invalid_session());
    }
    Ok(Authenticated {
        principal: value["principal"]
            .as_str()
            .ok_or_else(invalid_session)?
            .to_owned(),
    })
}

fn invalid_session() -> Error {
    error(
        "INVALID_SESSION",
        t(
            "认证响应未确认完整的会话，请重试登录。",
            "The authentication response did not confirm a complete session. Try signing in again.",
        ),
    )
    .with_status(200)
}

fn accept_login(generation: u64, result: Value) -> Result<Authenticated, Error> {
    let authenticated = validate_session(&result, true)?;
    let token = result["token"]
        .as_str()
        .filter(|token| !token.is_empty())
        .ok_or_else(invalid_session)?
        .to_owned();
    let accepted = CREDENTIALS.with(|saved| {
        saved.borrow_mut().replace_if_current(
            generation,
            Some(token),
            result["csrf_token"].as_str().map(str::to_owned),
        )
    });
    if !accepted {
        return Err(session_changed());
    }
    Ok(authenticated)
}

pub(crate) async fn restore() -> Result<Authenticated, Error> {
    let generation = CREDENTIALS.with(|saved| saved.borrow().generation);
    let value = request("GET", "/api/auth/me", None).await?;
    accept_restore(generation, value)
}

fn accept_restore(generation: u64, value: Value) -> Result<Authenticated, Error> {
    let authenticated = validate_session(&value, false)?;
    let accepted = CREDENTIALS.with(|saved| {
        let mut saved = saved.borrow_mut();
        if saved.generation != generation {
            return false;
        }
        saved.csrf = value["csrf_token"].as_str().map(str::to_owned);
        true
    });
    if accepted {
        Ok(authenticated)
    } else {
        Err(session_changed())
    }
}

#[cfg(test)]
#[path = "../../../test/web/unit/api/tests.rs"]
mod tests;
