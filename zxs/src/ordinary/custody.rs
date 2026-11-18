//! A FILE operation can hand off only a real, complete, clean historical
//! container. A frozen source is local to one operation and one held FILE.
//! One native SYSTEM decision hands the same container identity to its exact
//! destination. Operands alone cannot authorize original source retirement.
mod source;
mod command;
mod finish;
mod retire;
pub(super) mod inventory;
pub use command::{move_file,move_receipt,MoveReceipt,MoveStatus};
mod staging;
pub(super) mod placed;
mod destination;
pub(super) mod transfer;
pub(super) mod allocation;
pub(super) use destination::Method;

#[cfg(test)]
#[path="../../tests/unit/custody_handoff.rs"]
mod tests;
