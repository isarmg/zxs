//! Executes only after the independent process has dropped root privileges.
use super::{
    Action, Format, Plan, WorkerOutput, WorkerRequest,
    layout::{self, Layout},
};
use crate::Result;
use data_serialization::{Deserialize, Serialize};
use json_codec::json;
use platform_io::service_lease::ServiceLease;
use std::{collections::BTreeMap, path::PathBuf};
use storage_contract::storage_directories::{CandidateSelection, InspectRequest};
use storage_engine::{Config, Engine, IndexHealth, Mode, OperationId, ValidatedConfig};
use storage_server::{
    ApiError,
    auth::AuthStore,
    setup_recovery::{InstallationState, SetupRecovery},
    storage_directories,
};

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Materialized {
    pool_id: String,
    sequence: String,
    primary_disks: Vec<DiskMaterial>,
    system_catalog_current: bool,
    credential_recovery_snapshot_published: bool,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct DiskMaterial {
    slot_id: String,
    disk_id: String,
    current_records_hash: String,
    history_records_hash: String,
}
struct Target {
    auth: AuthStore,
    layout: Layout,
    setup: SetupRecovery,
    // No lock ownership gap while a worker can touch authority or storage.
    lease: ServiceLease,
}
impl Target {
    fn open(request: &WorkerRequest) -> Result<Self> {
        let lease = ServiceLease::host()?;
        let layout = Layout::open(request.context.account.name.as_str().into())?;
        if layout.context(lease.identity()) != request.context {
            return Err("storage worker rejected changed root/account/layout identities before storage access".into());
        }
        layout.control()?;
        let auth = AuthStore::open_system_management_directory(layout.management()?)?;
        let setup = SetupRecovery::new(
            PathBuf::from(layout::STATE),
            PathBuf::from(layout::MANAGEMENT),
            BTreeMap::new(),
        )?
        .with_discovery_root(layout::DISKS.parse()?);
        Ok(Self {
            auth,
            layout,
            setup,
            lease,
        })
    }
    fn recheck(&self) -> Result<()> {
        self.lease.recheck()?;
        self.layout.recheck()
    }
    fn plan(&self, plan: &Plan) -> Result<()> {
        self.recheck()?;
        if self.layout.context(self.lease.identity()) != plan.context
            || self.auth.credential_revision()? != plan.credential
        {
            return Err("original storage plan no longer matches current directory identities or management credentials/sessions".into());
        }
        self.layout.inventory(&plan.roots())
    }
    fn fresh(&self) -> Result<()> {
        self.layout.fresh()?;
        if self.setup.management_status() != InstallationState::ManagementReady {
            return Err("first storage requires completed management without an existing pool or recovery evidence".into());
        }
        Ok(())
    }
    fn initial_selections(&self, roots: &[String]) -> Result<Vec<CandidateSelection>> {
        self.fresh()?;
        self.layout.inventory(roots)?;
        let mut excluded = vec![
            PathBuf::from(layout::STATE),
            PathBuf::from(layout::MANAGEMENT),
        ];
        let mut selections = Vec::with_capacity(roots.len());
        for root in roots {
            let location = storage_directories::browse(root)?;
            let selection = storage_directories::inspect_within(
                layout::DISKS,
                &InspectRequest {
                    mount_path: location.mount_path,
                    root_path: root.clone(),
                },
                &excluded,
            )?;
            excluded.push(PathBuf::from(root));
            selections.push(selection);
        }
        // Preview the actual production contract; no silent development fallback.
        let config = Config::independent_roots(Mode::Production, roots.iter().map(PathBuf::from))?;
        let _validated = ValidatedConfig::try_from(config)?;
        self.recheck()?;
        Ok(selections)
    }
    fn original(&self, plan: &Plan) -> Result<bool> {
        self.plan(plan)?;
        let started = self.setup.initial_activation_matches(&plan.activation())?;
        if !started && self.initial_selections(&plan.roots())? != plan.selections {
            return Err("unstarted original storage selection changed; it cannot be silently re-previewed or reinitialized".into());
        }
        Ok(started)
    }
}
pub(super) fn execute(request: WorkerRequest) -> Result<WorkerOutput> {
    let target = Target::open(&request)?;
    match request.action {
        Action::Preview { roots } => {
            let selections = target.initial_selections(&roots)?;
            let plan = Plan {
                format: Format::FirstStorage,
                context: request.context,
                credential: target.auth.credential_revision()?,
                selections,
            };
            Ok(WorkerOutput::Preview { plan })
        }
        Action::InspectOriginal { plan } => {
            // Correlation only. A partly initialized pool remains possibly
            // unrecoverable; only Execute can run the core completion verifier.
            target.original(&plan)?;
            Ok(WorkerOutput::OriginalInspected)
        }
        Action::Execute { plan } => {
            initialize(&target, &plan).map(|report| WorkerOutput::Initialized { report })
        }
    }
}
fn initialize(target: &Target, plan: &Plan) -> Result<Materialized> {
    let started = target.original(plan)?;
    let identity = target.auth.sole_identity()?;
    let revalidate =
        |identity: &storage_server::auth::Identity| -> std::result::Result<(), ApiError> {
            target.auth.revalidate_identity(identity)?;
            target
                .plan(plan)
                .map_err(|error| ApiError::io(std::io::Error::other(error)))
        };
    let mut material = None;
    let install = |engine: Engine| -> std::result::Result<(), ApiError> {
        let result = materialize(engine, target, plan);
        let success = result.is_ok();
        material = Some(result);
        if success {
            Ok(())
        } else {
            Err(ApiError::invalid(
                "first-storage materialization failed; retain original evidence",
            ))
        }
    };
    let activated = if started {
        target
            .setup
            .complete_activation_exact(&identity, &plan.activation(), revalidate, install)
    } else {
        target
            .setup
            .activate(&identity, plan.activation(), revalidate, install)
    };
    // Preserve the concrete materialization error, not only its API adapter.
    let report = match material {
        Some(result) => result?,
        None => {
            activated?;
            return Err("activation completed without required materialization callback".into());
        }
    };
    activated?;
    target.setup.activation_admitted()?;
    target.plan(plan)?;
    Ok(report)
}
fn materialize(mut engine: Engine, target: &Target, plan: &Plan) -> Result<Materialized> {
    target.plan(plan)?;
    if engine.is_read_only()
        || engine.state().sequence > 1
        || engine
            .state()
            .operations
            .values()
            .any(|operation| operation.kind != "auth.control-state")
    {
        return Err("first-storage completion refuses read-only or later-used storage; this is not a general repair/reset operation".into());
    }
    let expected = plan.roots();
    if engine.config().mode != Mode::Production
        || engine.config().disks.len() != expected.len()
        || engine
            .config()
            .disks
            .iter()
            .zip(&expected)
            .any(|(disk, path)| {
                disk.root != PathBuf::from(path)
                    || disk.level != 1
                    || disk.upstream_slot_id.is_some()
            })
    {
        return Err(
            "initialized topology differs from the original independent-primary batch".into(),
        );
    }
    let mut snapshot = target.auth.snapshot()?;
    snapshot["sessions"] = json!({});
    if engine.auth_state_ref() != Some(&snapshot) {
        if engine.auth_state_ref().is_some() || engine.state().sequence != 0 {
            return Err(
                "initial credential recovery snapshot differs; no later authority was overwritten"
                    .into(),
            );
        }
        engine.persist_auth_state(snapshot, "system_auth", OperationId::generate()?)?;
    }
    let slots: Vec<_> = engine
        .config()
        .disks
        .iter()
        .map(|disk| (disk.slot_id.clone(), disk.disk_id.clone()))
        .collect();
    let mut primary_disks = Vec::with_capacity(slots.len());
    for (slot_id, disk_id) in slots {
        let current = engine.rebuild_disk_catalog(&slot_id)?;
        let history = engine.rebuild_disk_history(&slot_id)?;
        let observed = engine.disk_catalog_observation(&slot_id)?;
        let observed_history = engine.disk_history_observation(&slot_id)?;
        if observed.verified_sequence != engine.state().sequence
            || observed_history.verified_sequence != engine.state().sequence
            || observed_history.snapshot.header.through_sequence != engine.state().sequence
        {
            return Err("first-storage database observations are not current; activation remains incomplete".into());
        }
        primary_disks.push(DiskMaterial {
            slot_id,
            disk_id,
            current_records_hash: current.records_hash,
            history_records_hash: history.records_hash,
        });
    }
    let metadata = engine.system_state_metadata()?;
    if metadata.sequence != engine.state().sequence
        || !matches!(engine.index_health(),IndexHealth::Ready{sequence} if sequence==engine.state().sequence)
    {
        return Err(
            "system SQLite projection is not current; first storage cannot be completed".into(),
        );
    }
    target.plan(plan)?;
    Ok(Materialized {
        pool_id: engine.config().pool_id.clone(),
        sequence: engine.state().sequence.to_string(),
        primary_disks,
        system_catalog_current: true,
        credential_recovery_snapshot_published: true,
    })
}
