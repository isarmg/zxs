//! Working-directory structure and complete immutable snapshot material.
//! Fresh initialization consumes only the complete root structural validator;
//! directory/container authority still requires its own actual publication.
pub(crate) mod records;
pub(crate) mod snapshot;

pub(crate) use records::admission;
pub(crate) use records::{
    CurrentRootSelection, RegisteredEntry, StructurallyValidWorkingTreeCatalog,
};
