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

pub(crate) fn namespace_preparation(id: DirectoryId) -> EntryName {
    EntryName::new(format!("namespace-prepare-{id}"))
        .expect("invariant: generated namespace preparation is one relative entry")
}

/// Draft payload ids are a separate domain from immutable VersionIds.
pub(crate) static DRAFTS: std::sync::LazyLock<EntryName> = std::sync::LazyLock::new(|| {
    EntryName::new("drafts").expect("invariant: draft storage is one internal entry")
});

pub(crate) fn draft_payload(id: storage_contract::file_draft::DraftPayloadId) -> EntryName {
    EntryName::new(id.to_string())
        .expect("invariant: typed draft identity is one relative entry")
}

/// An extent is directly inside its already admitted container. There is no
/// unrecorded intermediate directory whose replacement could rebind the source.
pub(crate) fn working_extent(
    id: storage_contract::file_draft::manifest::WorkingExtentId,
) -> EntryName {
    EntryName::new(id.to_string())
        .expect("invariant: typed working extent identity is one relative entry")
}

/// A pack stays private unless the original FILE decision references its sealed identity.
pub(crate) fn working_pack(id: storage_contract::file_draft::manifest::packed::PackId) -> EntryName {
    EntryName::new(id.to_string()).expect("invariant: working pack identity is one relative entry")
}

/// A private full materialization is not a Version, draft payload or extent.
/// Its identity is never accepted as published state or reconstructed by name.
pub(crate) enum WorkingFull {}
impl storage_contract::IdKind for WorkingFull {
    const PREFIX: &'static str = "wfull";
}
pub(crate) type WorkingFullId = storage_contract::Id<WorkingFull>;

pub(crate) fn working_full(id: WorkingFullId) -> EntryName {
    EntryName::new(id.to_string())
        .expect("invariant: private materialization identity is one relative entry")
}
