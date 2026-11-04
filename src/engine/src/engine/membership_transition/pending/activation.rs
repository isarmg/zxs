//! Hash-linked pure-add generations. The joint authority is portable, while activation
//! additionally requires held roots, local control archives and actual journals.
use super::*;
use crate::engine::membership_transition::archive::persistent::{
    ArchiveRoot, install_generation_archive, read_generation_base,
};
use joint::store::PortableJoint;

pub(in crate::engine::membership_transition) const AUTHORITY_KIND: &str = "membership-generation";
const GENERATION_FILE: &str = "metadata/pool-membership/generation.json";

mod authority;
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
    pub(crate) base: Arc<State>,
}
impl Generation {
    pub(crate) fn membership_epoch(&self) -> u64 {
        self.epoch
    }
    pub(crate) fn verify_candidate(&self, candidate: &Candidate) -> Result<()> {
        match candidate.generation() {
            Some(hash) if hash == self.hash && candidate.sequence > self.intent.fence_sequence => {
                self.intent
                    .target
                    .verify_original_candidate_membership(candidate)
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
        self.intent
            .fence_sequence
            .checked_add(1)
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimit, "activation sequence exhausted"))
    }
    pub(crate) fn transaction_id(&self, operation: OperationId) -> Option<String> {
        (operation == self.intent.operation_id).then(|| format!("tx_{}", &self.hash[..32]))
    }
    fn result(&self) -> Result<OperationResult> {
        Ok(OperationResult {
            operation_id: self.intent.operation_id,
            kind: "membership-add".into(),
            commit_id: None,
            history_tree_id: None,
            branch_id: None,
            commit_state: storage_contract::CommitState::Published,
            protection_at_publish: storage_contract::ProtectionAtPublish::NotApplicable,
            current_health: storage_contract::CurrentHealth::Healthy,
            request_digest: object_hash(INTENT_KIND, &self.intent)?,
            sequence: self.first_sequence()?,
            result: json_codec::json!({"generation": self.hash, "membership_epoch": self.membership_epoch().to_string(), "metadata_members": self.intent.target.metadata_members().len(), "topology_generation": self.base.topology_generation.checked_add(1).ok_or_else(|| Error::new(ErrorCode::ResourceLimit, "topology generation exhausted"))?.to_string()}),
        })
    }
    fn next_state(&self) -> Result<(State, OperationResult)> {
        let mut next = (*self.base).clone();
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
            &transition,
            observe,
            maximum_objects,
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
        hash: portable.hash()?,
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
        Some(hash) => generation_directory(disk, hash).join("journal.bin"),
        None => crate::journal::log_path(disk),
    }
}

mod local;
mod publication;
mod recovery;
pub(crate) use local::verify_local_membership;
pub(crate) use publication::predecessor_seal_matches;
pub(crate) use recovery::{discover, latest_restore_config, restore_anchor};

impl Generation {
    pub(crate) fn verify_activation_candidate(&self, candidate: &Candidate) -> Result<()> {
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
