//! HTTP-independent request values and the primary-placement wire projection.
use data_serialization::Serialize;

#[derive(Clone, Debug)]
pub struct ReadPlacementConfiguration {
    pub repository_id: String,
    pub path: String,
}

#[derive(Clone, Debug)]
pub struct ConfigurePlacement {
    pub repository_id: String,
    pub path: String,
    pub primary_slot_id: String,
    pub expected_scope_id: String,
    pub expected_rule_id: Option<String>,
    pub expected_generation: u64,
    pub operation_id: storage_contract::OperationId,
}

#[derive(Serialize)]
pub(crate) struct PlacementConfigurationResponse {
    repository_id: String,
    repository_name: String,
    scope: ScopeResponse,
    rule_id: Option<String>,
    #[serialization(with = "storage_contract::u64_string")]
    generation: u64,
    primary_slot_id: String,
    inherited: bool,
    required_copies: u32,
    targets: Vec<TargetResponse>,
}
#[derive(Serialize)]
struct ScopeResponse {
    id: String,
    path: String,
}
#[derive(Serialize)]
struct TargetResponse {
    slot_id: String,
    root: String,
    unavailable_reason: Option<storage_contract::PrimaryPlacementBlock>,
}
impl From<storage_contract::PlacementConfigurationSnapshot> for PlacementConfigurationResponse {
    fn from(snapshot: storage_contract::PlacementConfigurationSnapshot) -> Self {
        let storage_contract::PlacementConfigurationSnapshot {
            repository_id,
            repository_name,
            scope,
            rule_id,
            generation,
            primary_slot_id,
            inherited,
            required_copies,
            targets,
        } = snapshot;
        let storage_contract::PlacementScope { id, path } = scope;
        Self {
            repository_id,
            repository_name,
            scope: ScopeResponse { id, path },
            rule_id,
            generation,
            primary_slot_id,
            inherited,
            required_copies,
            targets: targets
                .into_iter()
                .map(|target| {
                    let storage_contract::PrimaryPlacementTarget {
                        slot_id,
                        root,
                        unavailable_reason,
                    } = target;
                    TargetResponse {
                        slot_id,
                        root,
                        unavailable_reason,
                    }
                })
                .collect(),
        }
    }
}
