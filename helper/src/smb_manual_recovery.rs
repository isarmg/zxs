//! Online client only. The daemon retains all storage and recovery authority.
use crate::{Result, print};
use command_line::{Args, Subcommand};
use std::path::PathBuf;
use storage_engine::OperationId;

#[derive(Args)]
pub(crate) struct Options {
    /// The running daemon's protected local SMB bridge socket.
    #[arg(long)]
    socket: PathBuf,
    /// The configured Samba account; the server revalidates its binding.
    #[arg(long)]
    username: String,
    /// The original operation from the retained manual save, never a new ID.
    #[arg(long)]
    operation: OperationId,
    /// Exact retained working-record basename, for work not yet fully frozen.
    #[arg(long)]
    working_record: Option<String>,
    #[command(subcommand)]
    action: Option<Action>,
}

#[derive(Subcommand)]
enum Action {
    /// Inspect only the original request result; this is the default.
    Query,
    /// Resume the exact frozen request if still unpublished and currently authorized.
    Resume,
}

impl Options {
    fn resumes(&self) -> bool {
        matches!(self.action, Some(Action::Resume))
    }
}

pub(crate) fn run(options: Options) -> Result<()> {
    // No engine open, direct spool access or offline ownership is permitted in
    // this command. Peer/account admission and original-result ownership stay
    // in the normal server protocol and Backend boundaries.
    let client = smb_protocol::client::Client::connect_default(&options.socket, &options.username)?;
    let resume = options.resumes();
    let response = match options.working_record {
        Some(record) => client.recover_manual_working(options.operation, record, resume)?,
        None => client.recover_manual_draft(options.operation, resume)?,
    };
    print(response)
}

#[cfg(test)]
#[path = "../../test/helper/unit/smb_manual_recovery_tests.rs"]
mod tests;
