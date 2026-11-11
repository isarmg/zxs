//! Authenticated target inspection. There is no foreign import mutation here.
use super::*;
use storage_contract::storage_foreign_plan::ForeignPlanTarget;

pub(crate) struct ForeignDestination {
    slot: name_policy::SlotId,
    parent: Option<name_policy::PortablePath>,
    names: std::collections::BTreeMap<String, String>,
}
impl TryFrom<&ForeignPlanTarget> for ForeignDestination {
    type Error = ApiError;
    fn try_from(value: &ForeignPlanTarget) -> Result<Self, ApiError> {
        let slot = name_policy::SlotId::parse(&value.slot_id).map_err(ApiError::invalid_source)?;
        let parent = value
            .parent
            .as_deref()
            .map(name_policy::PortablePath::parse)
            .transpose()
            .map_err(ApiError::invalid_source)?;
        // The engine checks exact source coverage, names and collision keys.
        Ok(Self {
            slot,
            parent,
            names: value.history_names.clone(),
        })
    }
}

impl EngineBackend {
    pub(crate) fn map_foreign_plan(
        &self,
        identity: &Identity,
        source: &storage_engine::ForeignImportPreview,
        target: &ForeignDestination,
    ) -> Result<storage_engine::ForeignImportMapping, ApiError> {
        let (engine, _) = self.lock_current(identity)?;
        source
            .map_to_primary(
                &engine,
                target.slot.as_str(),
                target.parent.as_ref(),
                &target.names,
            )
            .map_err(error)
    }
}

#[cfg(test)]
#[path = "../../../../test/server/unit/engine_backend/foreign_plan.rs"]
mod tests;
