//! Explicit new-profile reuse. A TCP connection never caches a client's identity.
use super::{Head, Outcome, Problem};
use crate::headless_auth::{Account, LoginError, SESSION_TTL_SECONDS};
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

const LOGIN: &str = "/zxs/v1/sessions";
pub(super) const MAX_EXCHANGES: usize = 32;
pub(super) const NEXT_HEAD: Duration = Duration::from_secs(5);
const MAX_REUSED_CONNECTIONS: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum BackendProfile {
    Legacy,
    Login,
    Bearer,
}

pub(super) fn profile(head: &Head<'_>) -> BackendProfile {
    if head.target == LOGIN {
        return BackendProfile::Login;
    }
    if head.fields.iter().any(|(key, value)| {
        key.eq_ignore_ascii_case("authorization")
            && value
                .split_ascii_whitespace()
                .next()
                .is_some_and(|scheme| scheme.eq_ignore_ascii_case("Bearer"))
    }) {
        return BackendProfile::Bearer;
    }
    BackendProfile::Legacy
}

pub(super) fn can_reuse(head: &Head<'_>) -> bool {
    // Backend-only control. Missing is the ordinary HTTP/1.1 default; an
    // explicit close, ambiguous control or unsupported option is not reused.
    let mut values = head
        .fields
        .iter()
        .filter_map(|(key, v)| key.eq_ignore_ascii_case("connection").then_some(*v));
    values
        .next()
        .is_none_or(|first| first.eq_ignore_ascii_case("keep-alive") && values.next().is_none())
}

fn login_frame(head: &Head<'_>) -> Result<(), Problem> {
    if head.method != "POST" {
        return Err(Problem::Missing);
    }
    let mut length = None;
    for (key, value) in &head.fields {
        if key.eq_ignore_ascii_case("content-length") && length.replace(*value).is_some() {
            return Err(Problem::Malformed);
        }
        if ["content-encoding", "content-range", "range"]
            .iter()
            .any(|name| key.eq_ignore_ascii_case(name))
        {
            return Err(Problem::Malformed);
        }
    }
    if length != Some("0") {
        return Err(Problem::Malformed);
    }
    Ok(())
}

pub(super) fn session_outcome(
    account: &Account,
    head: &Head<'_>,
    stopping: &dyn Fn() -> bool,
) -> Outcome {
    if let Err(problem) = login_frame(head) {
        return Outcome::Failed(problem);
    }
    match account.issue_session(&head.fields, stopping) {
        Ok(token) => Outcome::Json(
            format!(
                r#"{{"access_token":"{token}","token_type":"Bearer","expires_in":{SESSION_TTL_SECONDS}}}"#
            )
            .into_bytes(),
        ),
        Err(LoginError::Throttled(seconds)) => Outcome::Failed(Problem::LoginThrottled(seconds)),
        Err(LoginError::Auth(error)) => Outcome::Failed(error.into()),
    }
}

/// At least 24 of the listener's existing 32 worker positions cannot be
/// occupied merely by an idle reusable connection.
#[derive(Default)]
pub(crate) struct ReuseBudget(AtomicUsize);

pub(super) struct Slot<'a>(&'a ReuseBudget);

impl ReuseBudget {
    pub(super) fn try_hold(&self) -> Option<Slot<'_>> {
        self.0
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |held| {
                (held < MAX_REUSED_CONNECTIONS).then_some(held + 1)
            })
            .ok()?;
        Some(Slot(self))
    }
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        let budget = self.0;
        budget.0.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
#[path = "../../tests/unit/headless_reuse.rs"]
mod tests;
