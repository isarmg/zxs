//! A FILE operation can hand off only a real, complete, clean historical
//! container. A frozen source is local to one operation and one held FILE.
//! One native SYSTEM decision hands the same container identity to its exact
//! destination. Operands alone cannot authorize original source retirement.
pub(crate) mod admission;
pub(super) mod purpose;
mod source;
pub(in crate::ordinary) use source::FrozenSecond;
pub(crate) use source::Clean as LifecycleSource;
mod command;
mod incoming;
mod target;
pub(crate) use target::{Goal as LifecycleTarget,Scan as LifecycleScan,Checked as LifecycleChecked};
pub(crate) use incoming::{IncomingSelection,KnownStage};
mod finish;
mod lifecycle;
mod recycle;
pub use recycle::{apply_recycle,recycle_receipt,list_retained,DeletionKey,DeletionTime,RestoreTarget,RecycleAction,RecycleKind,RecycleReceipt,RecycleCursor,RecycleFilter,RecyclePage,RetainedEntry};
mod retire;
pub(crate) use retire::dedicated::{Previous as LifecyclePrevious,Originals as LifecycleOriginals,Retiring as LifecycleRetiring};
pub(super) mod inventory;
pub use command::{move_file,move_file_at,move_receipt,move_receipt_at,MoveReceipt,MoveStatus};
mod staging;
pub(super) mod placed;
mod destination;
pub(super) mod transfer;
pub(super) mod allocation;
pub(super) use destination::Method;

#[cfg(test)]
#[path="../../tests/unit/custody_handoff.rs"]
mod tests;
