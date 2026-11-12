//! Private structural records and held-directory I/O prerequisites.
//! No current production initializer, scanner or recovery path uses this module.
pub(crate) mod records;

pub(crate) use records::admission;
pub(crate) use records::{
    CurrentRootSelection, RegisteredEntry, StructurallyValidWorkingTreeCatalog,
};
