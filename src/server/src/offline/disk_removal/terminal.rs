//! Terminal remaining-member diagnostics. A source commitment and a valid
//! registration are not a successor publication: this module never deregisters.
use super::*;
use storage_engine::storage_readmission::{RegistrationEntry, RegistrationSnapshot};
use storage_server::management::storage_lifecycle::{ExclusiveExit, TerminalBoundary};

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Input {
    service_user: String,
    slot_id: String,
    expected_disk_id: String,
    external_writers_stopped: bool,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Source {
    pool_id: String,
    terminal_operation_id: OperationId,
    terminal_authority_hash: String,
    registry_root_hash: String,
    #[serialization(with = "storage_engine::u64_string")]
    registry_generation: u64,
    #[serialization(with = "storage_engine::u64_string")]
    sequence: u64,
    #[serialization(with = "storage_engine::u64_string")]
    topology_generation: u64,
    pending_retirement: Option<storage_engine::storage_lifecycle::TerminalRetirementPending>,
}
#[derive(Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
enum Blocker {
    ExplicitSuccessorPreviewRequired,
    OriginalTerminalRetirementPending,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Selection {
    source: Source,
    target: RegistrationEntry,
    recorded_root_path: String,
    level: u32,
    upstream_slot_id: Option<String>,
    registered_direct_dependents: Vec<RegistrationEntry>,
    current_registered_directories: usize,
    proposed_remaining_directories: usize,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Inspection {
    format: InspectionFormat,
    selection: Selection,
    inspection_digest: String,
    executable: bool,
    blocker: Blocker,
    operation_created: bool,
    physical_bytes_erased: bool,
    target_io_probed: bool,
    existing_original_recovery_may_run: bool,
}
#[derive(Serialize, Deserialize)]
enum InspectionFormat {
    #[serialization(rename = "zxs-helper-terminal-removal-inspection")]
    TerminalRemoval,
}
pub(super) fn capture(control: &ExclusiveExit, slot: &str, disk: &str) -> Result<Inspection> {
    let boundary = control.terminal_boundary()?.ok_or(
        "terminal removal inspection requires admitted terminal authority, not an active or missing pool",
    )?;
    let config = control.current_configuration()?;
    let RegistrationSnapshot::TerminalManagement { registrations } = control.registrations()?
    else {
        return Err(
            "terminal boundary has no matching terminal registry; no empty pool was inferred"
                .into(),
        );
    };
    inspect(boundary, config, registrations, slot, disk)
}
fn inspect(
    boundary: TerminalBoundary,
    config: storage_engine::Config,
    registrations: Vec<RegistrationEntry>,
    slot: &str,
    disk: &str,
) -> Result<Inspection> {
    let target = registrations
        .iter()
        .find(|entry| entry.slot_id == slot && entry.disk_id == disk)
        .ok_or("selected terminal member no longer has this exact slot/disk identity")?
        .clone();
    if target.metadata_member || registrations.iter().any(|entry| entry.metadata_member) {
        return Err("terminal inspection cannot adopt an active voter registry".into());
    }
    let definition = config
        .disks
        .iter()
        .find(|entry| {
            entry.disk_id == target.disk_id
                && entry.slot_id == target.slot_id
                && entry.binding_generation == target.binding_generation
        })
        .ok_or("terminal registration differs from its authenticated historical definition")?;
    let root = definition
        .root
        .to_str()
        .ok_or("terminal registered root is not UTF-8")?
        .to_owned();
    layout::names(std::slice::from_ref(&root))?;
    let mut dependents = Vec::new();
    for entry in &registrations {
        let historical = config
            .disks
            .iter()
            .find(|definition| {
                definition.disk_id == entry.disk_id
                    && definition.slot_id == entry.slot_id
                    && definition.binding_generation == entry.binding_generation
            })
            .ok_or("terminal member has no exact historical topology definition")?;
        if historical.upstream_slot_id.as_deref() == Some(target.slot_id.as_str()) {
            dependents.push(entry.clone());
        }
    }
    let blocker = if boundary.pending_retirement.is_some() {
        Blocker::OriginalTerminalRetirementPending
    } else {
        Blocker::ExplicitSuccessorPreviewRequired
    };
    let selection = Selection {
        source: Source {
            pool_id: config.pool_id.clone(),
            terminal_operation_id: boundary.operation_id,
            terminal_authority_hash: boundary.authority_hash,
            registry_root_hash: boundary.registry.root_hash().to_owned(),
            registry_generation: boundary.registry.view_generation(),
            sequence: boundary.sequence,
            topology_generation: boundary.topology_generation,
            pending_retirement: boundary.pending_retirement,
        },
        target,
        recorded_root_path: root,
        level: definition.level,
        upstream_slot_id: definition.upstream_slot_id.clone(),
        registered_direct_dependents: dependents,
        current_registered_directories: registrations.len(),
        // The exact selected target above proves this collection is nonempty.
        proposed_remaining_directories: registrations.len() - 1,
    };
    let inspection_digest =
        platform_io::lower_hex(&Sha256::digest(json_codec::to_vec(&selection)?));
    Ok(Inspection {
        format: InspectionFormat::TerminalRemoval,
        selection,
        inspection_digest,
        executable: false,
        blocker,
        operation_created: false,
        physical_bytes_erased: false,
        target_io_probed: false,
        existing_original_recovery_may_run: true,
    })
}
/// Opens only the existing native owner. No new root plan, native exit intent,
/// operation ID or approval token is created for this unsupported mutation.
pub(crate) fn run(input: Input) -> Result<Inspection> {
    stopped(input.external_writers_stopped)?;
    let root = Root::open(input.service_user.clone())?;
    let context = root.context.clone();
    drop(root);
    let Output::TerminalInspection { inspection } =
        crate::offline::service_worker::run(WorkerRequest {
            context: context.clone(),
            action: Action::InspectTerminal {
                slot_id: input.slot_id,
                expected_disk_id: input.expected_disk_id,
            },
        })?
    else {
        return Err("unexpected terminal inspection reply".into());
    };
    let root = Root::open(input.service_user)?;
    if root.context != context {
        return Err("terminal inspection root/account identity changed; this is not a retained source proof".into());
    }
    Ok(inspection)
}
#[cfg(test)]
#[path = "../../../../../test/server/unit/offline/terminal_removal_inspection.rs"]
mod tests;
