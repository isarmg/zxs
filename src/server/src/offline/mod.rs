//! Non-listening maintenance entry for the exact embedded helper payload.
//! It is not daemon startup, a network API, or an alternate credential policy.
use crate::Result;
use command_line::Subcommand;
use data_serialization::{Serialize, de::DeserializeOwned};
use platform_io::local_account as account;
use platform_io::secret::SecretBytes;
use std::io::{BufRead, Read, Write};
use storage_contract::offline_maintenance::{
    self as wire, FailureKind, Operation, ReplyHeader, RequestHeader,
};
mod private_io {
    pub(crate) use platform_io::publication::BundleDestination;
}
mod credentials;
mod disk_addition;
mod disk_readmission;
mod disk_removal;
mod foreign_chain;
mod initialization;
mod managed_layout;
mod managed_pool;
mod service_worker;

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Pipe-only host coordinator. No listener, service activation or background runtime.
    #[command(name = "internal-maintenance", hide = true)]
    Maintenance,
    #[command(name = "internal-storage-worker", hide = true)]
    StorageWorker,
    #[command(name = "internal-addition-worker", hide = true)]
    AdditionWorker,
    #[command(name = "internal-foreign-chain-worker", hide = true)]
    ForeignChainWorker,
    #[command(name = "internal-removal-worker", hide = true)]
    RemovalWorker,
    #[command(name = "internal-readmission-worker", hide = true)]
    ReadmissionWorker,
    #[command(name = "internal-terminal-chain-worker", hide = true)]
    TerminalChainWorker,
    #[command(name = "internal-chain-readmission-worker", hide = true)]
    ChainReadmissionWorker,
    #[command(name = "internal-chain-target-worker", hide = true)]
    ChainTargetWorker,
}
pub(crate) fn run(command: Command) -> Result<()> {
    match command {
        Command::Maintenance => entry(),
        Command::StorageWorker => initialization::storage::process::entry(),
        Command::AdditionWorker => disk_addition::worker_entry(),
        Command::ForeignChainWorker => foreign_chain::worker_entry(),
        Command::RemovalWorker => disk_removal::worker_entry(),
        Command::ReadmissionWorker => disk_readmission::worker_entry(),
        Command::TerminalChainWorker => disk_removal::chain_archive::worker_entry(),
        Command::ChainReadmissionWorker => disk_removal::chain_readmission::worker_entry(),
        Command::ChainTargetWorker => disk_removal::chain_target::worker_entry(),
    }
}
#[derive(Debug)]
struct Failure {
    kind: FailureKind,
    message: String,
}
fn call<T: DeserializeOwned, U: Serialize>(
    operation: Operation,
    bytes: &[u8],
    run: impl FnOnce(T) -> Result<U>,
) -> std::result::Result<Vec<u8>, Failure> {
    let input = json_codec::from_slice(bytes).map_err(|error| Failure {
        kind: FailureKind::InvalidRequest,
        message: if operation.secret() {
            "Invalid credential request; password values and parser details are not returned."
                .into()
        } else {
            error.to_string()
        },
    })?;
    let output = run(input).map_err(|error| Failure {
        kind: FailureKind::Conflict,
        message: if operation.terse_failure() {
            error.to_string()
        } else {
            diagnostic_chain(error.as_ref())
        },
    })?;
    let encoded = json_codec::to_vec(&output).map_err(|error| Failure {
        kind: FailureKind::Conflict,
        message: error.to_string(),
    })?;
    if encoded.len() > operation.response_limit() {
        return Err(Failure { kind: FailureKind::Conflict, message: "maintenance response exceeds its bound; preserve the original operation before retrying".into() });
    }
    Ok(encoded)
}
fn dispatch(operation: Operation, body: &[u8]) -> std::result::Result<Vec<u8>, Failure> {
    match operation {
        Operation::AdministratorResetPreview => {
            call::<credentials::Input, _>(operation, body, credentials::preview)
        }
        Operation::AdministratorReset => {
            call::<credentials::Reset, _>(operation, body, credentials::reset)
        }
        Operation::AdministratorResetRecoverPreview => {
            call::<credentials::RecoverPreview, _>(operation, body, credentials::recover_preview)
        }
        Operation::FirstAdministratorPreview => call::<initialization::administrator::Input, _>(
            operation,
            body,
            initialization::administrator::preview,
        ),
        Operation::FirstAdministratorRecoverPreview => {
            call::<initialization::administrator::RecoverPreview, _>(
                operation,
                body,
                initialization::administrator::recover_preview,
            )
        }
        Operation::FirstAdministratorCreate => call::<initialization::administrator::Create, _>(
            operation,
            body,
            initialization::administrator::create,
        ),
        Operation::FirstStoragePreview => call::<initialization::storage::Input, _>(
            operation,
            body,
            initialization::storage::preview,
        ),
        Operation::FirstStorageRecover => call::<initialization::storage::Original, _>(
            operation,
            body,
            initialization::storage::recover_preview,
        ),
        Operation::FirstStorageCreate => call::<initialization::storage::Commit, _>(
            operation,
            body,
            initialization::storage::commit,
        ),
        Operation::ForeignChainPreview => {
            call::<foreign_chain::Input, _>(operation, body, foreign_chain::preview)
        }
        Operation::ForeignChainOriginal => {
            call::<foreign_chain::Original, _>(operation, body, foreign_chain::original)
        }
        Operation::ForeignChainPrepare => {
            call::<foreign_chain::Prepare, _>(operation, body, foreign_chain::prepare)
        }
        Operation::ForeignChainDatabasesPreview => {
            call::<foreign_chain::Original, _>(operation, body, foreign_chain::databases::preview)
        }
        Operation::ForeignChainDatabasesPrepare => call::<foreign_chain::databases::Prepare, _>(
            operation,
            body,
            foreign_chain::databases::prepare,
        ),
        Operation::ForeignChainIdentityPreview => {
            call::<foreign_chain::Original, _>(operation, body, foreign_chain::identity::preview)
        }
        Operation::ForeignChainObjectsPreview => {
            call::<foreign_chain::Original, _>(operation, body, foreign_chain::objects::preview)
        }
        Operation::ForeignChainObjectsPrepare => call::<foreign_chain::objects::Prepare, _>(
            operation,
            body,
            foreign_chain::objects::prepare,
        ),
        Operation::DiskAdditionPreview => {
            call::<disk_addition::Input, _>(operation, body, disk_addition::preview)
        }
        Operation::DiskAdditionOriginal => {
            call::<disk_addition::Original, _>(operation, body, disk_addition::recover_preview)
        }
        Operation::DiskAdditionCommit => {
            call::<disk_addition::Commit, _>(operation, body, disk_addition::commit)
        }
        Operation::DiskRemovalInventory => {
            call::<disk_removal::InventoryInput, _>(operation, body, disk_removal::inventory)
        }
        Operation::DiskRemovalPreview => {
            call::<disk_removal::Input, _>(operation, body, disk_removal::preview)
        }
        Operation::DiskRemovalOriginal => {
            call::<disk_removal::Original, _>(operation, body, disk_removal::recover_preview)
        }
        Operation::DiskRemovalCommit => {
            call::<disk_removal::Commit, _>(operation, body, disk_removal::commit)
        }
        Operation::TerminalRemovalInspect => {
            call::<disk_removal::terminal::Input, _>(operation, body, disk_removal::terminal::run)
        }
        Operation::TerminalRetirementPreview => {
            call::<disk_removal::Input, _>(operation, body, disk_removal::terminal_action::preview)
        }
        Operation::TerminalRetirementOriginal => call::<disk_removal::Original, _>(
            operation,
            body,
            disk_removal::terminal_action::original,
        ),
        Operation::TerminalRetirementCommit => call::<disk_removal::terminal_action::Commit, _>(
            operation,
            body,
            disk_removal::terminal_action::commit,
        ),
        Operation::DiskReadmissionPreview => {
            call::<disk_readmission::Input, _>(operation, body, disk_readmission::preview)
        }
        Operation::DiskReadmissionOriginal => {
            call::<disk_readmission::Original, _>(operation, body, disk_readmission::original)
        }
        Operation::DiskReadmissionCommit => {
            call::<disk_readmission::Commit, _>(operation, body, disk_readmission::commit)
        }
        Operation::TerminalChainExport => call::<disk_removal::InventoryInput, _>(
            operation,
            body,
            disk_removal::chain_archive::export,
        ),
        Operation::TerminalChainReadmissionProposal => {
            call::<disk_removal::Input, _>(operation, body, disk_removal::chain_readmission::export)
        }
        Operation::TerminalChainTargetStatus => call::<disk_removal::InventoryInput, _>(
            operation,
            body,
            disk_removal::chain_target::status,
        ),
        Operation::TerminalChainTargetSynchronize => call::<disk_removal::chain_target::Commit, _>(
            operation,
            body,
            disk_removal::chain_target::synchronize,
        ),
        Operation::TerminalChainFirstPrepare => call::<disk_removal::chain_target::FirstInput, _>(
            operation,
            body,
            disk_removal::chain_target::prepare_first,
        ),
        Operation::TerminalChainFirstPublish => {
            call::<disk_removal::chain_target::PublicationInput, _>(
                operation,
                body,
                disk_removal::chain_target::publish_first,
            )
        }
        Operation::TerminalChainCurrent => call::<disk_removal::chain_target::CurrentInput, _>(
            operation,
            body,
            disk_removal::chain_target::inspect_current,
        ),
        Operation::TerminalChainActivationPlan => {
            call::<disk_removal::chain_target::ActivationPlanInput, _>(
                operation,
                body,
                disk_removal::chain_target::prepare_activation,
            )
        }
        Operation::TerminalChainIoDecide => call::<disk_removal::chain_target::IoDecisionInput, _>(
            operation,
            body,
            disk_removal::chain_target::decide_io,
        ),
        Operation::TerminalChainGenerationLog => {
            call::<disk_removal::chain_target::GenerationLogInput, _>(
                operation,
                body,
                disk_removal::chain_target::prepare_generation_log,
            )
        }
        Operation::TerminalChainGenerationSelect => {
            call::<disk_removal::chain_target::GenerationSelectionInput, _>(
                operation,
                body,
                disk_removal::chain_target::select_generation,
            )
        }
        Operation::TerminalChainGenerationRead => {
            call::<disk_removal::chain_target::CurrentInput, _>(
                operation,
                body,
                disk_removal::chain_target::inspect_generation,
            )
        }
    }
}
fn entry() -> Result<()> {
    if rustix::process::getuid().as_raw() != 0 || rustix::process::geteuid().as_raw() != 0 {
        return Err("offline management coordinator requires the manually run root helper".into());
    }
    let mut input = std::io::BufReader::new(std::io::stdin().lock());
    let (header, body) = read_request(&mut input)?;
    let result = dispatch(header.operation, body.expose());
    drop(body);
    let (header, body) = match result {
        Ok(body) => (
            ReplyHeader::Success {
                body_bytes: body.len(),
            },
            body,
        ),
        Err(Failure { kind, message }) => (ReplyHeader::Failed { kind, message }, Vec::new()),
    };
    let encoded = json_codec::to_vec(&header)?;
    if encoded.len() + 1 > wire::MAX_HEADER_BYTES {
        return Err("maintenance diagnostic exceeds its bound; preserve original intent".into());
    }
    let mut output = std::io::stdout().lock();
    output.write_all(&encoded)?;
    output.write_all(b"\n")?;
    output.write_all(&body)?;
    output.flush()?;
    Ok(())
}
fn read_request(input: &mut impl BufRead) -> Result<(RequestHeader, SecretBytes)> {
    let mut header = Vec::new();
    (&mut *input)
        .take(wire::MAX_HEADER_BYTES as u64 + 1)
        .read_until(b'\n', &mut header)?;
    if header.len() > wire::MAX_HEADER_BYTES || header.last() != Some(&b'\n') {
        return Err("invalid or oversized maintenance header; no host access was attempted".into());
    }
    let header: RequestHeader = json_codec::from_slice(&header)?;
    let mut body = Vec::new();
    let read = input
        .take(wire::MAX_REQUEST_BYTES as u64 + 1)
        .read_to_end(&mut body);
    // Own partial input as a secret before propagating an I/O failure.
    let body = SecretBytes::new(body);
    read?;
    if body.expose().len() > wire::MAX_REQUEST_BYTES {
        return Err("maintenance request exceeds its bound; no host access was attempted".into());
    }
    Ok((header, body))
}
/// Only final non-secret diagnostics retain their cause chain.
fn diagnostic_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut message = String::new();
    let mut current = Some(error);
    while let Some(cause) = current {
        if !message.is_empty() {
            message.push_str("; caused by: ");
        }
        for character in cause.to_string().chars() {
            if character.is_control() {
                message.extend(character.escape_default());
            } else {
                message.push(character);
            }
        }
        current = cause.source();
    }
    message
}

#[cfg(test)]
#[path = "../../../../test/server/unit/offline/protocol_request_tests.rs"]
mod protocol_request_tests;

#[cfg(test)]
#[path = "../../../../test/server/unit/offline/pipe_tests.rs"]
mod pipe_tests;
