//! Lifecycle planning and restrictive bootstrap storage primitives.
//! Neither primitive implements deregistration or changes membership authority.
#[allow(
    dead_code,
    reason = "restrictive bootstrap primitive awaits the production discovery gate"
)]
pub(crate) mod bootstrap;
mod plan;
pub use plan::{
    DeregistrationNamespace, DeregistrationPlanError, DeregistrationPreview,
    FrozenDeregistrationBoundary, ImpactPage, ImpactPageSize, NamespaceDeregistrationPreview,
    NamespaceOccurrenceImpact, NamespaceOccurrenceScan, OccurrenceVisibility, RegisteredHolders,
    RegistrationEffect, TargetPlacementRole, VersionDeregistrationImpact, VersionImpactScan,
};
