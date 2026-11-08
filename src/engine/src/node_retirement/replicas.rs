//! Immutable cleanup obligations from the original admitted placement. A later
//! route edit or slot rebinding must not discharge an offline old copy.
use super::*;

pub(crate) const MAX_CLEANUP_BINDINGS: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct CleanupBinding {
    pub(crate) version_id: String,
    pub(crate) slot_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub(crate) binding_generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct CleanupRoutes {
    pub(crate) bindings: Vec<CleanupBinding>,
}
impl CleanupRoutes {
    pub(super) fn capture(state: &State, request: &Request) -> Result<Self> {
        let mut bindings = Vec::new();
        for id in &request.version_ids {
            let placement = state.placements.get(id).ok_or_else(invalid)?;
            for slot in &placement.route_slot_ids {
                if bindings.len() == MAX_CLEANUP_BINDINGS {
                    return Err(Error::new(ErrorCode::ResourceLimit, "retirement cleanup binding set exceeds its budget"));
                }
                bindings.push(CleanupBinding {
                    version_id: id.clone(), slot_id: slot.clone(),
                    binding_generation: *placement.binding_generations.get(slot).ok_or_else(invalid)?,
                });
            }
        }
        bindings.sort();
        let result = Self { bindings };
        result.validate(request)?;
        Ok(result)
    }

    pub(super) fn validate(&self, request: &Request) -> Result<()> {
        if self.bindings.is_empty() || self.bindings.len() > MAX_CLEANUP_BINDINGS
            || self.bindings.windows(2).any(|pair| pair[0] >= pair[1]
                || (pair[0].version_id == pair[1].version_id && pair[0].slot_id == pair[1].slot_id)) {
            return Err(invalid());
        }
        let mut versions = BTreeSet::new();
        for binding in &self.bindings {
            HistoryVersionId::parse(&binding.version_id)?;
            let slot = binding.slot_id.as_bytes();
            if slot.len() > 64 || !slot.first().is_some_and(u8::is_ascii_alphabetic)
                || !slot.iter().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
                || request.version_ids.binary_search(&binding.version_id).is_err() {
                return Err(invalid());
            }
            versions.insert(&binding.version_id);
        }
        if versions.into_iter().ne(request.version_ids.iter()) { return Err(invalid()); }
        Ok(())
    }
}

pub(crate) fn original_cleanup_bindings(state: &State) -> Result<BTreeMap<String, Vec<CleanupBinding>>> {
    let mut result = BTreeMap::new();
    for receipt in state.operations.values().filter(|receipt| receipt.kind == KIND) {
        let decision = decision(receipt)?;
        decision.cleanup_routes.validate(&decision.request)?;
        for binding in decision.cleanup_routes.bindings {
            result.entry(binding.version_id.clone()).or_insert_with(Vec::new).push(binding);
        }
    }
    Ok(result)
}
