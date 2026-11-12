//! Complete global business-map rows, including retained content/metadata
//! nodes and current lifecycle/placement state. Mutable rows describe the
//! captured State, never an invented sequence of their previous values.
use super::*;
use data_serialization::Deserialize;
use std::collections::BTreeMap;

/// A catalog row retains its map identity even when its value is only a path or
/// selection. This is an Engine read model, not a recovery/admission credential.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct SystemCatalogRecord {
    pub id: String,
    pub value: SystemCatalogValue,
}

#[derive(Serialize)]
struct RecordRef<'a> {
    id: &'a str,
    value: ValueRef<'a>,
}

/// One declaration binds every supported row family to its exact State map,
/// durable discriminator and typed result. No field-name reflection or JSON
/// filtering determines scope.
macro_rules! catalog_get {
    ($state:ident, container_tags, $id:ident) => {
        $id.parse::<storage_contract::FileId>().ok()
            .and_then(|key| $state.container_tags.get(&key))
    };
    ($state:ident, $field:ident, $id:ident) => { $state.$field.get($id) };
}
macro_rules! catalog_families {
    ($( $variant:ident => ($number:literal, $field:ident, $(#[$codec:meta])* $value:ty) ),+ $(,)?) => {
        /// Catalog families are independent of operation/path event families.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
        pub enum SystemCatalogKind { $( $variant ),+ }

        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        #[serialization(tag = "kind", content = "record", rename_all = "kebab-case", deny_unknown_fields)]
        pub enum SystemCatalogValue { $( $variant($(#[$codec])* $value) ),+ }

        #[derive(Serialize)]
        #[serialization(tag = "kind", content = "record", rename_all = "kebab-case")]
        enum ValueRef<'a> { $( $variant($(#[$codec])* &'a $value) ),+ }

        impl SystemCatalogKind {
            pub(super) const ALL: &'static [Self] = &[$(Self::$variant),+];

            pub(super) fn sql(self) -> i64 {
                match self { $( Self::$variant => $number ),+ }
            }
            pub(super) fn contains(self, state: &State, id: &str) -> bool {
                match self { $( Self::$variant => catalog_get!(state, $field, id).is_some() ),+ }
            }
            pub(super) fn payload(self, state: &State, id: &str) -> Result<Vec<u8>> {
                let value = match self {
                    $( Self::$variant => ValueRef::$variant(catalog_get!(state, $field, id)
                        .expect("invariant: trusted catalog key belongs to admitted State")) ),+
                };
                encode(&RecordRef { id, value })
            }
        }

        fn changes(before: Option<&State>, after: &State) -> Vec<FamilyDelta> {
            // Assign every State field either to this complete business-map
            // projection or to an explicit separate scope. New State fields
            // cannot silently disappear from the system-database decision.
            let State {
                $( $field: _, )+
                // Existing event families have their own sequence ordering.
                operations: _,
                file_path_nodes: _,
                // Audit rows keep their independent identities/order.
                events: _,
                operation_principals: _,
                sequence: _,
                business_generation: _,
                topology_generation: _,
                // Authentication secrets never enter the public read model.
                control_auth: _,
            } = after;
            vec![$( FamilyDelta {
                kind: SystemCatalogKind::$variant,
                keys: changed_keys!(before, after, $field),
            } ),+]
        }
    };
}

macro_rules! changed_keys {
    ($before:ident, $after:ident, $field:ident) => {{
        match $before {
            None => $after.$field.keys().map(ToString::to_string).collect(),
            Some(before) => match $after.$field.changes_since(&before.$field) {
                MapChanges::Keys(keys) => keys
                    .iter()
                    .filter(|id| before.$field.get(*id) != $after.$field.get(*id))
                    .map(ToString::to_string)
                    .collect(),
                MapChanges::All => {
                    let mut changed = Vec::new();
                    let mut old = before.$field.iter().peekable();
                    for (id, value) in $after.$field.iter() {
                        while let Some((id, _)) = old.next_if(|(old, _)| *old < id) {
                            changed.push(id.to_string());
                        }
                        let previous = old.next_if(|(old, _)| *old == id).map(|(_, v)| v);
                        if previous != Some(value) {
                            changed.push(id.to_string());
                        }
                    }
                    changed.extend(old.map(|(id, _)| id.to_string()));
                    changed
                }
            },
        }
    }};
}

catalog_families! {
    Repository => (2, repositories, Repository),
    History => (3, histories, HistoryTree),
    Branch => (4, branches, Branch),
    NamespaceCommit => (5, commits, Commit),
    DirectoryRepository => (6, directory_git_repositories, storage_contract::directory_git::DirectoryGitRepository),
    DirectoryBranch => (7, directory_branches, DirectoryBranch),
    DirectoryTree => (8, directory_trees, DirectoryTree),
    FileLocation => (9, file_locations, String),
    FileBackupPaths => (10, file_backup_paths, BTreeMap<String, String>),
    VersionPlacement => (11, placements, Placement),
    FileSelection => (12, file_selections, String),
    DirectorySelection => (13, directory_selections, String),
    DirectoryTreeSelection => (14, directory_tree_selections, String),
    FileContext => (15, file_contexts, FileContext),
    FileWriteOrigin => (16, file_write_origins, String),
    FileVersion => (17, versions, FileVersion),
    FileMetadataNode => (18, file_metadata_nodes, FileMetadataNode),
    FileDeletion => (19, file_deletions, FileDeletionNode),
    FileBranchMerge => (20, file_branch_merges, FileBranchMerge),
    FileBranchAlias => (21, file_branch_aliases, String),
    FileNodeParents => (22, file_node_parents, Vec<String>),
    FilePathNote => (23, file_path_notes, String),
    FileBackupPolicy => (24, file_backup_policies, String),
    DirectorySelectionGeneration => (25, directory_selection_generations, #[serialization(with = "storage_contract::u64_string")] u64),
    FileSelectionGeneration => (26, file_selection_generations, #[serialization(with = "storage_contract::u64_string")] u64),
    FileNote => (27, file_notes, String),
    VersionNote => (28, version_notes, String),
    FileName => (29, file_names, String),
    MergeLink => (30, merge_links, MergeLink),
    HistoryBoundary => (31, history_boundaries, HistoryBoundary),
    ContentState => (32, content_states, ContentState),
    Plan => (33, plans, Plan),
    PlacementRule => (34, placement_rules, PlacementRule),
    ProtectionPolicy => (35, protection_policies, ProtectionPolicy),
    ContainerTags => (39, container_tags, storage_contract::file_container_tags::ContainerTags),
}

pub(super) struct FamilyDelta {
    kind: SystemCatalogKind,
    keys: Vec<String>,
}

pub(super) struct CatalogDelta(Vec<FamilyDelta>);
impl CatalogDelta {
    pub(super) fn between(before: Option<&State>, after: &State) -> Self {
        Self(changes(before, after))
    }

    pub(super) fn update_order(&self, order: &mut CatalogOrder, after: &State) {
        for family in &self.0 {
            let rows = order.0.entry(family.kind).or_default();
            for id in &family.keys {
                let key = (0, id.clone());
                if family.kind.contains(after, id) {
                    rows.insert(key);
                } else {
                    rows.remove(&key);
                }
            }
        }
    }

    pub(super) async fn project(
        &self,
        connection: &mut sqlite_access::SqliteConnection,
        after: &State,
    ) -> std::result::Result<(), ProjectionFailure> {
        for family in &self.0 {
            for id in &family.keys {
                let payload = family
                    .kind
                    .contains(after, id)
                    .then(|| family.kind.payload(after, id))
                    .transpose()
                    .map_err(|source| ProjectionFailure {
                        source,
                        corrupt: false,
                    })?;
                store_payload(
                    connection,
                    Kind::Catalog(family.kind),
                    id,
                    payload.as_deref().map(|bytes| (0, bytes)),
                )
                .await?;
            }
        }
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct CatalogOrder(BTreeMap<SystemCatalogKind, BTreeSet<(u64, String)>>);
impl CatalogOrder {
    pub(super) fn rows(&self, kind: SystemCatalogKind) -> &BTreeSet<(u64, String)> {
        self.0
            .get(&kind)
            .expect("invariant: admitted catalog initializes every family")
    }
}

impl Engine {
    /// Read one complete global business-map family from the system SQLite
    /// cache, including versions, tags, lifecycle and policy state. Mutable
    /// values describe this captured State, not a chronological event stream.
    /// Pages are ordered by canonical map key and bound to State and projection
    /// instance. The caller must authorize global metadata access; this does
    /// not grant access to a file's body or authority to restore these rows.
    pub fn system_catalog_records(
        &self,
        kind: SystemCatalogKind,
        cursor: Option<&SystemChangeCursor>,
        limit: usize,
    ) -> std::result::Result<SystemChangePage<SystemCatalogRecord>, SystemChangeReadError> {
        self.system_change_page(Kind::Catalog(kind), cursor, limit)
    }
}
