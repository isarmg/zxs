//! Pipe client for the exact bundled ZXS program, not a linked daemon backend.
//! No pathname from a request selects a program; no listener/service is started.
use http_routing::http::StatusCode;
use platform_io::secret::SecretBytes;
use std::{
    io::{BufRead, Read, Write},
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::{Child, Command, Stdio},
};
use storage_engine::offline_maintenance::{self as wire, FailureKind, Operation, ReplyHeader};

type Cause = Box<dyn std::error::Error + Send + Sync>;
#[derive(Debug)]
pub(crate) enum Error {
    Payload(Cause),
    Transport { stage: &'static str, source: Cause },
    Rejected { kind: FailureKind, message: String },
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Payload(source) => write!(
                f,
                "matching embedded maintenance program is unavailable: {source}"
            ),
            Self::Transport { stage, source } => write!(
                f,
                "maintenance {stage}: {source}; preserve the original operation before retrying; no rollback is claimed"
            ),
            Self::Rejected { message, .. } => f.write_str(message),
        }
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Payload(source) | Self::Transport { source, .. } => Some(source.as_ref()),
            Self::Rejected { .. } => None,
        }
    }
}
impl Error {
    pub(crate) fn status(&self) -> StatusCode {
        match self {
            Self::Payload(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::Transport { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Rejected {
                kind: FailureKind::InvalidRequest,
                ..
            } => StatusCode::BAD_REQUEST,
            Self::Rejected {
                kind: FailureKind::Conflict,
                ..
            } => StatusCode::CONFLICT,
        }
    }
    pub(crate) fn code(&self, operation: Operation) -> &'static str {
        match self {
            Self::Payload(_) => "maintenance_payload_unavailable",
            Self::Transport { .. } => "maintenance_transport",
            Self::Rejected {
                kind: FailureKind::InvalidRequest,
                ..
            } => "request_json",
            Self::Rejected {
                kind: FailureKind::Conflict,
                ..
            } => operation.failure_code(),
        }
    }
}
fn transport(stage: &'static str, source: impl Into<Cause>) -> Error {
    Error::Transport {
        stage,
        source: source.into(),
    }
}
struct Joined {
    child: Child,
    reaped: bool,
}
impl Joined {
    fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        let status = self.child.wait()?;
        self.reaped = true;
        Ok(status)
    }
}
impl Drop for Joined {
    fn drop(&mut self) {
        if !self.reaped
            && let Err(error) = self.child.wait()
        {
            tracing::error!(%error, "maintenance child could not be reaped; inspect original operation before retrying");
        }
    }
}

pub(crate) fn run(operation: Operation, body: SecretBytes) -> Result<Vec<u8>, Error> {
    if rustix::process::getuid().as_raw() != 0 || rustix::process::geteuid().as_raw() != 0 {
        return Err(transport("admission", "manually run root helper required"));
    }
    if body.expose().len() > wire::MAX_REQUEST_BYTES {
        return Err(transport(
            "request",
            "bounded request exceeded before program access",
        ));
    }
    let executable = crate::installation::worker::open().map_err(Error::Payload)?;
    let child = Command::new(format!("/proc/self/fd/{}", executable.as_raw_fd()))
        .arg0("zxs")
        .arg("internal-maintenance")
        .env_clear()
        .current_dir("/")
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|source| transport("spawn", source))?;
    let mut child = Joined {
        child,
        reaped: false,
    };
    let mut input = child
        .child
        .stdin
        .take()
        .expect("invariant: maintenance stdin is piped");
    let header = wire::RequestHeader {
        format: wire::Format::Current,
        operation,
    };
    let written = (|| -> crate::Result<()> {
        input.write_all(&json_codec::to_vec(&header)?)?;
        input.write_all(b"\n")?;
        input.write_all(body.expose())?;
        Ok(())
    })();
    drop(body);
    drop(input); // EOF also terminates a partial request; no retry is automatic.
    let output = child
        .child
        .stdout
        .take()
        .expect("invariant: maintenance stdout is piped");
    let mut output = std::io::BufReader::new(output);
    let reply = read_reply(operation, &mut output);
    // Always drain and join, including invalid/overlong output. A cancelled HTTP
    // request cannot release its job/lease while the program still owns work.
    let drained = std::io::copy(&mut output, &mut std::io::sink());
    drop(output);
    let status = child.wait().map_err(|source| transport("wait", source))?;
    written.map_err(|source| transport("request write", source))?;
    drained.map_err(|source| transport("response drain", source))?;
    if !status.success() {
        return Err(transport(
            "exit",
            "embedded program failed without a complete successful protocol exit",
        ));
    }
    reply
}
fn read_reply(operation: Operation, output: &mut impl BufRead) -> Result<Vec<u8>, Error> {
    let mut header = Vec::new();
    (&mut *output)
        .take(wire::MAX_HEADER_BYTES as u64 + 1)
        .read_until(b'\n', &mut header)
        .map_err(|source| transport("response header", source))?;
    if header.len() > wire::MAX_HEADER_BYTES || header.last() != Some(&b'\n') {
        return Err(transport(
            "response header",
            "missing, incomplete or oversized reply header",
        ));
    }
    let header: ReplyHeader =
        json_codec::from_slice(&header).map_err(|source| transport("response header", source))?;
    let limit = match &header {
        ReplyHeader::Success { body_bytes } => {
            if *body_bytes == 0 || *body_bytes > operation.response_limit() {
                return Err(transport("response body", "invalid declared response size"));
            }
            *body_bytes
        },
        ReplyHeader::Failed { .. } => 0,
    };
    let mut body = Vec::new();
    output
        .take(limit as u64 + 1)
        .read_to_end(&mut body)
        .map_err(|source| transport("response body", source))?;
    if body.len() != limit {
        return Err(transport(
            "response body",
            "reply differs from its exact declared response size",
        ));
    }
    match header {
        ReplyHeader::Success { .. } => Ok(body),
        ReplyHeader::Failed { kind, message } => Err(Error::Rejected { kind, message }),
    }
}

#[cfg(test)]
#[path = "../../test/helper/unit/maintenance_tests.rs"]
mod tests;
