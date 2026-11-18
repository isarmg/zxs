//! Working-directory structure and complete immutable snapshot material.
//! This catalog can consume registered root structure, but cannot establish
//! physical roots, directory/container publication, or current authority.
pub(crate) mod records;
pub(crate) mod snapshot;

pub(crate) use records::admission;
pub(crate) use records::{
    CurrentRootSelection, RegisteredEntry, StructurallyValidWorkingTreeCatalog,
};
