//! Foreign joint authority has a distinct strict schema. It is not empty Add
//! authority and cannot mint active I/O merely by parsing a target Config.
use super::authority::AuthorityNode;
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "foreign-membership-joint-state")]
    JointState,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct ForeignCore {
    format: Format,
    addition: AuthorityNode,
}
impl ForeignCore {
    pub(super) fn new(addition: AuthorityNode) -> Result<Self> {
        let result = Self {
            format: Format::JointState,
            addition,
        };
        result.intent()?;
        Ok(result)
    }
    pub(super) fn node(&self) -> &AuthorityNode {
        &self.addition
    }
    pub(super) fn intent(&self) -> Result<TransitionIntent> {
        self.addition.foreign_intent()
    }
    pub(super) fn activation_required() -> Error {
        Error::new(
            ErrorCode::RecoveryRequired,
            "foreign joint authority requires exact FULL/three-scope installation and foreign-preserving root activation; ordinary Add cannot activate it",
        )
    }
    pub(super) fn verify_first_slot(&self, candidate: &Candidate, hash: &str) -> Result<()> {
        let intent = self.intent()?;
        if candidate.sequence != intent.first_sequence()?
            || candidate.generation() != Some(hash)
            || candidate.operation_id != intent.operation_id
            || candidate.request_digest != object_hash(INTENT_KIND, &intent)?
            || candidate.previous_snapshot
                != intent
                    .boundary
                    .as_ref()
                    .map(|boundary| boundary.snapshot_hash.clone())
            || candidate.transaction_id != format!("tx_{}", &hash[..32])
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "foreign candidate is not the exact joint first slot; later writers require completed admission",
            ));
        }
        self.addition.verify_membership(candidate, &intent, hash)
    }
}

impl Generation {
    pub(super) fn foreign_first_state(&self) -> Result<(State, OperationResult)> {
        let recipe = self
            .intent
            .foreign
            .as_ref()
            .expect("invariant: Foreign core requires recipe");
        let mut next = recipe.joint_state(
            &self.base,
            self.intent.origin.config(),
            &self.intent.target,
            self.intent.fence_sequence,
        )?;
        let result = self.result()?;
        next.operations.insert(result.operation_id, result.clone());
        next.operation_principals
            .insert(result.operation_id, self.intent.principal.clone());
        crate::history_package::validate_prepared_state(&next)?;
        Ok((next, result))
    }
    pub(super) fn verify_foreign_first_candidate(&self, candidate: &Candidate) -> Result<()> {
        self.portable.verify_registry_source(&self.base)?;
        let (next, result) = self.foreign_first_state()?;
        let snapshot = prepare_snapshot(&next, MAX_METADATA)?;
        let (history, _) = crate::history_package::prepare_history_manifests(&next, MAX_METADATA)?;
        let versions: Vec<_> = next
            .versions
            .iter()
            .filter(|(id, _)| {
                !self.base.versions.contains_key(*id)
                    && matches!(
                        next.content_states.get(*id),
                        None | Some(ContentState::Available)
                    )
            })
            .collect();
        if candidate.sequence != result.sequence
            || candidate.snapshot_hash != snapshot.hash
            || &candidate.history_roots != history.reference()
            || candidate.versions.len() != versions.len()
            || candidate
                .versions
                .iter()
                .zip(versions)
                .any(|(actual, (id, version))| {
                    actual.version != *version || actual.placement != next.placements[id]
                })
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "foreign first snapshot/history/FULL obligations differ from joint-state recomputation",
            ));
        }
        self.verify_candidate(candidate)
    }
}
