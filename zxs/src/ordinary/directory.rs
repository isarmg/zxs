//! Directory identities and direct children belong to the original SYSTEM
//! prefix. An ordinary directory contains no private FILE metadata.
pub(in crate::ordinary) mod types;
pub(in crate::ordinary) mod operand;
mod path;
pub(in crate::ordinary) mod inventory;
pub(in crate::ordinary) mod file_start;
pub(in crate::ordinary) mod file_home;
mod prepare;
mod command;
pub use command::{DirectoryEntry,DirectoryInfo,DirectoryListing,make_directory,list_directory};
pub use types::Parent as DirectoryParent;
