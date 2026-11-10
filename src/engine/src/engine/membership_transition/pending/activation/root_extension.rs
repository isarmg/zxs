//! Explicit ordinary-Add recovery for an interrupted bootstrap publication.
//! SYSTEM retains the exact authority and immutable publication plan before the
//! first locator record. Neither that plan nor a locator vote replaces the real
//! joint certificates, fixed new quorum, complete archives or held identities.
use super::plan_store::Plan;
use super::*;
type PlanStore = super::plan_store::PlanStore<AddPlan>;
use crate::engine::disk_lifecycle::bootstrap::{
    RootObservation,
    runtime::{management_error, observe, open_store},
    store::{BootstrapStore, PreparedRootExtension},
};

const DIRECTORY: &str = "membership-root-extensions";
const PLAN_KIND: &str = "membership-add-root-extension";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "ordinary-add-root-extension")]
    Add,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct AddPlan {
    format: Format,
    authority: PortableAuthority,
    extension: PreparedRootExtension,
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

impl Plan for AddPlan {
    type Head = Head;
    const DIRECTORY: &'static str = DIRECTORY;
    const LABEL: &'static str = "Add extension";

    fn operation_id(head: &Head) -> OperationId {
        head.operation_id
    }

    fn head(&self) -> Result<Head> {
        // This explicitly excludes Abort, Exit and future readmission authority.
        // Merely having a generation or an old locator is not Add permission.
        self.authority.add_joint()?.verify()?;
        let intent = self.authority.intent()?;
        if self.authority.is_abort() || self.authority.is_exit() {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "bootstrap Add plan cannot authorize another lifecycle operation",
            ));
        }
        Ok(Head {
            format: Format::Add,
            pool_id: intent.target.pool_id.clone(),
            operation_id: intent.operation_id,
            authority_hash: self.authority.hash()?,
            plan_hash: object_hash(PLAN_KIND, self)?,
        })
    }
}

/// Called only with a verified pending source/Joint generation and all of its
/// live resources. This is the ordinary Add writer, not an offline reenable API.
pub(in crate::engine) fn synchronize(
    engine: &Engine,
    config: &ValidatedConfig,
    observations: Vec<RootObservation>,
    store: &mut BootstrapStore,
) -> Result<()> {
    let generation = config.generation().ok_or_else(|| {
        Error::new(
            ErrorCode::RecoveryRequired,
            "Add root extension requires its verified generation",
        )
    })?;
    let intent = generation.portable.intent()?;
    generation.portable.add_joint()?.verify()?;
    let fence = engine.membership_fence.as_ref().ok_or_else(|| {
        Error::new(
            ErrorCode::RecoveryRequired,
            "Add root extension requires its owning pending fence",
        )
    })?;
    if generation.portable.is_abort()
        || generation.portable.is_exit()
        || fence.hash != object_hash(INTENT_KIND, &intent)?
        || engine.config.generation_hash() != intent.origin.predecessor()
        || !authority_config_matches(&engine.config, intent.origin.config())
        || !authority_config_matches(config, &intent.target)
    {
        return Err(Error::new(
            ErrorCode::LogFork,
            "Add root extension differs from its exact source and target",
        ));
    }
    let prepared = match PlanStore::open(&engine.root)? {
        Some((saved, plan)) if saved.head.operation_id == intent.operation_id => {
            if saved.head.authority_hash != generation.hash {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "same Add operation has another extension authority",
                ));
            }
            saved.verify_current(engine)?;
            Some((saved, plan))
        }
        Some((saved, _)) if !saved.completed()? => {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "another Add extension still needs explicit activation recovery",
            ));
        }
        _ => None,
    };
    let (saved, plan) = match prepared {
        Some(prepared) => prepared,
        None => {
            let previous = store.reload(&config.pool_id).map_err(management_error)?;
            let Some(extension) = store
                .prepare_root_extension(&previous, observations.clone())
                .map_err(management_error)?
            else {
                return store.sync_current(&previous).map_err(management_error);
            };
            let plan = AddPlan {
                format: Format::Add,
                authority: generation.portable.clone(),
                extension,
            };
            let saved = PlanStore::prepare(engine, &plan)?;
            (saved, plan)
        }
    };
    saved.verify_current(engine)?;
    store
        .root_extension_locator(&plan.extension)
        .map_err(management_error)?;
    store
        .resume_root_extension(&plan.extension, observations)
        .map(|_| ())
        .map_err(management_error)
}

pub(super) struct PreparedResume {
    config: ValidatedConfig,
    bootstrap: BootstrapStore,
    saved: PlanStore,
    plan: AddPlan,
}

impl PreparedResume {
    pub(super) fn config(&self) -> ValidatedConfig {
        self.config.clone()
    }

    pub(super) fn publish(mut self, engine: &Engine) -> Result<()> {
        self.saved.verify_current(engine)?;
        if engine.config.generation_hash() != Some(self.saved.head.authority_hash.as_str()) {
            return Err(Error::new(
                ErrorCode::LogFork,
                "resumed Add resources belong to another authority",
            ));
        }
        self.bootstrap
            .root_extension_locator(&self.plan.extension)
            .map_err(management_error)?;
        let observations = observe(&engine.config, engine.disk_resources.handles())?;
        self.bootstrap
            .resume_root_extension(&self.plan.extension, observations)
            .map(|_| ())
            .map_err(management_error)
    }
}

/// Only the explicit activation entry calls this. Missing plans never relax
/// ordinary discovery, scan dangling records, follow Config paths or clear stops.
pub(super) fn prepare_resume(
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
            "Add extension belongs to another pool",
        ));
    }
    // This validates exact old/new head, immutable chain and inventory before
    // the verifier is allowed to stat even one storage directory.
    let locator = bootstrap
        .root_extension_locator(&plan.extension)
        .map_err(management_error)?;
    let admissions = bootstrap.readmission_probe_admissions(&locator)?;
    let config =
        recovery::verify_prepared_add(&locator, plan.authority.clone(), projected, &admissions)?;
    Ok(Some(PreparedResume {
        config,
        bootstrap,
        saved,
        plan,
    }))
}

/// A pending plan may restrict startup but never select an authority. A marker
/// written first on a data-only root must not let metadata-only discovery return
/// the old fenced generation as though no activation had begun.
pub(super) fn require_selected_activation(root: &Path, config: &ValidatedConfig) -> Result<()> {
    let Some((saved, _)) = PlanStore::open(root)? else {
        return Ok(());
    };
    if saved.completed()? {
        return Ok(());
    }
    let selected = config
        .generation()
        .map(|generation| generation.portable.contains(&saved.head.authority_hash))
        .transpose()?
        .unwrap_or(false);
    if !selected {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "unfinished Add activation requires explicit membership resume",
        ));
    }
    Ok(())
}

/// Persist completion only after this exact Add's activation result and SYSTEM
/// projection are durable. Historical plans are retained and never repurposed.
pub(super) fn complete(engine: &Engine) -> Result<()> {
    let Some((saved, _)) = PlanStore::open(&engine.root)? else {
        return Ok(());
    };
    let Some(generation) = engine.config.generation() else {
        return Ok(());
    };
    if saved.head.authority_hash != generation.hash {
        return Ok(());
    }
    let expected = generation.result()?;
    if saved.head.operation_id != expected.operation_id
        || engine.state.operations.get(&expected.operation_id) != Some(&expected)
    {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "Add extension completion lacks its exact published operation",
        ));
    }
    saved.verify_current(engine)?;
    saved.complete()
}
