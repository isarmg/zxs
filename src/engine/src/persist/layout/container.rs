//! Addresses consumed only with a held namespace or admitted preparation scope.
use platform_io::fs_safety::EntryName;
use storage_contract::working_tree::DirectoryId;

pub(crate) fn preparation(id: DirectoryId) -> EntryName {
    EntryName::new(format!("container-prepare-{id}"))
        .expect("invariant: generated preparation identity is one relative entry")
}

/// Automatic source geometry is produced by the configured-name domain, not
/// recovered by splitting a caller path or interpreting a directory name.
pub(crate) fn source_components<'a>(
    path: &'a crate::disk_names::configuration::ConfiguredSourcePath<'_>,
) -> impl ExactSizeIterator<Item = EntryName> + 'a {
    path.components().map(|component| {
        EntryName::new(component)
            .expect("invariant: configured source name and UUID form one safe component")
    })
}

pub(crate) fn prefix_preparation(id: DirectoryId) -> EntryName {
    EntryName::new(format!("source-prepare-{id}"))
        .expect("invariant: generated preparation identity is one relative entry")
}

/// Draft payload ids are a separate domain from immutable VersionIds.
pub(crate) static DRAFTS: std::sync::LazyLock<EntryName> = std::sync::LazyLock::new(|| {
    EntryName::new("drafts").expect("invariant: draft storage is one internal entry")
});

pub(crate) fn draft_payload(id: storage_contract::file_draft::DraftPayloadId) -> EntryName {
    EntryName::new(id.to_string())
        .expect("invariant: typed draft identity is one relative entry")
}
