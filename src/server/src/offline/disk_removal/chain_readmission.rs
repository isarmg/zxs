//! A complete current-source proposal download. No target directory is opened,
//! no native decision is reserved and no uploaded proposal can authorize writes.
use super::*;
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Request {
    context: layout::Context,
    readmission: storage_engine::storage_readmission::ReadmissionRequest,
}
#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Plan {
    pub(super) context: layout::Context,
    pub(super) credential: CredentialRevision,
    pub(super) preparation: storage_engine::TerminalChainReadmissionPreparation,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Export {
    plan: Plan,
    preparation: storage_engine::TerminalChainReadmissionPreparation,
    payload: String,
    source_open_recovery_may_write: bool,
    proposal_is_reserved: bool,
    source_latest_only_at_capture: bool,
    contains_full_source_control_archive: bool,
    service_started: bool,
}
impl crate::offline::service_worker::Request for Request {
    type Output = Export;
    const COMMAND: &'static str = "internal-chain-readmission-worker";
    const MAX_RESPONSE_BYTES: usize = 128 * 1024 * 1024 + 64 * 1024;
    fn account(&self) -> &crate::offline::account::Snapshot {
        &self.context.account
    }
    fn execute(self) -> Result<Export> {
        let target = worker::Target::open(&self.context)?;
        target.recheck()?;
        let proposal = target
            .control
            .prepare_terminal_chain_readmission(self.readmission)?;
        target.recheck()?;
        Ok(Export {
            plan: Plan {
                context: self.context,
                credential: target.credential_revision()?,
                preparation: proposal.preparation().clone(),
            },
            preparation: proposal.preparation().clone(),
            payload: String::from_utf8(proposal.bytes().to_vec())?,
            source_open_recovery_may_write: true,
            proposal_is_reserved: false,
            source_latest_only_at_capture: true,
            contains_full_source_control_archive: false,
            service_started: false,
        })
    }
}
pub(crate) fn worker_entry() -> Result<()> {
    crate::offline::service_worker::entry::<Request>()
}
pub(crate) fn export(input: Input) -> Result<Export> {
    stopped(input.external_writers_stopped)?;
    let root = Root::open(input.service_user.clone())?;
    let context = root.context.clone();
    drop(root);
    let result = crate::offline::service_worker::run(Request {
        context: context.clone(),
        readmission: storage_engine::storage_readmission::ReadmissionRequest {
            operation_id: input.operation_id,
            slot_id: input.slot_id,
            expected_disk_id: input.expected_disk_id,
        },
    })?;
    let root = Root::open(input.service_user)?;
    if root.context != context {
        return Err(
            "chain readmission source host/account/layout changed; proposal not accepted".into(),
        );
    }
    root.host.recheck()?;
    root.layout.recheck()?;
    Ok(result)
}
