//! Private Core authority for the embedded Rust SMB startup and worker.
use super::{Catalog,Connection,InitialReader,denied};

#[path="smb_reference_capacity.rs"]
mod reference_capacity;
#[path="smb_first_plan.rs"]
mod first;
pub(crate) use first::{CapacityUnits,QualifiedRustInstall,RustRoot,RustRootCapacity,RustRootProjection,RustRootStamp};
pub(super) use first::OrdinaryMoveSite;
pub(crate) use first::preflight::{PreflightClaim,PreflightEpoch,PreflightPlan,PreflightRead,PreflightSelection,
    preflight_read,web_reader};
