//! The four explicit metadata GETs are a new Bearer permission, not a new
//! route, body decoder, or way to dispatch a legacy write.
use super::{Incoming, Outcome, Problem, Request};
use crate::headless_auth::{Account, AuthError, BearerPermit, Permit};
use std::time::Instant;

pub(super) enum AccessKind {
    ExistingRead,
    Metadata,
}

pub(super) fn classify(request: &Request, headers: &[(&str, &str)]) -> Result<AccessKind, Problem> {
    let control: &[&str] = match request {
        // The original receipt and immutable-payload profiles stay exact.
        Request::Receipt { .. } | Request::NodePayload(_) => return Ok(AccessKind::ExistingRead),
        Request::ListCompact { .. } => &["zxs-view", "zxs-after"],
        Request::ContainerDetail { .. } => {
            &["zxs-detail", "zxs-detail-sequence", "zxs-detail-after"]
        }
        Request::BranchHistory { .. } => &["zxs-history-head", "zxs-history-next"],
        Request::Tags(_) => &[],
        _ => return Err(Problem::Denied),
    };
    // The shared typed parser has already proved method, exact target,
    // uniqueness of controls and zero declared body. Reject cross-lane
    // application headers rather than giving them an ignored meaning.
    if headers.iter().any(|(name, _)| {
        name.eq_ignore_ascii_case("content-type")
            || (name
                .get(..4)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("zxs-"))
                && !control
                    .iter()
                    .any(|allowed| name.eq_ignore_ascii_case(allowed)))
    }) {
        return Err(Problem::Malformed);
    }
    Ok(AccessKind::Metadata)
}

pub(super) fn bearer_problem(error: AuthError) -> Problem {
    match error {
        AuthError::Invalid => Problem::BearerUnauthorized,
        AuthError::Unavailable => Problem::Unavailable,
    }
}

pub(super) struct Reply<'account> {
    pub(super) outcome: Outcome,
    evidence: Option<BearerPermit<'account>>,
}

impl From<Outcome> for Reply<'_> {
    fn from(outcome: Outcome) -> Self {
        Self {
            outcome,
            evidence: None,
        }
    }
}

impl<'account> Reply<'account> {
    pub(super) fn guarded(outcome: Outcome, evidence: BearerPermit<'account>) -> Self {
        Self {
            outcome,
            evidence: Some(evidence),
        }
    }

    pub(super) fn before_wire(
        &self,
        account: &Account,
        stopping: &dyn Fn() -> bool,
        now: Instant,
    ) -> Result<(), Problem> {
        self.evidence
            .as_ref()
            .map_or(Ok(()), |permit| permit.require(account, stopping, now))
            .map_err(bearer_problem)
    }
}

pub(super) fn project<'account>(
    account: &'account Account,
    permit: Permit<'account>,
    request: Request,
    stopping: &dyn Fn() -> bool,
    dispatch: &impl Fn(Request, &mut Incoming<'_>, &dyn Fn() -> bool) -> Outcome,
) -> Reply<'account> {
    let checked = (|| {
        let permit = permit.bearer_evidence()?;
        let require = || permit.require(account, stopping, Instant::now());
        require()?;
        let result = dispatch(request, &mut Incoming::Empty, &|| require().is_ok());
        // Check after the actual DTO projection, including any 404/409.
        require()?;
        let outcome = match result {
            Outcome::Json(_) | Outcome::Failed(_) => result,
            _ => Outcome::Failed(Problem::Unavailable), // This profile cannot stream.
        };
        Ok(Reply::guarded(outcome, permit))
    })();
    checked.unwrap_or_else(|error| Outcome::Failed(bearer_problem(error)).into())
}

#[cfg(test)]
#[path = "../../tests/unit/browse_wire.rs"]
mod tests;
