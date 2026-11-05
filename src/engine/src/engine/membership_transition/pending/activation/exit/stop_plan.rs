//! The exact retained Exit authority owns a durable SYSTEM stop plan. Neither a
//! request digest, local locator, nor a saved plan can replace retained new-Q.
use super::*;
use crate::engine::disk_lifecycle::bootstrap::{
    IntentDigest, LoadedLocator, RootObservation,
    runtime::{management_error, observe, open_store},
    store::{BootstrapStore, PreparedStop},
};

const DIRECTORY: &str = "membership-exit-stops";
const HEAD: &str = "head.json";
const PLAN: &str = "plan.json";
const COMPLETE: &str = "complete.json";
const PLAN_KIND: &str = "membership-exit-stop-plan";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "ordinary-exit-stop-publication")]
    Exit,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct ExitStopPlan {
    format: Format,
    authority: PortableAuthority,
    stop: PreparedStop,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Head {
    format: Format,
    pool_id: String,
    operation_id: OperationId,
    authority_hash: String,
    plan_hash: String,
}
impl ExitStopPlan {
    fn head(&self) -> Result<Head> {
        if !self.authority.is_exit() {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "stop plan is not an ordinary Exit authority",
            ));
        }
        let intent = self.authority.intent()?;
        Ok(Head {
            format: Format::Exit,
            pool_id: intent.target.pool_id.clone(),
            operation_id: intent.operation_id,
            authority_hash: self.authority.hash()?,
            plan_hash: PreparedMetadataObject::new(PLAN_KIND, self, MAX_METADATA)?.hash,
        })
    }
}

/// Private construction follows independent retained certificates and complete
/// archives. This short-lived token owns its exact generation and stop identity.
pub(in crate::engine) struct VerifiedExitStopSource {
    generation: Arc<Generation>,
    identity: DiskRegistrationIdentity,
    operation_id: OperationId,
    intent_digest: IntentDigest,
    admission_epoch: Option<String>,
    observations: Vec<RootObservation>,
}
impl VerifiedExitStopSource {
    pub(in crate::engine) fn identity(&self) -> &DiskRegistrationIdentity {
        &self.identity
    }
    pub(in crate::engine) fn operation_id(&self) -> OperationId {
        self.operation_id
    }
    pub(in crate::engine) fn intent_digest(&self) -> IntentDigest {
        self.intent_digest
    }

    pub(in crate::engine) fn admission_epoch(&self) -> Option<&str> {
        self.admission_epoch.as_deref()
    }

    pub(in crate::engine) fn observations(&self) -> &[RootObservation] {
        &self.observations
    }

    fn from_verified_generation(
        generation: Arc<Generation>,
        observations: Vec<RootObservation>,
    ) -> Result<Self> {
        if !generation.portable.is_exit() || generation.hash != generation.portable.hash()? {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "stop source is not the exact Exit generation",
            ));
        }
        let intent = generation.portable.intent()?;
        let effect = intent
            .effect
            .as_ref()
            .ok_or_else(|| Error::new(ErrorCode::RecoveryRequired, "Exit source has no effect"))?;
        let identity = effect.identity(&intent.origin.config().pool_id);
        let operation_id = intent.operation_id;
        let transition = object_hash(INTENT_KIND, &intent)?;
        let admission_epoch = generation.portable.io_admission_epoch(&identity.disk_id)?;
        generation.portable.verify_bootstrap_stop_epoch(
            &identity,
            operation_id,
            &transition,
            admission_epoch.as_deref(),
        )?;
        Ok(Self {
            generation,
            identity,
            operation_id,
            intent_digest: IntentDigest::parse(&transition).map_err(management_error)?,
            admission_epoch,
            observations,
        })
    }

    fn from_pending(engine: &Engine) -> Result<Self> {
        let pending = engine.pending_disk_exit.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "stop requires its prepared Exit runtime",
            )
        })?;
        let observations = verify_retained_source(engine, &pending.generation)?;
        let source = Self::from_verified_generation(Arc::clone(&pending.generation), observations)?;
        let fence = engine.membership_fence.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "stop requires its original Exit fence",
            )
        })?;
        if fence.hash != source.intent_digest.to_string()
            || fence.intent.operation_id != source.operation_id
            || !authority_config_matches(&engine.config, fence.intent.origin.config())
            || engine.config.generation_hash() != fence.intent.origin.predecessor()
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "stop source differs from its original operation, actor or configuration",
            ));
        }
        Ok(source)
    }

    fn matches(
        &self,
        binding: &DiskRegistrationIdentity,
        operation: OperationId,
        intent: IntentDigest,
    ) -> Result<()> {
        if &self.identity != binding
            || self.operation_id != operation
            || self.intent_digest != intent
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "requested stop differs from its verified Exit source",
            ));
        }
        Ok(())
    }
}

fn verify_retained_source(
    engine: &Engine,
    generation: &Generation,
) -> Result<Vec<RootObservation>> {
    let portable = &generation.portable;
    let intent = portable.intent()?;
    let target = portable.effective_config()?;
    let transition = object_hash(INTENT_KIND, &intent)?;
    let joint = portable.add_joint()?;
    let verified = joint.verify()?;
    let forbidden = portable.forbidden_disk_ids()?;
    let mut observations = Vec::new();
    let mut rejected = None;
    for member in target.metadata_members() {
        if forbidden.contains(&member.disk_id) {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "Exit retains a forbidden metadata member",
            ));
        }
        let result = (|| {
            let disk = engine
                .config
                .disks
                .iter()
                .find(|disk| disk.disk_id == member.disk_id)
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::RecoveryRequired,
                        "retained Exit member is absent from held configuration",
                    )
                })?;
            let root = &engine.disk_resources.get(disk)?.root;
            let physical = admission::verify_named_root(root)?;
            engine._locks.verify_disk(disk, root)?;
            let dependency = verified.dependencies.get(&disk.disk_id).ok_or_else(|| {
                Error::new(
                    ErrorCode::RecoveryRequired,
                    "retained Exit member has no joint dependency",
                )
            })?;
            if dependency.enrollment.identity
                != admission::member_identity(&target, member, physical)
            {
                return Err(Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "retained Exit member differs from its certificate",
                ));
            }
            let saved: PortableAuthority = decode_strict(&root.read_bytes(
                &runtime::prepared_authority_path(disk, &transition),
                MAX_METADATA,
            )?)?;
            if saved.hash()? != generation.hash {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "retained prepared Exit authority differs",
                ));
            }
            let parent = root.private_child(&member_directory(disk, &transition))?;
            let local = joint::store::JointStore::open(
                root,
                &parent,
                &dependency.enrollment.identity,
                &transition,
                &joint.proposal_hash,
                false,
            )?;
            local.verify_certificate_read_only(
                &joint.certificate_hash,
                &joint.proposal_hash,
                disk,
            )?;
            let (state, _) = portable.read_archive_base(
                parent.private_child(&parent.path().join(CONTROL_ARCHIVE_DIRECTORY))?,
            )?;
            portable.verify_registry_source(&state)?;
            RootObservation::capture(
                &DiskRegistrationIdentity {
                    pool_id: target.pool_id.clone(),
                    disk_id: disk.disk_id.clone(),
                    slot_id: disk.slot_id.clone(),
                    binding_generation: disk.binding_generation,
                },
                &disk.root,
                &root.directory(root.path())?,
                member.metadata_member,
            )
            .map_err(management_error)
        })();
        match result {
            Ok(observation) => observations.push(observation),
            Err(error) => rejected = Some(error),
        }
    }
    if observations.len() < target.metadata_quorum() {
        let detail = "Exit stop lacks independently authenticated fixed retained quorum and complete archives";
        return Err(match rejected {
            Some(error) => error.context(ErrorCode::MetadataQuorumUnavailable, detail),
            None => Error::new(ErrorCode::MetadataQuorumUnavailable, detail),
        });
    }
    Ok(observations)
}

struct PlanStore {
    system: DiskRoot,
    directory: DiskRoot,
    operation: DiskRoot,
    head: Head,
}
impl PlanStore {
    fn open(root: &Path) -> Result<Option<(Self, ExitStopPlan)>> {
        let system = DiskRoot::open(root)?;
        let path = system.path().join(DIRECTORY);
        if system.optional_metadata(&path)?.is_none() {
            return Ok(None);
        }
        let directory = system.private_child(&path)?;
        inventory(&directory, true)?;
        let Some(head) = optional::<Head>(&directory, HEAD)? else {
            return Ok(None);
        };
        let operation =
            directory.private_child(&directory.path().join(head.operation_id.to_string()))?;
        inventory(&operation, false)?;
        let plan: ExitStopPlan =
            decode_strict(&operation.read_bytes(&operation.path().join(PLAN), MAX_METADATA)?)?;
        if plan.head()? != head {
            return Err(Error::new(
                ErrorCode::LogFork,
                "Exit stop plan differs from its protected head",
            ));
        }
        Ok(Some((
            Self {
                system,
                directory,
                operation,
                head,
            },
            plan,
        )))
    }
    fn prepare(engine: &Engine, plan: &ExitStopPlan) -> Result<Self> {
        let system = DiskRoot::open(&engine.root)?;
        verify_writer(&system, engine)?;
        system.private_dir(&system.path().join(DIRECTORY))?;
        let directory = system.private_child(&system.path().join(DIRECTORY))?;
        inventory(&directory, true)?;
        let head = plan.head()?;
        let path = directory.path().join(head.operation_id.to_string());
        directory.private_dir(&path)?;
        let operation = directory.private_child(&path)?;
        inventory(&operation, false)?;
        operation.write_json(&operation.path().join(PLAN), plan, false)?;
        directory.write_json(&directory.path().join(HEAD), &head, true)?;
        Ok(Self {
            system,
            directory,
            operation,
            head,
        })
    }
    fn verify_current(&self, engine: &Engine) -> Result<()> {
        verify_writer(&self.system, engine)?;
        inventory(&self.directory, true)?;
        inventory(&self.operation, false)?;
        let current: Head = decode_strict(
            &self
                .directory
                .read_bytes(&self.directory.path().join(HEAD), MAX_METADATA)?,
        )?;
        let plan: ExitStopPlan = decode_strict(
            &self
                .operation
                .read_bytes(&self.operation.path().join(PLAN), MAX_METADATA)?,
        )?;
        if current != self.head || plan.head()? != self.head {
            return Err(Error::new(
                ErrorCode::LogFork,
                "Exit stop plan changed before publication",
            ));
        }
        // Existing exact bytes still require today's file and directory barriers.
        self.operation
            .write_json(&self.operation.path().join(PLAN), &plan, false)?;
        self.directory
            .write_json(&self.directory.path().join(HEAD), &self.head, false)
    }
    fn completed(&self) -> Result<bool> {
        match optional::<Head>(&self.operation, COMPLETE)? {
            None => Ok(false),
            Some(head) if head == self.head => Ok(true),
            Some(_) => Err(Error::new(
                ErrorCode::LogFork,
                "Exit stop completion differs from its exact plan",
            )),
        }
    }
    fn complete(&self) -> Result<()> {
        self.operation
            .write_json(&self.operation.path().join(COMPLETE), &self.head, false)
    }
}
fn verify_writer(system: &DiskRoot, engine: &Engine) -> Result<()> {
    let named = system.open_regular(&system.path().join("writer.lock"))?;
    if FileIdentity::from_fd(&named)? != engine._locks.system_identity()? {
        return Err(Error::new(
            ErrorCode::DiskIdentityMismatch,
            "Exit stop lost the protected system writer",
        ));
    }
    Ok(())
}
fn optional<T: for<'de> Deserialize<'de>>(root: &DiskRoot, name: &str) -> Result<Option<T>> {
    root.open_optional_read_regular(&root.path().join(name))?
        .map(|file| Ok(decode_strict(&read_file_bytes(file, MAX_METADATA)?)?))
        .transpose()
}
fn inventory(root: &DiskRoot, top: bool) -> Result<()> {
    for entry in root.entries(root.path())? {
        let entry = entry.map_err(crate::disk::fs_error)?;
        let name = entry.as_os_str().to_str().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "non-UTF-8 Exit stop control entry",
            )
        })?;
        let permitted = if top {
            name == HEAD || name.parse::<OperationId>().is_ok()
        } else {
            matches!(name, PLAN | COMPLETE)
        };
        if !permitted {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "unknown or unfinished Exit stop control write",
            ));
        }
    }
    Ok(())
}

pub(in crate::engine) fn synchronize(
    engine: &Engine,
    store: &mut BootstrapStore,
    binding: &DiskRegistrationIdentity,
    operation: OperationId,
    digest: IntentDigest,
    append: bool,
) -> Result<Option<LoadedLocator>> {
    let saved = PlanStore::open(&engine.root)?;
    let prepared = match saved {
        Some((saved, plan)) if saved.head.operation_id == operation && !saved.completed()? => {
            Some((saved, plan))
        }
        Some((saved, _)) if !saved.completed()? => {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "another Exit stop plan is unfinished",
            ));
        }
        _ => None,
    };
    if prepared.is_none() && !append {
        return Ok(None);
    }
    if prepared.is_none() {
        let locator = store
            .reload(&engine.config.pool_id)
            .map_err(management_error)?;
        let admission_epoch = engine.config.io_admission_epoch(&binding.disk_id)?;
        if locator.stop_hints().any(|stop| {
            stop.disk_id == binding.disk_id
                && stop.slot_id == binding.slot_id
                && stop.binding_generation == binding.binding_generation
                && stop.operation_id == operation
                && stop.intent_digest == digest
                && stop.admission_epoch.map(|epoch| epoch.to_string()) == admission_epoch
        }) {
            return Ok(None);
        }
    }
    let source = VerifiedExitStopSource::from_pending(engine)?;
    source.matches(binding, operation, digest)?;
    let (saved, plan) = match prepared {
        Some((saved, plan)) => {
            if saved.head.authority_hash != source.generation.hash {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "same Exit operation has another stop authority",
                ));
            }
            (saved, plan)
        }
        None => {
            let previous = store.reload(&binding.pool_id).map_err(management_error)?;
            let observations = observe(&engine.config, engine.disk_resources.handles())?;
            let stop = store
                .prepare_exit_stop(&previous, &source, observations)
                .map_err(management_error)?;
            let plan = ExitStopPlan {
                format: Format::Exit,
                authority: source.generation.portable.clone(),
                stop,
            };
            let saved = PlanStore::prepare(engine, &plan)?;
            (saved, plan)
        }
    };
    let admitted = store
        .admit_exit_stop(&plan.stop, &source)
        .map_err(management_error)?;
    saved.verify_current(engine)?;
    let locator = match store.publish_stop(admitted).map_err(management_error) {
        Ok(locator) => locator,
        Err(error) => {
            // Only this path has a verified source and durably synchronized
            // exact plan. Preparation failures never close an unrelated gate.
            quiesce_prepared_target(engine, &source, &error)?;
            return Err(error);
        }
    };
    if let Err(error) = saved.complete() {
        quiesce_prepared_target(engine, &source, &error)?;
        return Err(error);
    }
    Ok(Some(locator))
}

fn quiesce_prepared_target(
    engine: &Engine,
    source: &VerifiedExitStopSource,
    publication_error: &Error,
) -> Result<()> {
    let pending = engine
        .pending_disk_exit
        .as_ref()
        .expect("invariant: verified pending Exit source remains owned");
    match pending
        .ownership
        .get(&source.identity.disk_id)
        .expect("invariant: prepared Exit owns its target")
    {
        runtime::ExitIoOwner::Live(binding) => {
            drop(binding.begin_quiesce().map_err(|error| {
                error.context(
                    ErrorCode::RecoveryRequired,
                    format!(
                        "cannot quiesce after uncertain exact stop publication: {publication_error}"
                    ),
                )
            })?);
        }
        runtime::ExitIoOwner::Unopened | runtime::ExitIoOwner::Recovered(_) => (),
    }
    Ok(())
}

/// Ordinary startup must reject even the pre-record window: an exact durable
/// stop plan may already have closed the live gate while the old ledger is valid.
pub(in crate::engine) fn require_no_pending_stop(
    root: &Path,
    store: &BootstrapStore,
) -> Result<()> {
    let Some((saved, plan)) = PlanStore::open(root)? else {
        return Ok(());
    };
    if !saved.completed()? {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "a prepared Exit stop requires exact activation recovery before storage discovery",
        ));
    }
    let locator = store
        .reload(&saved.head.pool_id)
        .map_err(management_error)?;
    let intent = plan.authority.intent()?;
    let effect = intent
        .effect
        .as_ref()
        .expect("invariant: Exit stop plan authority");
    let transition = object_hash(INTENT_KIND, &intent)?;
    let admission_epoch = plan.authority.io_admission_epoch(&effect.disk_id)?;
    if !locator.stop_hints().any(|stop| {
        stop.disk_id == effect.disk_id
            && stop.slot_id == effect.slot_id
            && stop.binding_generation == effect.binding_generation
            && stop.operation_id == intent.operation_id
            && stop.intent_digest.to_string() == transition
            && stop.admission_epoch.map(|epoch| epoch.to_string()) == admission_epoch
    }) {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "completed Exit stop plan lacks its exact durable ledger restriction",
        ));
    }
    store.stop_locator(&plan.stop).map_err(management_error)?;
    Ok(())
}

pub(in crate::engine::membership_transition::pending::activation) struct PreparedResume {
    config: ValidatedConfig,
    bootstrap: BootstrapStore,
    saved: PlanStore,
    plan: ExitStopPlan,
}
impl PreparedResume {
    pub(in crate::engine::membership_transition::pending::activation) fn config(
        &self,
    ) -> ValidatedConfig {
        self.config.clone()
    }
    pub(in crate::engine::membership_transition::pending::activation) fn publish(
        mut self,
        engine: &Engine,
    ) -> Result<()> {
        self.saved.verify_current(engine)?;
        let generation = engine.config.generation().cloned().ok_or_else(|| {
            Error::new(ErrorCode::RecoveryRequired, "resumed Exit has no authority")
        })?;
        if generation.hash != self.saved.head.authority_hash {
            return Err(Error::new(
                ErrorCode::LogFork,
                "resumed resources differ from the Exit stop authority",
            ));
        }
        // Recheck the fixed retained quorum and complete local archives after
        // acquiring the actual system/disk writers, using only retained handles.
        let observations = verify_retained_source(engine, &generation)?;
        let source = VerifiedExitStopSource::from_verified_generation(generation, observations)?;
        let admitted = self
            .bootstrap
            .admit_exit_stop(&self.plan.stop, &source)
            .map_err(management_error)?;
        self.bootstrap
            .publish_stop(admitted)
            .map_err(management_error)?;
        self.saved.complete()
    }
}
pub(in crate::engine::membership_transition::pending::activation) fn prepare_resume(
    root: &Path,
    projected: &ValidatedConfig,
) -> Result<Option<PreparedResume>> {
    let bootstrap = open_store(root)?;
    let Some((saved, plan)) = PlanStore::open(root)? else {
        return Ok(None);
    };
    if saved.completed()? {
        return Ok(None);
    }
    if saved.head.pool_id != projected.pool_id {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "Exit stop plan belongs to another pool",
        ));
    }
    let config =
        recovery::verify_prepared_exit_stop(&bootstrap, &plan.stop, &plan.authority, projected)?;
    Ok(Some(PreparedResume {
        config,
        bootstrap,
        saved,
        plan,
    }))
}

#[cfg(test)]
#[path = "../../../../../../../../test/engine/unit/engine/exit_stop_publication_tests.rs"]
mod tests;
