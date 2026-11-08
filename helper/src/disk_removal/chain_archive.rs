//! Download a complete native management-chain checkpoint. No uploaded proof,
//! new operation, target writer or service activation is selected. Opening the
//! original native owner may repeat its existing recovery/synchronization.
use super::*;
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Request {
    context: layout::Context,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Export {
    checkpoint: storage_engine::TerminalRetirementCheckpoint,
    payload: String,
    source_open_recovery_may_write: bool,
    contains_full_source_control_archive: bool,
    checkpoint_latest_only_at_capture: bool,
    service_started: bool,
}
impl crate::service_worker::Request for Request {
    type Output = Export;
    const COMMAND: &'static str = "internal-terminal-chain-worker";
    // One native <=64 MiB canonical JSON object, escaped once for bounded IPC.
    // Requests remain at the global 64 KiB limit; other workers retain it too.
    const MAX_RESPONSE_BYTES: usize = 128 * 1024 * 1024 + 64 * 1024;
    fn account(&self) -> &crate::account::Snapshot {
        &self.context.account
    }
    fn execute(self) -> Result<Export> {
        let target = worker::Target::open(&self.context)?;
        target.recheck()?;
        let archive = target.control.export_terminal_retirement_archive()?;
        target.recheck()?;
        Ok(Export {
            checkpoint: archive.checkpoint().clone(),
            payload: String::from_utf8(archive.bytes().to_vec())?,
            source_open_recovery_may_write: true,
            contains_full_source_control_archive: false,
            checkpoint_latest_only_at_capture: true,
            service_started: false,
        })
    }
}
pub(crate) fn worker_entry() -> Result<()> {
    crate::service_worker::entry::<Request>()
}
pub(crate) fn export(input: InventoryInput) -> Result<Export> {
    stopped(input.external_writers_stopped)?;
    let root = Root::open(input.service_user.clone())?;
    let context = root.context.clone();
    drop(root);
    let archive = crate::service_worker::run(Request {
        context: context.clone(),
    })?;
    let root = Root::open(input.service_user)?;
    if root.context != context {
        return Err("terminal checkpoint host/account/layout changed; no archive accepted".into());
    }
    root.host.recheck()?;
    root.layout.recheck()?;
    Ok(archive)
}
