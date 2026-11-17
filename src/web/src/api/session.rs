//! Authentication response admission. Secrets never enter the public UI model.
use data_serialization::{Deserialize, Deserializer, de};
use json_codec::{Value, error::Category};
use std::num::NonZeroU64;
use storage_contract::Principal;
use zeroize::Zeroizing;

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct LoginResponse {
    pub principal: Principal,
    pub csrf_token: SessionSecret,
    pub token: SessionSecret,
    #[serialization(rename = "expires_at", deserialize_with = "positive_expiry")]
    _expires_at: NonZeroU64,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct SessionResponse {
    pub principal: Principal,
    pub csrf_token: SessionSecret,
}

// Neither Clone, Debug, Display nor Serialize: the account-generation owner
// moves the decoded buffer; replacing/revoking/dropping it erases that buffer.
// Browser network/JS-managed copies are outside the Rust owner's control.
pub(super) struct SessionSecret(Zeroizing<String>);
impl SessionSecret {
    pub(super) fn expose(&self) -> &str {
        self.0.as_str()
    }
    pub(super) fn bearer_header(&self) -> Zeroizing<String> {
        const PREFIX: &str = "Bearer ";
        let mut value = Zeroizing::new(String::with_capacity(PREFIX.len() + self.0.len()));
        value.push_str(PREFIX);
        value.push_str(self.expose());
        value
    }
}

impl<'de> Deserialize<'de> for SessionSecret {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Zeroizing::new(String::deserialize(deserializer)?);
        if value.is_empty() {
            return Err(de::Error::custom("session secret is empty"));
        }
        Ok(Self(value))
    }
}

fn positive_expiry<'de, D: Deserializer<'de>>(deserializer: D) -> Result<NonZeroU64, D::Error> {
    let value = String::deserialize(deserializer)?;
    let expiry = storage_contract::parse_u64_decimal(&value).map_err(de::Error::custom)?;
    NonZeroU64::new(expiry).ok_or_else(|| de::Error::custom("session expiry is zero"))
}

/// Retain only the decoder category: its raw message can contain credentials
/// or attacker-controlled field names, including through Debug and source().
#[derive(Debug, error_definition::Error)]
#[error("authentication response decoding failed ({category:?})")]
pub struct DecodeError {
    category: Category,
}

pub(super) fn decode<T: de::DeserializeOwned>(value: Value) -> Result<T, DecodeError> {
    let Value::Object(object) = value else {
        return Err(DecodeError {
            category: Category::Data,
        });
    };
    T::deserialize(object).map_err(|source| DecodeError {
        category: source.classify(),
    })
}

/// Header setters are allowed to report the rejected value in their exception.
/// Retain the header and standardized failure class, never the raw JS message,
/// stack, or original thrown value (which could itself be the credential).
#[derive(Clone, Copy, Debug)]
pub(super) enum Header {
    Authorization,
    Csrf,
}
#[derive(Clone, Copy, Debug)]
enum HeaderCause {
    TypeError,
    DomException(u16),
    Other,
}
#[derive(Debug)]
pub struct HeaderFailure {
    header: Header,
    cause: HeaderCause,
}
impl std::fmt::Display for HeaderFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self.header {
            Header::Authorization => "Authorization",
            Header::Csrf => "X-CSRF-Token",
        };
        match self.cause {
            HeaderCause::TypeError => write!(f, "browser rejected the {name} header (TypeError)"),
            HeaderCause::DomException(code) => {
                write!(f, "browser rejected the {name} header (DOM error {code})")
            }
            HeaderCause::Other => write!(f, "browser rejected the {name} header"),
        }
    }
}
impl std::error::Error for HeaderFailure {}
pub(super) fn header_error(header: Header, value: &wasm_bindgen::JsValue) -> super::Error {
    use wasm_bindgen::JsCast;
    let cause = if value.is_instance_of::<js_sys::TypeError>() {
        HeaderCause::TypeError
    } else if let Some(dom) = value.dyn_ref::<web_sys::DomException>() {
        HeaderCause::DomException(dom.code())
    } else {
        HeaderCause::Other
    };
    let mut error = super::error(
        storage_contract::ErrorCode::TransportError,
        web_interface::i18n::t(
            "浏览器无法使用当前会话凭据，此次请求尚未发送。",
            "The browser could not use this session credential. The request was not sent.",
        ),
    );
    error.source = Some(std::sync::Arc::new(super::ErrorSource::SessionHeader(
        HeaderFailure { header, cause },
    )));
    error
}

#[cfg(test)]
#[path = "../../../../test/web/unit/api/session_secrets.rs"]
mod secret_tests;
