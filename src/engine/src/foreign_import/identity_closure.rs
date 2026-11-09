//! Collision-free, inert object names for a future foreign joint State. Source
//! receipts and actors are namespaced audit references, never local authority.
use super::*;
use data_serialization::{Deserialize, Serialize};
use std::collections::BTreeSet;

mod admission;
mod catalog;
mod graph;
pub(crate) use admission::ForeignAdmissionRecipe;
use catalog::Catalog;
pub use graph::ForeignHistoryGraphReport;
pub(super) use graph::PreparedGraph;

pub(crate) const MAX_MAPPING_BYTES: usize = 16 * 1024 * 1024;
const MAX_IDENTITIES: usize = 100_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ForeignIdentityBaseline {
    pub pool_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub sequence: u64,
    pub state_hash: String,
    pub configuration_hash: String,
    pub proposed_configuration_hash: String,
    pub operation_id: OperationId,
    pub request_digest: String,
    pub chain_digest: String,
    pub registration_proposal_digest: String,
}

/// Typed input to the current Engine boundary, not an authorization token.
pub struct ForeignIdentityTargetRequest {
    pub proposed_configuration: Config,
    pub operation_id: OperationId,
    pub request_digest: ForeignEvidenceDigest,
    pub chain_digest: ForeignEvidenceDigest,
    pub registration_proposal_digest: ForeignEvidenceDigest,
}

/// Captured from an admitted Engine and a complete extension of its Config.
/// Not deserializable and not a membership/authorization capability.
pub struct ForeignIdentityTarget {
    pub(super) baseline: ForeignIdentityBaseline,
    reserved: BTreeSet<String>,
    new_disks: Vec<DiskConfig>,
}
impl ForeignIdentityTarget {
    pub fn baseline(&self) -> &ForeignIdentityBaseline {
        &self.baseline
    }
    pub(super) fn require_lower(
        &self,
        root: &std::path::Path,
        level: u32,
        source_path: &std::path::Path,
    ) -> Result<()> {
        if self
            .new_disks
            .last()
            .expect("invariant: validated complete new chain")
            .root
            .as_path()
            != source_path
        {
            return Err(invalid(
                "identity material changed the original foreign source root",
            ));
        }
        if !self.new_disks[..self.new_disks.len() - 1]
            .iter()
            .any(|disk| disk.root.as_path() == root && disk.level == level)
        {
            return Err(invalid(
                "identity material root is not an exact proposed real lower binding",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ForeignIdentityClosureReport {
    pub baseline: ForeignIdentityBaseline,
    pub original: ForeignOriginalStateReport,
    pub mapping_digest: String,
    pub objects_by_domain: BTreeMap<String, usize>,
    pub objects: usize,
    pub archived_operations: usize,
    pub archived_principals: usize,
    pub archived_plans: usize,
    pub archived_events: usize,
    #[serialization(with = "storage_contract::u64_string")]
    pub mapping_bytes: u64,
}

pub(super) struct Prepared {
    pub(super) report: ForeignIdentityClosureReport,
    pub(super) bytes: Vec<u8>,
    objects: BTreeMap<catalog::Domain, BTreeMap<String, String>>,
    principals: BTreeMap<String, String>,
}
#[derive(Serialize)]
struct Mapping<'a> {
    format: &'static str,
    baseline: &'a ForeignIdentityBaseline,
    original: &'a ForeignOriginalStateReport,
    objects: BTreeMap<catalog::Domain, BTreeMap<String, String>>,
    // Separate tags deliberately cannot be parsed as local op/user/plan IDs.
    archived_operations: BTreeMap<String, String>,
    archived_principals: BTreeMap<String, String>,
    archived_plans: BTreeMap<String, String>,
    original_operations_hash: String,
    original_principals_hash: String,
    original_plans_hash: String,
    original_events_hash: String,
    original_control_auth_hash: String,
}

impl Engine {
    /// Freeze the complete current State, not only its Config. This is a naming
    /// preparation and performs no reservation, publication or database write.
    pub fn capture_foreign_identity_target(
        &self,
        request: ForeignIdentityTargetRequest,
    ) -> Result<ForeignIdentityTarget> {
        self.require_write()?;
        let operation = request.operation_id;
        if self.state.operations.contains_key(&operation) {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "foreign identity operation already belongs to the current pool",
            ));
        }
        let proposed = ValidatedConfig::try_from(request.proposed_configuration)?;
        let current = self.config();
        let originals: BTreeSet<_> = current
            .disks
            .iter()
            .map(|disk| disk.disk_id.as_str())
            .collect();
        let mut added: Vec<_> = proposed
            .disks
            .iter()
            .filter(|disk| !originals.contains(disk.disk_id.as_str()))
            .cloned()
            .collect();
        if added.len() < 2
            || added.len() > 32
            || added.iter().any(|disk| disk.binding_generation != 1)
        {
            return Err(invalid(
                "identity closure requires 2 to 32 complete new registration bindings",
            ));
        }
        added.sort_by_key(|disk| disk.level);
        for (index, disk) in added.iter().enumerate() {
            let upstream = index
                .checked_sub(1)
                .map(|index| added[index].slot_id.as_str());
            if disk.level != index as u32 + 1 || disk.upstream_slot_id.as_deref() != upstream {
                return Err(invalid(
                    "new registration identities must form one complete adjacent-level chain",
                ));
            }
        }
        let mut preserved = Config::clone(&proposed);
        preserved
            .disks
            .retain(|disk| originals.contains(disk.disk_id.as_str()));
        if object_hash("foreign-preserved-configuration", &preserved)?
            != object_hash("foreign-preserved-configuration", current)?
            || self
                .registered_disks()
                .any(|disk| disk.registration_mode != RegistrationMode::Enabled)
        {
            return Err(invalid(
                "identity closure must preserve the exact enabled current pool and all its settings",
            ));
        }
        let catalog = Catalog::capture(&self.state)?;
        let mut reserved = catalog.all_objects();
        reserved.extend(self.state.operations.keys().map(ToString::to_string));
        reserved.extend(catalog.principals.iter().cloned());
        reserved.extend(self.state.plans.keys().cloned());
        reserved.extend(proposed.disks.iter().map(|disk| disk.disk_id.clone()));
        let baseline = ForeignIdentityBaseline {
            pool_id: current.pool_id.clone(),
            sequence: self.state.sequence,
            state_hash: object_hash("foreign-identity-current-state", self.state.as_ref())?,
            configuration_hash: object_hash("foreign-identity-current-configuration", current)?,
            proposed_configuration_hash: object_hash(
                "foreign-identity-proposed-configuration",
                &*proposed,
            )?,
            operation_id: operation,
            request_digest: request.request_digest.to_string(),
            chain_digest: request.chain_digest.to_string(),
            registration_proposal_digest: request.registration_proposal_digest.to_string(),
        };
        Ok(ForeignIdentityTarget {
            baseline,
            reserved,
            new_disks: added,
        })
    }

    /// Re-establish the same current State/authenticated pool baseline after
    /// unlocked source reads and before any inert target material is stored.
    pub fn recheck_foreign_identity_target(&self, target: &ForeignIdentityTarget) -> Result<()> {
        self.require_write()?;
        if self.config.pool_id != target.baseline.pool_id
            || self.state.sequence != target.baseline.sequence
            || object_hash("foreign-identity-current-state", self.state.as_ref())?
                != target.baseline.state_hash
            || object_hash("foreign-identity-current-configuration", self.config())?
                != target.baseline.configuration_hash
        {
            return Err(Error::new(
                ErrorCode::GenerationMismatch,
                "current pool State or registration baseline changed; original identity material cannot be rebound",
            ));
        }
        Ok(())
    }
}

impl Prepared {
    pub(super) fn capture(
        target: &ForeignIdentityTarget,
        source: &ForeignImportPreview,
    ) -> Result<Self> {
        let (original, admitted) = source.visit_original_states(|_, _, _, _| Ok(()))?;
        Self::from_state(target, source.source_level(), original, admitted.state())
    }
    fn from_state(
        target: &ForeignIdentityTarget,
        source_level: u32,
        original: ForeignOriginalStateReport,
        state: &State,
    ) -> Result<Self> {
        if source_level as usize != target.new_disks.len() {
            return Err(invalid(
                "object identity target omits or adds a required lower level",
            ));
        }
        if original.pool_id == target.baseline.pool_id {
            return Err(invalid(
                "foreign object closure cannot relabel its own pool",
            ));
        }
        if original.registrations.iter().any(|old| {
            target
                .new_disks
                .iter()
                .any(|new| new.disk_id == old.disk_id)
        }) {
            return Err(invalid(
                "new chain registrations reuse an original source disk identity",
            ));
        }
        let catalog = Catalog::capture(state)?;
        if catalog.count + catalog.principals.len() + state.operations.len() + state.plans.len()
            > MAX_IDENTITIES
        {
            return Err(budget());
        }
        let mut occupied = target.reserved.clone();
        occupied.extend(catalog.all_objects());
        occupied.extend(catalog.principals.iter().cloned());
        occupied.extend(
            original
                .registrations
                .iter()
                .map(|disk| disk.disk_id.clone()),
        );
        let scope = object_hash(
            "foreign-object-identity-scope",
            &(&target.baseline, &original),
        )?;
        let mut objects = BTreeMap::new();
        let mut counts = BTreeMap::new();
        for (domain, values) in &catalog.objects {
            let mapped = assign(
                &scope,
                domain.prefix(),
                values.iter().map(String::as_str),
                &mut occupied,
            )?;
            counts.insert(domain.prefix().to_owned(), mapped.len());
            objects.insert(*domain, mapped);
        }
        let operation_ids: Vec<_> = state.operations.keys().map(ToString::to_string).collect();
        let archived_operations = assign(
            &scope,
            "foreignop",
            operation_ids.iter().map(String::as_str),
            &mut occupied,
        )?;
        let archived_principals = assign(
            &scope,
            "foreignactor",
            catalog.principals.iter().map(String::as_str),
            &mut occupied,
        )?;
        let archived_plans = assign(
            &scope,
            "foreignplan",
            state.plans.keys().map(String::as_str),
            &mut occupied,
        )?;
        let mapping = Mapping {
            format: "zxs-foreign-object-identity-closure",
            baseline: &target.baseline,
            original: &original,
            objects,
            archived_operations,
            archived_principals,
            archived_plans,
            original_operations_hash: object_hash(
                "foreign-identity-original-operations",
                &state.operations,
            )?,
            original_principals_hash: object_hash(
                "foreign-identity-original-principals",
                &state.operation_principals,
            )?,
            original_plans_hash: object_hash("foreign-identity-original-plans", &state.plans)?,
            original_events_hash: object_hash("foreign-identity-original-events", &state.events)?,
            original_control_auth_hash: object_hash(
                "foreign-identity-original-control-auth",
                &state.control_auth,
            )?,
        };
        let bytes = canonical_json(&mapping)?;
        if bytes.len() > MAX_MAPPING_BYTES {
            return Err(budget());
        }
        let report = ForeignIdentityClosureReport {
            baseline: target.baseline.clone(),
            original: original.clone(),
            mapping_digest: object_hash("foreign-object-identity-closure", &mapping)?,
            objects: catalog.count,
            objects_by_domain: counts,
            archived_operations: mapping.archived_operations.len(),
            archived_principals: mapping.archived_principals.len(),
            archived_plans: mapping.archived_plans.len(),
            archived_events: state.events.len(),
            mapping_bytes: bytes.len() as u64,
        };
        Ok(Self {
            report,
            bytes,
            objects: mapping.objects,
            principals: mapping.archived_principals,
        })
    }
}
fn assign<'a>(
    scope: &str,
    prefix: &str,
    values: impl Iterator<Item = &'a str>,
    occupied: &mut BTreeSet<String>,
) -> Result<BTreeMap<String, String>> {
    let mut result = BTreeMap::new();
    for original in values {
        let digest = object_hash("foreign-object-identity", &(scope, prefix, original))?;
        let mapped = format!("{prefix}_{}", &digest[..32]);
        if !occupied.insert(mapped.clone()) {
            return Err(Error::new(
                ErrorCode::ObjectCollision,
                "derived foreign identity collides with an original, current or proposed object",
            ));
        }
        result.insert(original.to_owned(), mapped);
    }
    Ok(result)
}
fn budget() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "foreign identity closure exceeds 100000 identities or 16 MiB",
    )
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/foreign_import/identity_closure_tests.rs"]
mod tests;
