//! A single deterministic joint-state recipe, carried by the exact membership
//! intent and recomputed on cold admission. No independent UI/material stage.
use super::*;
use crate::history_package::{AdmittedState, HistoryManifest, RawState};
use data_serialization::{Deserializer, Serializer};

mod projection;
const MAX_RECIPE: usize = 32 * 1024 * 1024;

/// Source State stays an archived foreign domain. Deserialization performs full
/// logical State admission; only a real joint certificate can attest its capture.
#[derive(Clone)]
struct OriginalState(State);
impl std::fmt::Debug for OriginalState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OriginalState")
            .field("sequence", &self.0.sequence)
            .finish_non_exhaustive()
    }
}
impl Serialize for OriginalState {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for OriginalState {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let raw = RawState::deserialize(deserializer)?;
        AdmittedState::try_from(raw)
            .map(|state| Self(state.into_state()))
            .map_err(data_serialization::de::Error::custom)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "foreign-single-origin-joint-state")]
    SingleOrigin,
}
/// Private fields prevent callers from constructing an unauthenticated recipe.
/// Serialized recipes have no standalone authority: the membership joint intent
/// must bind these exact bytes, and installation/serving remain separately gated.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct ForeignAdmissionRecipe {
    format: Format,
    identities: ForeignIdentityClosureReport,
    source: OriginalState,
    source_configuration: ValidatedConfig,
    source_slot: String,
    source_level: u32,
    source_primary: String,
}
impl Engine {
    /// Freeze source evidence for the one membership decision. Reject incomplete
    /// FULLs and currently unsupported multi-origin/lifecycle scopes without
    /// trimming histories, changing tombstones or writing any disk.
    pub(crate) fn capture_foreign_admission_recipe(
        &self,
        target: &ForeignIdentityTarget,
        source: &ForeignImportPreview,
    ) -> Result<ForeignAdmissionRecipe> {
        self.recheck_foreign_identity_target(target)?;
        if target
            .new_disks
            .last()
            .expect("invariant: complete new chain")
            .root
            != source.root.path()
        {
            return Err(invalid(
                "foreign admission target must retain the original highest root",
            ));
        }
        let mut source_configuration = None;
        let (original, admitted) = source.visit_original_states(|config, _, _, _| {
            source_configuration = Some(config.clone());
            Ok(())
        })?;
        let identities =
            Prepared::from_state(target, source.source_level(), original, admitted.state())?.report;
        let primary = source
            .histories()
            .first()
            .ok_or_else(|| invalid("foreign admission has no retained history"))?
            .origin();
        for history in source.histories() {
            if history.origin() != primary {
                return Err(Error::new(
                    ErrorCode::UnsupportedOperation,
                    "atomic foreign admission currently requires one original primary namespace; no other history was omitted",
                ));
            }
            if history
                .availability()
                .values()
                .any(|value| *value == ForeignVersionAvailability::TemporarilyUnavailable)
            {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "foreign admission requires every AVAILABLE original FULL; unavailable histories were preserved",
                ));
            }
        }
        let recipe = ForeignAdmissionRecipe {
            format: Format::SingleOrigin,
            identities,
            source: OriginalState(admitted.into_state()),
            source_configuration: source_configuration
                .expect("invariant: complete nonempty source prefix"),
            source_slot: source.source().registration().slot_id.clone(),
            source_level: source.source_level(),
            source_primary: primary.slot_id().to_owned(),
        };
        recipe.require_source()?;
        // Before the source graph is embedded in a persisted joint intent, its
        // encoding is bounded separately from the surrounding membership proof.
        super::graph::encoded_length(&recipe, MAX_RECIPE)?;
        source.revalidate_source()?;
        self.recheck_foreign_identity_target(target)?;
        Ok(recipe)
    }
}
impl ForeignAdmissionRecipe {
    pub(crate) fn recheck_request_source(
        &self,
        request: &ForeignIdentityTargetRequest,
        source: &ForeignImportPreview,
    ) -> Result<()> {
        let baseline = &self.identities.baseline;
        if request.operation_id != baseline.operation_id
            || request.request_digest.to_string() != baseline.request_digest
            || request.chain_digest.to_string() != baseline.chain_digest
            || request.registration_proposal_digest.to_string()
                != baseline.registration_proposal_digest
            || object_hash(
                "foreign-identity-proposed-configuration",
                &request.proposed_configuration,
            )? != baseline.proposed_configuration_hash
            || source.source_level() != self.source_level
            || source.source().registration().slot_id != self.source_slot
        {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "foreign retry differs from its exact original joint request",
            ));
        }
        if !request
            .proposed_configuration
            .disks
            .iter()
            .any(|disk| disk.level == self.source_level && disk.root == source.root.path())
        {
            return Err(invalid(
                "foreign retry changed its exact selected source root",
            ));
        }
        let (original, _) = source.visit_original_states(|_, _, _, _| Ok(()))?;
        if original != self.identities.original
            || source.histories().iter().any(|history| {
                history.origin().slot_id() != self.source_primary
                    || history
                        .availability()
                        .values()
                        .any(|value| *value == ForeignVersionAvailability::TemporarilyUnavailable)
            })
        {
            return Err(invalid(
                "foreign retry source no longer covers its complete frozen native State and FULLs",
            ));
        }
        Ok(())
    }
    pub(crate) fn identities(&self) -> &ForeignIdentityClosureReport {
        &self.identities
    }
    pub(crate) fn digest(&self) -> Result<String> {
        Ok(object_hash("foreign-joint-state-recipe", self)?)
    }
    pub(crate) fn first_sequence(&self, fence: u64) -> Result<u64> {
        fence
            .checked_add(self.source.0.sequence)
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::ResourceLimit,
                    "foreign joint publication sequence exhausted",
                )
            })
    }
    fn require_source(&self) -> Result<()> {
        if self.source_configuration.pool_id != self.identities.original.pool_id
            || object_hash("foreign-original-configuration", &self.source_configuration)?
                != self.identities.original.configuration_hash
            || self.source.0.sequence != self.identities.original.sequence
            || object_hash("foreign-original-state", &self.source.0)?
                != self.identities.original.state_hash
        {
            return Err(invalid(
                "foreign recipe differs from its exact original full State",
            ));
        }
        for receipt in self.source.0.operations.values() {
            if receipt.kind == crate::business::history_copy::KIND
                || crate::persist::path_changes::is_kind(&receipt.kind)
                    && !crate::persist::path_changes::read(receipt)?.is_empty()
            {
                return Err(Error::PathChange(crate::PathChangeError::ForeignSource));
            }
        }
        // Even an undone D contributes permanent file-local history through
        // lifecycle/copy receipts. Until that ledger is remapped, do not claim
        // a complete target merely because no current deletion remains.
        if self.source.0.operations.values().any(|receipt| {
            matches!(
                receipt.kind.as_str(),
                "file.delete"
                    | crate::file_deletion::batch::KIND
                    | crate::file_deletion::batch::UNDO
                    | crate::file_deletion::directory::DELETE
                    | crate::file_deletion::directory::UNDO
                    | "file.delete.undo"
                    | crate::business::FILE_HISTORY_MOVE_KIND
                    | crate::business::history_copy::RETAINED_KIND
                    | crate::business::history_copy::DIRECTORY_EXPORT_KIND
                    | crate::node_retirement_engine::KIND
            )
        }) || !self.source.0.file_deletions.is_empty()
            || self
                .source
                .0
                .content_states
                .values()
                .any(|state| *state == ContentState::Retired)
        {
            return Err(Error::new(
                ErrorCode::UnsupportedOperation,
                "foreign joint admission has no complete cross-pool deletion/undo/retirement ledger; original records were not dropped",
            ));
        }
        if self
            .source
            .0
            .repositories
            .values()
            .any(|repo| repo.primary_slot_id != self.source_primary)
            || self
                .source
                .0
                .placements
                .values()
                .any(|place| place.primary_slot_id != self.source_primary)
            || self.source.0.histories.is_empty()
        {
            return Err(invalid(
                "foreign recipe does not retain one complete original primary namespace",
            ));
        }
        Ok(())
    }
    pub(crate) fn verify_contract(
        &self,
        original: &Config,
        target: &Config,
        operation: OperationId,
    ) -> Result<()> {
        self.require_source()?;
        super::graph::encoded_length(self, MAX_RECIPE)?;
        let baseline = &self.identities.baseline;
        if baseline.operation_id != operation
            || original.pool_id != baseline.pool_id
            || original.pool_id == self.identities.original.pool_id
            || object_hash("foreign-identity-current-configuration", original)?
                != baseline.configuration_hash
            || object_hash("foreign-identity-proposed-configuration", target)?
                != baseline.proposed_configuration_hash
        {
            return Err(invalid(
                "foreign recipe differs from the current/target configuration or original operation",
            ));
        }
        Ok(())
    }
    fn naming(&self, base: &State, original: &Config, target: &Config) -> Result<Prepared> {
        self.verify_contract(original, target, self.identities.baseline.operation_id)?;
        if base.sequence != self.identities.baseline.sequence
            || object_hash("foreign-identity-current-state", base)?
                != self.identities.baseline.state_hash
        {
            return Err(invalid(
                "foreign joint recipe cannot rebind its complete current-pool State",
            ));
        }
        let catalog = Catalog::capture(base)?;
        let mut reserved = catalog.all_objects();
        reserved.extend(base.operations.keys().map(ToString::to_string));
        reserved.extend(catalog.principals);
        reserved.extend(base.plans.keys().cloned());
        reserved.extend(target.disks.iter().map(|disk| disk.disk_id.clone()));
        let mut new_disks: Vec<_> = target
            .disks
            .iter()
            .filter(|disk| !original.disks.iter().any(|old| old.disk_id == disk.disk_id))
            .cloned()
            .collect();
        new_disks.sort_by_key(|disk| disk.level);
        if new_disks.len() != self.source_level as usize || new_disks.len() < 2 {
            return Err(invalid(
                "foreign joint recipe omits a required real lower level",
            ));
        }
        for (index, disk) in new_disks.iter().enumerate() {
            let upstream = index
                .checked_sub(1)
                .map(|index| new_disks[index].slot_id.as_str());
            if disk.level != index as u32 + 1
                || disk.upstream_slot_id.as_deref() != upstream
                || disk.binding_generation != 1
            {
                return Err(invalid(
                    "foreign joint recipe has a partial or relinked target chain",
                ));
            }
        }
        let captured = ForeignIdentityTarget {
            baseline: self.identities.baseline.clone(),
            reserved,
            new_disks,
        };
        let prepared = Prepared::from_state(
            &captured,
            self.source_level,
            self.identities.original.clone(),
            &self.source.0,
        )?;
        if prepared.report != self.identities {
            return Err(invalid(
                "cold foreign identity derivation differs from its joint-bound mapping",
            ));
        }
        Ok(prepared)
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/foreign_import/foreign_admission_tests.rs"]
mod tests;
