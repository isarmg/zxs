//! Authenticated target inspection. There is no foreign import mutation here.
use super::*;
use storage_contract::storage_foreign_plan::ForeignPlanTarget;

pub(crate) struct ForeignDestination {
    slot: String,
    parent: Option<name_policy::PortablePath>,
    names: std::collections::BTreeMap<String, String>,
}
impl TryFrom<&ForeignPlanTarget> for ForeignDestination {
    type Error = ApiError;
    fn try_from(value: &ForeignPlanTarget) -> Result<Self, ApiError> {
        name_policy::validate_slot(&value.slot_id).map_err(ApiError::invalid_source)?;
        let parent = value
            .parent
            .as_deref()
            .map(name_policy::PortablePath::parse)
            .transpose()
            .map_err(ApiError::invalid_source)?;
        // The engine checks exact source coverage, names and collision keys.
        Ok(Self {
            slot: value.slot_id.clone(),
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
        let engine = crate::lock(&self.engine);
        current_protected_identity(&engine, identity)?;
        source
            .map_to_primary(&engine, &target.slot, target.parent.as_ref(), &target.names)
            .map_err(error)
    }
}
