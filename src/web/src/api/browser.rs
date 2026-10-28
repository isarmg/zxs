//! Parse browser exceptions once, retaining the original JavaScript value.
use super::{Error, ErrorSource, error};
use std::cell::Cell;
use wasm_bindgen::{JsCast, JsValue};
use web_interface::i18n::t;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FailureKind {
    Browser,
    Request,
    ReadCancelled,
    Timeout,
}

#[derive(Debug)]
pub struct BrowserFailure {
    original: JsValue,
    name: Option<String>,
    message: Option<String>,
    kind: FailureKind,
}

impl BrowserFailure {
    fn parse(original: JsValue, kind: FailureKind) -> Self {
        let (name, message) = if let Some(error) = original.dyn_ref::<js_sys::Error>() {
            (Some(error.name().into()), Some(error.message().into()))
        } else if let Some(error) = original.dyn_ref::<web_sys::DomException>() {
            (Some(error.name()), Some(error.message()))
        } else {
            (None, original.as_string())
        };
        Self {
            original,
            name,
            message,
            kind,
        }
    }

    fn into_error(self) -> Error {
        let mut failure = error(
            storage_contract::ErrorCode::TransportError,
            self.to_string(),
        );
        failure.source = Some(std::sync::Arc::new(ErrorSource::Browser(self)));
        failure
    }
}

impl std::fmt::Display for BrowserFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let context = match self.kind {
            FailureKind::Browser => t("浏览器操作失败", "Browser operation failed"),
            // Fetch also rejects for browser policy failures, not only network loss.
            FailureKind::Request => t("浏览器请求失败", "Browser request failed"),
            FailureKind::ReadCancelled => t("读取已取消", "The read was cancelled"),
            FailureKind::Timeout => t("请求超时", "The request timed out"),
        };
        write!(f, "{context}")?;
        if let Some(name) = &self.name {
            write!(f, " · {name}")?;
        }
        if let Some(message) = &self.message {
            write!(f, ": {message}")?;
        } else if self.name.is_none() {
            write!(f, " · {:?}", self.original)?;
        }
        Ok(())
    }
}

impl std::error::Error for BrowserFailure {}

pub(super) fn js_error(value: JsValue) -> Error {
    BrowserFailure::parse(value, FailureKind::Browser).into_error()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AbortCause {
    ReadCancelled,
    Timeout,
}

struct Cancellation {
    controller: web_sys::AbortController,
    cause: Cell<Option<AbortCause>>,
}

/// Shared only by a read owner and its request. The first abort owns the cause.
#[derive(Clone)]
pub(crate) struct ReadCancellation(std::sync::Arc<send_wrapper::SendWrapper<Cancellation>>);

impl ReadCancellation {
    pub(crate) fn new() -> Result<Self, Error> {
        web_sys::AbortController::new()
            .map(|controller| {
                Self(std::sync::Arc::new(send_wrapper::SendWrapper::new(
                    Cancellation {
                        controller,
                        cause: Cell::new(None),
                    },
                )))
            })
            .map_err(js_error)
    }

    pub(crate) fn abort(&self) {
        self.abort_with(AbortCause::ReadCancelled);
    }

    pub(super) fn timeout(&self) {
        self.abort_with(AbortCause::Timeout);
    }

    fn abort_with(&self, cause: AbortCause) {
        if !self.signal().aborted() {
            self.0.cause.set(Some(cause));
            self.0.controller.abort();
        }
    }

    pub(super) fn signal(&self) -> web_sys::AbortSignal {
        self.0.controller.signal()
    }

    pub(super) fn request_error(&self, value: JsValue) -> Error {
        let signal = self.signal();
        // An AbortError name alone cannot prove owner cancellation or timeout.
        // A racing unrelated rejection must retain its actual browser cause.
        let kind = match (
            signal.aborted() && value == signal.reason(),
            self.0.cause.get(),
        ) {
            (true, Some(AbortCause::ReadCancelled)) => FailureKind::ReadCancelled,
            (true, Some(AbortCause::Timeout)) => FailureKind::Timeout,
            _ => FailureKind::Request,
        };
        BrowserFailure::parse(value, kind).into_error()
    }
}

pub(super) struct AbortOnDrop(pub(super) ReadCancellation);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        // Resource cleanup is not evidence that an owner cancelled a read.
        self.0.0.controller.abort();
    }
}

#[cfg(test)]
#[path = "../../../../test/web/unit/api/browser/tests.rs"]
mod tests;
