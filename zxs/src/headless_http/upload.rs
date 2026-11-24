//! Explicit Bearer upload requests, on the same bounded backend connection.
use super::{Head, Outcome, Problem, Reply, bearer_problem};
#[cfg(test)]
use super::{head, read_head, write_outcome};
#[cfg(test)]
use crate::{container_store::Store, fs::HeldDir};
use crate::{
    container_store::upload::{Authorize, Scope, Uploads, expired_upload_authority},
    headless_auth::{Account, AuthError, Permit},
    workspace_protocol::http::uploads,
};
use std::{
    io::{self, Read},
    time::{Duration, Instant},
};

#[cfg(test)]
use std::io::{BufRead, Write};

const RAW_INGRESS: Duration = Duration::from_secs(180);
const CONTROL_INGRESS: Duration = Duration::from_secs(60);
const STOP_POLL: Duration = Duration::from_millis(250);

pub(crate) struct Ingress<'a> {
    pub(crate) now: &'a dyn Fn() -> Instant,
    /// Set the underlying blocking socket's next read timeout. The source
    /// fixture supplies a fake socket; no whole-body time resets here.
    pub(crate) wait: &'a mut dyn FnMut(Duration) -> io::Result<()>,
    pub(crate) stopping: &'a dyn Fn() -> bool,
}

struct Timed<'a, 'c, R> {
    raw: &'a mut R,
    io: &'a mut Ingress<'c>,
    authorize: Authorize<'a>,
    limit: Duration,
    deadline: Option<Instant>,
}

impl<R: Read> Timed<'_, '_, R> {
    fn remaining(&self, deadline: Instant) -> io::Result<Duration> {
        (self.authorize)()?;
        let left = deadline.saturating_duration_since((self.io.now)());
        if left.is_zero() {
            return Err(uploads::failed_ingress(io::ErrorKind::TimedOut));
        }
        Ok(left)
    }
}

impl<R: Read> Read for Timed<'_, '_, R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        let deadline = if let Some(deadline) = self.deadline {
            deadline
        } else {
            let deadline = (self.io.now)()
                .checked_add(self.limit)
                .ok_or(io::ErrorKind::InvalidInput)?;
            self.deadline = Some(deadline);
            deadline
        };
        loop {
            let wait = self.remaining(deadline)?.min(STOP_POLL);
            (self.io.wait)(wait)?;
            match self.raw.read(out) {
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => return Err(uploads::failed_ingress(error.kind())),
                Ok(bytes) => {
                    self.remaining(deadline)?;
                    return Ok(bytes);
                }
            }
        }
    }
}

fn authority(error: AuthError) -> io::Error {
    match error {
        AuthError::Invalid => expired_upload_authority(),
        AuthError::Unavailable => io::ErrorKind::NotConnected.into(),
    }
}

pub(super) fn resource<'account, 'store>(
    bind: impl FnOnce(Scope) -> io::Result<Uploads<'store>>,
    account: &'account Account,
    request: &Head<'_>,
    reader: &mut impl Read,
    ingress: &mut Ingress<'_>,
) -> Result<Reply<'account>, Problem> {
    let stopping = ingress.stopping;
    let now = ingress.now;
    let permit = account
        .authenticate_bearer(&request.fields, stopping)
        .and_then(Permit::bearer_evidence)
        .map_err(bearer_problem)?;
    let require = || permit.require(account, stopping, now());
    require().map_err(bearer_problem)?;
    let scope = permit.upload_scope();
    let result = (|| {
        let frame = uploads::frame(request.method, request.target, &request.fields)?;
        let authorize = || require().map_err(authority);
        let timed = Timed {
            raw: reader,
            io: ingress,
            authorize: &authorize,
            limit: if frame.raw() {
                RAW_INGRESS
            } else {
                CONTROL_INGRESS
            },
            deadline: None,
        };
        let mut bounded = timed.take(frame.extent());
        let outcome = uploads::exchange(|| bind(scope), scope, frame, &mut bounded, &authorize)?;
        if bounded.limit() != 0 {
            return Err(Problem::Unavailable);
        }
        Ok(outcome)
    })();
    // The underlying selection may already be durable when cleanup finishes
    // after expiry. Withhold the reply; the original operation GET is final.
    require().map_err(bearer_problem)?;
    Ok(Reply::guarded(
        result.unwrap_or_else(Outcome::Failed),
        permit,
    ))
}

/// Drive the same adapter and its final before-wire proof on a fake clock.
#[cfg(test)]
pub(crate) fn fixture_exchange(
    binding: (&Store, &HeldDir),
    account: &Account,
    reader: &mut impl BufRead,
    output: &mut impl Write,
    mut ingress: Ingress<'_>,
) -> io::Result<()> {
    let outcome = match read_head(reader, || Ok(())) {
        Ok(Some(raw)) => match head(&raw) {
            Ok(request) if uploads::is_target(request.target) => {
                match resource(
                    |scope| Uploads::bind(binding.0, binding.1, scope),
                    account,
                    &request,
                    reader,
                    &mut ingress,
                ) {
                    Ok(reply) => {
                        match reply.before_wire(account, ingress.stopping, (ingress.now)()) {
                            Ok(()) => reply.outcome,
                            Err(problem) => Outcome::Failed(problem),
                        }
                    }
                    Err(problem) => Outcome::Failed(problem),
                }
            }
            Ok(_) => Outcome::Failed(Problem::Missing),
            Err(problem) => Outcome::Failed(problem),
        },
        Ok(None) => Outcome::Failed(Problem::Malformed),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof
            ) =>
        {
            Outcome::Failed(Problem::Malformed)
        }
        Err(error) => return Err(error),
    };
    write_outcome(output, outcome, false)?;
    output.flush()
}
