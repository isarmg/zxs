//! Pipe observations do not decide whether the original storage operation ran.
use std::{fmt, io, process::ExitStatus};

#[derive(Clone, Copy, Debug)]
pub(super) enum Stage {
    Spawn,
    WriteRequest,
    ReadReply,
    DrainReply,
    Wait,
}
impl fmt::Display for Stage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Spawn => "spawn",
            Self::WriteRequest => "request write",
            Self::ReadReply => "reply read",
            Self::DrainReply => "reply drain",
            Self::Wait => "process wait",
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum RequestPhase {
    Execution,
    OriginalInspection,
}
impl fmt::Display for RequestPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Execution => "execution",
            Self::OriginalInspection => "original inspection",
        })
    }
}

#[derive(Debug, error_definition::Error)]
pub(super) enum Failure {
    #[error(
        "storage worker {phase} envelope exceeds its {maximum}-byte IPC budget; no intent was written or changed by this attempt; preserve any original plan and digest"
    )]
    RequestBudget { phase: RequestPhase, maximum: usize },
    #[error(
        "storage worker request exceeds its {maximum}-byte IPC budget before dispatch; no child was started; preserve any original intent"
    )]
    DispatchBudget { maximum: usize },
    #[error(
        "storage worker {stage} failed; preserve original intent and inspect its outcome; no rollback is claimed"
    )]
    Io {
        stage: Stage,
        #[source]
        source: io::Error,
    },
    #[error(
        "storage worker exited with {0}; preserve original intent and inspect its outcome; no rollback is claimed"
    )]
    Exit(ExitStatus),
    #[error(
        "storage worker reply exceeded {maximum} bytes; preserve original intent and inspect its outcome; no rollback is claimed"
    )]
    ReplyBudget { maximum: usize },
}
impl Failure {
    pub(super) fn io(stage: Stage, source: io::Error) -> Self {
        Self::Io { stage, source }
    }
}

pub(super) fn admit_reply(
    status: ExitStatus,
    bytes: usize,
    maximum: usize,
) -> Result<(), Failure> {
    // The process has already been joined and excess stdout drained. A failed
    // exit cannot be turned into success by a plausible or partial JSON reply.
    if !status.success() {
        return Err(Failure::Exit(status));
    }
    if bytes > maximum {
        return Err(Failure::ReplyBudget { maximum });
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../test/server/unit/offline/worker_failure_tests.rs"]
mod tests;
