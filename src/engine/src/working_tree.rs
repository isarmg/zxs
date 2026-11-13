//! Working-directory structure and complete immutable snapshot material.
//! No current production initializer, scanner or recovery path uses this module.
pub(crate) mod records;
pub(crate) mod snapshot;

pub(crate) use records::admission;
pub(crate) use records::{
    CurrentRootSelection, RegisteredEntry, StructurallyValidWorkingTreeCatalog,
};
