//! Hash-linked Add, Abort and prescribed Exit generations. Authority is portable; activation
//! additionally requires held roots, local control archives and actual journals.
use super::*;
use crate::engine::membership_transition::archive::persistent::{
    ArchiveRoot, install_generation_archive, read_generation_base,
};
use crate::persist::layout;
use joint::store::PortableJoint;
use std::collections::BTreeSet;

pub(in crate::engine::membership_transition) const AUTHORITY_KIND: &str = "membership-generation";
const GENERATION_FILE: &str = "metadata/pool-membership/generation.json";
type WorkspaceRootAliases = BTreeMap<(String, String, u64), BTreeSet<PathBuf>>;

mod authority;
pub(in crate::engine) mod backup;
mod foreign;
mod plan_store;
pub(in crate::engine) use backup::{BackupWriterIoConfig, PreparedBackupTargetLocator};
pub(in crate::engine) mod exit;
#[path = "../readmission/activation.rs"]
mod readmission;
mod registry;
pub(in crate::engine::membership_transition) use authority::AuthorityResolver;
pub(crate) use authority::PortableAuthority;
pub(in crate::engine::membership_transition) use authority::same_config_projection as authority_config_matches;
#[derive(Clone, Debug)]
pub(crate) struct Generation {
    pub(crate) hash: String,
    pub(crate) portable: PortableAuthority,
    intent: TransitionIntent,
    original: ValidatedConfig,
    history: BTreeMap<u64, String>,
    epoch: u64,
    workspace_roots: WorkspaceRootAliases,
    pub(crate) base: Arc<State>,
    backup_continuation: Option<backup::BackupContinuation>,
}
impl Generation {
    pub(crate) fn historical_workspace_roots(
        &self,
        disk: &DiskConfig,
    ) -> Option<&BTreeSet<PathBuf>> {
        self.workspace_roots.get(&(
            disk.disk_id.clone(),
            disk.slot_id.clone(),
            disk.binding_generation,
        ))
    }
    pub(crate) fn membership_epoch(&self) -> u64 {
        self.epoch
    }
    pub(crate) fn verify_candidate(&self, candidate: &Candidate) -> Result<()> {
        match candidate.generation() {
            Some(hash) if hash == self.hash && candidate.sequence > self.intent.fence_sequence => {
                if self.portable.is_backup() && candidate.sequence > self.first_sequence()? {
                    return self.backup_continuation.as_ref()
                        .ok_or_else(crate::engine::membership_transition::backup::BackupCore::activation_required)?
                        .verify_later(candidate);
                }
                self.portable.verify_candidate(candidate).map(|_| ())
            }
            _ if self.history.get(&candidate.sequence)
                == Some(&object_hash("candidate", candidate)?) =>
            {
                self.portable.verify_ancestor_candidate(candidate)
            }
            _ => Err(Error::new(
                ErrorCode::PublicationInvalid,
                "candidate is outside authenticated generation or exact frozen history",
            )),
        }
    }
    pub(crate) fn snapshot(&self) -> Option<String> {
        self.intent
            .boundary
            .as_ref()
            .map(|boundary| boundary.snapshot_hash.clone())
    }
    pub(crate) fn first_sequence(&self) -> Result<u64> {
        if self.portable.is_foreign() {
            return self.intent.first_sequence();
        }
        self.intent
            .fence_sequence
            .checked_add(1)
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimit, "activation sequence exhausted"))
    }
    pub(crate) fn transaction_id(&self, operation: OperationId) -> Option<String> {
        (operation == self.intent.operation_id).then(|| format!("tx_{}", &self.hash[..32]))
    }
    fn result(&self) -> Result<OperationResult> {
        if let Some(core) = self.portable.backup_core() {
            return self.backup_first_state(core).map(|(_, result)| result);
        }
        let effective = self.portable.effective_config()?;
        let topology_steps = if self.portable.terminal_resume_core().is_some() {
            2
        } else {
            1
        };
        let mut outcome = json_codec::json!({"generation": self.hash, "membership_epoch": self.membership_epoch().to_string(), "metadata_members": effective.metadata_members().len(), "topology_generation": self.base.topology_generation.checked_add(topology_steps).ok_or_else(|| Error::new(ErrorCode::ResourceLimit, "topology generation exhausted"))?.to_string()});
        if let Some(core) = self.portable.terminal_resume_core() {
            outcome["terminal_authority"] = core.terminal().hash()?.into();
            if core.retained_request().is_some() {
                outcome["authority_kind"] = "terminal-metadata-resume".into();
            }
        }
        if let Some(effect) = &self.intent.effect {
            outcome["disk_id"] = effect.disk_id.clone().into();
            outcome["slot_id"] = effect.slot_id.clone().into();
            outcome["binding_generation"] = effect.binding_generation.to_string().into();
            outcome["registration"] = match effect.action {
                MembershipExitAction::PlannedOffline => "disabled",
                MembershipExitAction::Deregister => "detached",
            }
            .into();
            outcome["target_io"] = "closed".into();
        }
        if self.portable.is_foreign() {
            let recipe = self
                .intent
                .foreign
                .as_ref()
                .expect("invariant: Foreign core requires recipe");
            outcome["foreign_recipe"] = recipe.digest()?.into();
            outcome["foreign_mapping"] = recipe.identities().mapping_digest.clone().into();
            outcome["original_state"] = recipe.identities().original.state_hash.clone().into();
            outcome["original_pool"] = recipe.identities().original.pool_id.clone().into();
        }
        let readmission = match &self.intent.origin {
            Origin::Readmission { request, .. } => Some(request.as_ref()),
            _ => self
                .portable
                .terminal_resume_core()
                .and_then(|core| core.retained_request()),
        };
        if let Some(request) = readmission {
            outcome["disk_id"] = request.target.disk_id.clone().into();
            outcome["slot_id"] = request.target.slot_id.clone().into();
            outcome["binding_generation"] = request.target.binding_generation.to_string().into();
            outcome["registration"] = "enabled".into();
            outcome["io_admission_epoch"] = object_hash(INTENT_KIND, &self.intent)?.into();
        }
        Ok(OperationResult {
            operation_id: self.intent.operation_id,
            kind: match self.intent.effect.as_ref().map(|effect| effect.action) {
                Some(MembershipExitAction::PlannedOffline) => "disk-planned-offline",
                Some(MembershipExitAction::Deregister) => "disk-deregister",
                None if readmission.is_some() => "disk-readmit",
                None if self.portable.is_abort()
                    && matches!(&self.intent.origin, Origin::BackupCutover { .. }) =>
                {
                    "backup-cutover-cancelled"
                }
                None if self.portable.is_abort() && self.intent.foreign.is_some() => {
                    "foreign-membership-cancelled"
                }
                None if self.portable.is_abort() => "membership-add-cancelled",
                None if self.portable.is_foreign() => "foreign-membership-admission",
                None if self.portable.terminal_resume_core().is_some() => {
                    "terminal-metadata-resume"
                }
                None => "membership-add",
            }
            .into(),
            commit_id: None,
            history_tree_id: None,
            branch_id: None,
            commit_state: storage_contract::CommitState::Published,
            protection_at_publish: storage_contract::ProtectionAtPublish::NotApplicable,
            current_health: if self.portable.terminal_resume_core().is_some() {
                storage_contract::CurrentHealth::Unknown
            } else {
                storage_contract::CurrentHealth::Healthy
            },
            request_digest: object_hash(INTENT_KIND, &self.intent)?,
            sequence: self.first_sequence()?,
            result: outcome,
        })
    }
    fn next_state(&self) -> Result<(State, OperationResult)> {
        if let Some(core) = self.portable.backup_core() {
            return self.backup_first_state(core);
        }
        if self.portable.is_foreign() {
            return self.foreign_first_state();
        }
        let mut next = match self.portable.terminal_resume_core() {
            Some(core) => (*core.management_state(&self.base)?).clone(),
            None => (*self.base).clone(),
        };
        let result = self.result()?;
        next.sequence = result.sequence;
        next.topology_generation = next
            .topology_generation
            .checked_add(1)
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimit, "topology generation exhausted"))?;
        next.operations.insert(result.operation_id, result.clone());
        next.operation_principals
            .insert(result.operation_id, self.intent.principal.clone());
        Ok((next, result))
    }
    pub(in crate::engine) fn historical_candidates(&self) -> impl Iterator<Item = &str> {
        self.history.values().map(String::as_str)
    }
    pub(crate) fn historical_candidate(&self, sequence: u64) -> Option<&str> {
        self.history.get(&sequence).map(String::as_str)
    }
    pub(in crate::engine) fn verify_archive_observed(
        &self,
        root: &DiskRoot,
        disk: &DiskConfig,
        observe: &mut impl FnMut(usize) -> Result<()>,
        maximum_objects: usize,
    ) -> Result<MembershipArchiveInspection> {
        let transition = object_hash(INTENT_KIND, &self.intent)?;
        super::super::archive::persistent::verify_generation_archive_observed(
            root.private_child(
                &member_directory(disk, &transition).join(CONTROL_ARCHIVE_DIRECTORY),
            )?,
            &self.portable.archive_hash()?,
            &self.portable.archive_transition()?,
            observe,
            maximum_objects,
        )
    }
    pub(in crate::engine) fn visit_native_archive(
        &self,
        root: &DiskRoot,
        disk: &DiskConfig,
        checkpoint: &impl Fn() -> Result<()>,
        visit: impl FnMut(
            &Candidate,
            &str,
            &Publication,
            &str,
            &crate::history_package::AdmittedState,
        ) -> Result<()>,
    ) -> Result<()> {
        let transition = object_hash(INTENT_KIND, &self.intent)?;
        super::super::archive::persistent::native_replay::visit(
            root.private_child(
                &member_directory(disk, &transition).join(CONTROL_ARCHIVE_DIRECTORY),
            )?,
            &self.portable.archive_hash()?,
            &self.portable.archive_transition()?,
            checkpoint,
            visit,
        )
    }

    pub(crate) fn original(&self) -> &ValidatedConfig {
        &self.original
    }
}
pub(in crate::engine::membership_transition) fn verify_frozen_activation(
    portable: &PortableAuthority,
    base: Arc<State>,
    candidate: &Candidate,
) -> Result<()> {
    let generation = Generation {
        backup_continuation: None,
        hash: portable.hash()?,
        workspace_roots: portable.workspace_root_aliases()?,
        portable: portable.clone(),
        intent: portable.intent()?,
        original: portable.genesis_config()?,
        history: BTreeMap::new(),
        epoch: portable.epoch()?,
        base,
    };
    generation.verify_activation_candidate(candidate)
}

pub(crate) fn generation_directory(disk: &DiskConfig, hash: &str) -> PathBuf {
    disk.root.join("metadata/membership-generations").join(hash)
}
pub(crate) fn log_path(config: &ValidatedConfig, disk: &DiskConfig) -> PathBuf {
    match config.generation_hash() {
        Some(hash) => generation_directory(disk, hash).join(layout::JOURNAL.as_path()),
        None => crate::journal::log_path(disk),
    }
}

mod cancellation;
mod local;
mod publication;
mod recovery;
pub(in crate::engine) mod root_extension;
pub(in crate::engine) mod terminal_resume;
pub(crate) use local::{read_local_disk_configuration, verify_local_membership};
pub(crate) use publication::predecessor_seal_matches;
pub(crate) use recovery::{
    discover, discover_startup, discover_startup_in_store, latest_restore_config, restore_anchor,
};

impl Generation {
    pub(crate) fn verify_activation_candidate(&self, candidate: &Candidate) -> Result<()> {
        if let Some(core) = self.portable.backup_core() {
            return self.verify_backup_first_candidate(core, candidate);
        }
        if self.portable.is_foreign() {
            return self.verify_foreign_first_candidate(candidate);
        }
        self.portable.verify_registry_source(&self.base)?;
        let (state, result) = self.next_state()?;
        let snapshot = prepare_snapshot(&state, MAX_METADATA)?;
        let (history, _) = crate::history_package::prepare_history_manifests(&state, MAX_METADATA)?;
        if candidate.sequence != result.sequence
            || candidate.operation_id != result.operation_id
            || candidate.request_digest != result.request_digest
            || candidate.snapshot_hash != snapshot.hash
            || candidate.previous_snapshot != self.snapshot()
            || !candidate.versions.is_empty()
            || &candidate.history_roots != history.reference()
            || Some(candidate.transaction_id.clone()) != self.transaction_id(result.operation_id)
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "first generation candidate is not the prescribed activation",
            ));
        }
        self.verify_candidate(candidate)
    }
}
impl Engine {
    pub(in crate::engine) fn generation_activation_published(&self) -> bool {
        self.config.generation().is_some_and(|generation| {
            self.state
                .operations
                .contains_key(&generation.intent.operation_id)
        })
    }
}
#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/membership_activation_tests.rs"]
mod tests;
