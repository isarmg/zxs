//! Lifecycle planning, restrictive startup admission and verified publication.
//! Stop hints restrict I/O; independently verified generations supply authority.
pub(crate) mod bootstrap;
pub(in crate::engine) mod terminal;
pub use terminal::{
    TerminalChainReadmissionPreparation, TerminalChainReadmissionProposal, TerminalDiskExit,
    TerminalManagement, TerminalManagementStatus, TerminalRegistrationState,
    TerminalRetirementArchive, TerminalRetirementCheckpoint, TerminalRetirementPhase,
    TerminalRetirementPlan, TerminalRetirementStatus,
};
mod plan;
pub use plan::{
    DeregistrationNamespace, DeregistrationPlanError, DeregistrationPreview,
    FrozenDeregistrationBoundary, ImpactPage, ImpactPageSize, NamespaceDeregistrationPreview,
    NamespaceOccurrenceImpact, NamespaceOccurrenceScan, OccurrenceVisibility, RegisteredHolders,
    RegistrationEffect, TargetPlacementRole, VersionDeregistrationImpact, VersionImpactScan,
};

pub(in crate::engine) use super::membership_transition::pending::activation::exit::AdmittedDiskExitPublication;
