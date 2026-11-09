//! Complete-chain registration proposal, never an active registration or a
//! transformed State. It is derived again from each original database image.
use super::*;
use storage_engine::{Config, DiskConfig, ForeignOriginalStateReport, ValidatedConfig};

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Target {
    root_path: String,
    physical: Physical,
    registration: DiskRegistrationIdentity,
    level: u32,
    upstream_slot_id: Option<String>,
    metadata_member: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Proposal {
    request_digest: String,
    database_digest: String,
    original: ForeignOriginalStateReport,
    source_registration: DiskRegistrationIdentity,
    targets: Vec<Target>,
    proposed_configuration_digest: String,
}
impl Proposal {
    pub(super) fn configuration(&self, current: &Config) -> Result<Config> {
        let mut proposed = current.clone();
        for item in &self.targets {
            proposed.disks.push(DiskConfig {
                slot_id: item.registration.slot_id.clone(),
                disk_id: item.registration.disk_id.clone(),
                root: item.root_path.clone().into(),
                level: item.level,
                upstream_slot_id: item.upstream_slot_id.clone(),
                metadata_member: item.metadata_member,
                binding_generation: item.registration.binding_generation,
                capacity_bytes: None,
            });
        }
        if digest(&proposed)? != self.proposed_configuration_digest {
            return Err("identity proposal no longer preserves its original current Config".into());
        }
        Ok(proposed)
    }
    pub(super) fn capture(
        configuration: &Config,
        plan: &Plan,
        observation: &databases::Observation,
    ) -> Result<Self> {
        plan.validate()?;
        if digest(configuration)? != plan.binding.configuration_digest {
            return Err(
                "target identity mapping requires the original current-pool configuration".into(),
            );
        }
        if observation.targets.len() != plan.binding.lower.len() || !observation.completed() {
            return Err("all original three-scope databases must be independently verified before identity mapping".into());
        }
        let original = &observation.targets[0].original;
        if original.pool_id == configuration.pool_id
            || !original
                .registrations
                .contains(&plan.binding.source.registration)
        {
            return Err("original State does not bind the selected foreign source identity".into());
        }
        let request_digest = digest(plan)?;
        let mut targets = Vec::with_capacity(plan.binding.lower.len() + 1);
        let mut proposed = configuration.clone();
        for item in &plan.binding.lower {
            targets.push(append_target(
                &mut proposed,
                &request_digest,
                &original.registrations,
                &item.directory,
                Physical {
                    device: item.selection.root_identity.device.to_string(),
                    inode: item.selection.root_identity.inode.to_string(),
                },
            )?);
        }
        let source = LowerDirectory {
            root_path: plan.binding.source.root_path.clone(),
            slot_id: plan.binding.source_slot_id.clone(),
            level: plan.binding.source.level,
            upstream_slot_id: Some(plan.binding.source_upstream_slot_id.clone()),
            metadata_member: plan.binding.source_metadata_member,
        };
        targets.push(append_target(
            &mut proposed,
            &request_digest,
            &original.registrations,
            &source,
            plan.binding.source.physical.clone(),
        )?);
        ValidatedConfig::try_from(proposed.clone())?;
        Ok(Self {
            request_digest,
            database_digest: digest(&observation.expected(plan)?)?,
            original: original.clone(),
            source_registration: plan.binding.source.registration.clone(),
            targets,
            proposed_configuration_digest: digest(&proposed)?,
        })
    }
}
fn append_target(
    proposed: &mut Config,
    request_digest: &str,
    original: &[DiskRegistrationIdentity],
    item: &LowerDirectory,
    physical: Physical,
) -> Result<Target> {
    // Deliberately distinct from ordinary Add's identity derivation. An
    // existing foreign seal cannot become a new pool binding by renaming it.
    let id = digest(&(
        "zxs:foreign-chain-target-identity",
        request_digest,
        &proposed.pool_id,
        item.level,
        &item.slot_id,
        &physical,
    ))?;
    let disk_id = format!("disk_{}", &id[..32]);
    if proposed
        .disks
        .iter()
        .any(|disk| disk.disk_id == disk_id || disk.slot_id.eq_ignore_ascii_case(&item.slot_id))
        || original.iter().any(|disk| disk.disk_id == disk_id)
    {
        return Err(
            "new chain identity or slot collides with current/historical/original registration"
                .into(),
        );
    }
    let registration = DiskRegistrationIdentity {
        pool_id: proposed.pool_id.clone(),
        disk_id,
        slot_id: item.slot_id.clone(),
        binding_generation: 1,
    };
    proposed.disks.push(DiskConfig {
        slot_id: registration.slot_id.clone(),
        disk_id: registration.disk_id.clone(),
        root: item.root_path.clone().into(),
        level: item.level,
        upstream_slot_id: item.upstream_slot_id.clone(),
        metadata_member: item.metadata_member,
        binding_generation: registration.binding_generation,
        capacity_bytes: None,
    });
    Ok(Target {
        root_path: item.root_path.clone(),
        physical,
        registration,
        level: item.level,
        upstream_slot_id: item.upstream_slot_id.clone(),
        metadata_member: item.metadata_member,
    })
}
fn response(proposal: Proposal) -> Result<json_codec::Value> {
    Ok(
        json_codec::json!({"identity_digest":digest(&proposal)?,"proposal":proposal,
        "original_database_scopes_prepared":true,"target_identity_proposal_only":true,
        "target_identity_databases_installed":false,"foreign_admitted":false,
        "changes_existing_pool_membership":false,"source_modified":false,
        "starts_service":false,"complete_installation_ready":false}),
    )
}
pub(crate) fn preview(input: Original) -> Result<json_codec::Value> {
    let (root, plan, expected) = databases::completed_original(&input)?;
    let envelope = WorkerRequest {
        context: root.context.clone(),
        action: Action::Identity {
            plan: plan.clone(),
            expected,
        },
    };
    drop(root);
    let Output::Identity { proposal } = crate::offline::service_worker::run(envelope)? else {
        return Err("unexpected foreign identity mapping response".into());
    };
    Root::open(input.service_user)?.check(&plan)?;
    response(proposal)
}

#[cfg(test)]
#[path = "../../../../../test/server/unit/offline/foreign_chain_identity_tests.rs"]
mod tests;
