//! Directory-oriented primary placement read model. No historical content moves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacementScope {
    pub id: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrimaryPlacementTarget {
    pub slot_id: String,
    pub root: String,
    pub unavailable_reason: Option<PrimaryPlacementBlock>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacementConfigurationSnapshot {
    pub repository_id: String,
    pub repository_name: String,
    pub scope: PlacementScope,
    pub rule_id: Option<String>,
    pub generation: u64,
    /// The effective path rule may still name a detached slot absent from targets.
    pub primary_slot_id: String,
    pub inherited: bool,
    pub required_copies: u32,
    pub targets: Vec<PrimaryPlacementTarget>,
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    data_serialization::Serialize,
    data_serialization::Deserialize,
)]
#[serialization(rename_all = "snake_case")]
pub enum PrimaryPlacementBlock {
    InsufficientCopies,
    MissingBackupPrefixes,
    /// A required registration is disabled or detached, not a health probe.
    RouteUnavailable,
}
