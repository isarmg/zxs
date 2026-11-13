//! Planning for independent editable views of an immutable directory snapshot.
//!
//! This module does not grant filesystem or publication authority. Its owner must
//! persist each transition under the repository writer fence and reconcile a
//! pending publication with its permanent operation receipt before exposing SMB.
pub(crate) mod lifecycle;
pub(crate) mod names;
pub(crate) mod store;
