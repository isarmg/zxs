//! Ordinary user components remain below an independently held namespace parent.
use name_policy::LookupName;
use platform_io::fs_safety::EntryName;

pub(crate) fn directory(name: &LookupName) -> EntryName {
    EntryName::new(name.as_str())
        .expect("invariant: an admitted ordinary component is one relative entry")
}

/// Physical retained spelling never replaces the logical SMB suffix or its identity.
pub(crate) fn container(
    namespace: &storage_contract::file_container::ContainerNamespace,
) -> EntryName {
    match &namespace.parent {
        storage_contract::file_container::ContainerParent::Worktree { name, .. } => {
            EntryName::new(name.as_str())
                .expect("invariant: RetainedName admits one complete physical component")
        }
        storage_contract::file_container::ContainerParent::Root
        | storage_contract::file_container::ContainerParent::Directory(_) => {
            directory(&namespace.name)
        }
    }
}
