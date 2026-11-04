//! Read-only lifecycle impact planning. No detach, capsule or restoration action.
mod plan;
pub use plan::{
    DeregistrationNamespace, DeregistrationPlanError, DeregistrationPreview,
    FrozenDeregistrationBoundary, ImpactPage, ImpactPageSize, NamespaceDeregistrationPreview,
    NamespaceOccurrenceImpact, NamespaceOccurrenceScan, OccurrenceVisibility, RegisteredHolders,
    RegistrationEffect, TargetPlacementRole, VersionDeregistrationImpact, VersionImpactScan,
};
