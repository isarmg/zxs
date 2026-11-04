//! Hypothetical removal of one exact binding from an admitted, borrowed view.
//! No State copy, filesystem access, health probe, publication or executable plan.
use super::super::{
    Engine,
    disk_registry::{CurrentDiskRegistry, DiskRegistrationIdentity, RegisteredDisk},
};
use crate::{
    Branch, Commit, ContentState, DirectoryBranch, DirectoryTree, Entry, FileVersion, HistoryTree,
    NamespaceEntryKind, Placement, Repository, State, TrackedMap, TrackedRecords,
};

/// Failures identify the rejected input; no bad binding is treated as a lost file.
#[derive(Debug, error_definition::Error, PartialEq, Eq)]
pub enum DeregistrationPlanError {
    #[error("target slot is not currently registered: {0}")]
    TargetNotRegistered(String),
    #[error(
        "target identity differs from current registration: requested {requested:?}, registered {registered:?}"
    )]
    TargetIdentityMismatch {
        requested: Box<DiskRegistrationIdentity>,
        registered: Box<DiskRegistrationIdentity>,
    },
    #[error("version not found: {0}")]
    VersionNotFound(String),
    #[error("placement missing for version {0}")]
    PlacementMissing(String),
    #[error("placement of {version_id} names unregistered slot {slot_id}")]
    PlacementSlotNotRegistered { version_id: String, slot_id: String },
    #[error(
        "placement of {version_id} has binding {placement_generation:?}, expected {registered:?}"
    )]
    PlacementBindingMismatch {
        version_id: String,
        registered: Box<DiskRegistrationIdentity>,
        placement_generation: Option<u64>,
    },
    #[error("invalid placement of {version_id}: {reason}")]
    PlacementInvalid {
        version_id: String,
        reason: &'static str,
    },
    #[error("namespace not found: {0}")]
    NamespaceNotFound(String),
    #[error("namespace graph reference missing: {0}")]
    NamespaceReferenceMissing(String),
    #[error("page size {0} is outside 1..=256")]
    PageSizeOutOfRange(usize),
}
type PlanResult<T> = std::result::Result<T, DeregistrationPlanError>;

/// Actual admitted inputs, not a made-up candidate hash or a registry certificate.
/// None means the Engine has no corresponding published snapshot/generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrozenDeregistrationBoundary<'a> {
    pub sequence: u64,
    pub business_generation: u64,
    pub topology_generation: u64,
    pub published_snapshot_hash: Option<&'a str>,
    pub admitted_membership_generation_hash: Option<&'a str>,
}

/// A bounded number of metadata rows. Holders and strings remain borrowed.
#[derive(Clone, Copy, Debug)]
pub struct ImpactPageSize(usize);
impl TryFrom<usize> for ImpactPageSize {
    type Error = DeregistrationPlanError;
    fn try_from(value: usize) -> PlanResult<Self> {
        if !(1..=256).contains(&value) {
            return Err(DeregistrationPlanError::PageSizeOutOfRange(value));
        }
        Ok(Self(value))
    }
}
#[derive(Debug)]
pub struct ImpactPage<T> {
    pub rows: Vec<T>,
    /// Continue using the same borrowed scan. There is no reusable wire cursor.
    pub has_more: bool,
}

/// Only the tables used by this projection. No control_auth, operations,
/// principals, root paths, credentials or disk resources are retained/exported.
struct ContentGraph<'a> {
    versions: &'a TrackedRecords<FileVersion>,
    placements: &'a TrackedMap<Placement>,
    content_states: &'a TrackedMap<ContentState>,
    repositories: &'a TrackedMap<Repository>,
    histories: &'a TrackedMap<HistoryTree>,
    branches: &'a TrackedMap<Branch>,
    commits: &'a TrackedRecords<Commit>,
    directory_branches: &'a TrackedMap<DirectoryBranch>,
    directory_trees: &'a TrackedRecords<DirectoryTree>,
}

/// An immutable hypothetical preview, not an authorization or an execution token.
/// The borrow freezes the Engine for the scan; dropping it changes nothing.
/// Production construction is only through Engine::preview_disk_deregistration.
/// An ongoing page scan cannot be rebound to a later mutable Engine state.
///
/// ```compile_fail
/// use storage_engine::{DiskRegistrationIdentity, Engine};
/// fn require_writer(_: &mut Engine) {}
/// fn frozen(engine: &mut Engine, target: &DiskRegistrationIdentity) {
///     let preview = engine.preview_disk_deregistration(target).unwrap();
///     let mut pages = preview.versions();
///     require_writer(engine);
///     let _ = pages.next_page(16usize.try_into().unwrap());
/// }
/// ```
pub struct DeregistrationPreview<'a> {
    registry: CurrentDiskRegistry<'a>,
    target: RegisteredDisk<'a>,
    boundary: FrozenDeregistrationBoundary<'a>,
    graph: ContentGraph<'a>,
}
impl Engine {
    /// Preview metadata effects without touching any disk, even an unavailable
    /// target. This does not establish I/O quiescence,
    /// workspace safety, quorum viability or permission to execute a deletion.
    pub fn preview_disk_deregistration(
        &self,
        target: &DiskRegistrationIdentity,
    ) -> PlanResult<DeregistrationPreview<'_>> {
        DeregistrationPreview::capture(
            &self.state,
            CurrentDiskRegistry::from_engine(self),
            self.snapshot.as_deref(),
            target,
        )
    }
}
impl<'a> DeregistrationPreview<'a> {
    pub(in crate::engine) fn capture(
        state: &'a State,
        registry: CurrentDiskRegistry<'a>,
        snapshot: Option<&'a str>,
        target: &DiskRegistrationIdentity,
    ) -> PlanResult<Self> {
        let registered = registry
            .slot(&target.slot_id)
            .ok_or_else(|| DeregistrationPlanError::TargetNotRegistered(target.slot_id.clone()))?;
        if registered.pool_id != target.pool_id
            || registered.disk_id != target.disk_id
            || registered.slot_id != target.slot_id
            || registered.binding_generation != target.binding_generation
        {
            return Err(DeregistrationPlanError::TargetIdentityMismatch {
                requested: Box::new(target.clone()),
                registered: Box::new(registered.identity()),
            });
        }
        Ok(Self {
            registry,
            target: registered,
            boundary: FrozenDeregistrationBoundary {
                sequence: state.sequence,
                business_generation: state.business_generation,
                topology_generation: state.topology_generation,
                published_snapshot_hash: snapshot,
                admitted_membership_generation_hash: registry.generation_hash(),
            },
            graph: ContentGraph {
                versions: &state.versions,
                placements: &state.placements,
                content_states: &state.content_states,
                repositories: &state.repositories,
                histories: &state.histories,
                branches: &state.branches,
                commits: &state.commits,
                directory_branches: &state.directory_branches,
                directory_trees: &state.directory_trees,
            },
        })
    }
    pub fn boundary(&self) -> FrozenDeregistrationBoundary<'a> {
        self.boundary
    }
    pub fn target(&self) -> RegisteredDisk<'a> {
        self.target
    }
    /// Registration is independent of health/Enabled/Disabled and voter status.
    /// Current Config has no durable Disabled field; this API neither invents nor
    /// changes an I/O mode. Every retained admitted registration is counted.
    pub fn remaining_registered_disks(&self) -> impl Iterator<Item = RegisteredDisk<'a>> {
        let target = self.target;
        self.registry.disks().filter(move |disk| *disk != target)
    }
    pub fn version(&self, version_id: &str) -> PlanResult<VersionDeregistrationImpact<'a>> {
        let version = self
            .graph
            .versions
            .get(version_id)
            .ok_or_else(|| DeregistrationPlanError::VersionNotFound(version_id.into()))?;
        let placement = self
            .graph
            .placements
            .get(version_id)
            .ok_or_else(|| DeregistrationPlanError::PlacementMissing(version_id.into()))?;
        self.validate_placement(version, placement)?;
        let holders = RegisteredHolders {
            registry: self.registry,
            placement,
            excluded: self.target,
        };
        let target_role = if placement
            .route_slot_ids
            .iter()
            .any(|slot| slot == self.target.slot_id)
        {
            Some(if placement.primary_slot_id == self.target.slot_id {
                TargetPlacementRole::Primary
            } else {
                TargetPlacementRole::BackupOnly
            })
        } else {
            None
        };
        let effect = match (target_role, holders.iter().next()) {
            (None, _) => RegistrationEffect::Unaffected,
            (Some(_), None) => RegistrationEffect::TargetExclusive,
            (Some(_), Some(_)) => RegistrationEffect::SharedWithRegistered,
        };
        Ok(VersionDeregistrationImpact {
            version_id: &version.version_id,
            file_id: &version.file_id,
            history_tree_id: &version.history_tree_id,
            logical_size: version.logical_size,
            content_state: self
                .graph
                .content_states
                .get(version_id)
                .copied()
                .unwrap_or(ContentState::Available),
            target_role,
            effect,
            remaining_holders: holders,
            original_primary_slot_id: &placement.primary_slot_id,
            target_copies: placement.target_copies,
        })
    }
    fn validate_placement(&self, version: &FileVersion, placement: &Placement) -> PlanResult<()> {
        let invalid = |reason| DeregistrationPlanError::PlacementInvalid {
            version_id: version.version_id.clone(),
            reason,
        };
        if placement.version_id != version.version_id {
            return Err(invalid("version identity differs"));
        }
        if placement.route_slot_ids.first() != Some(&placement.primary_slot_id) {
            return Err(invalid("route does not start at its primary"));
        }
        if placement.binding_generations.len() != placement.route_slot_ids.len() {
            return Err(invalid("binding set differs from route"));
        }
        for (index, slot) in placement.route_slot_ids.iter().enumerate() {
            if placement.route_slot_ids[..index].contains(slot) {
                return Err(invalid("duplicate route slot"));
            }
            let registered = self.registry.slot(slot).ok_or_else(|| {
                DeregistrationPlanError::PlacementSlotNotRegistered {
                    version_id: version.version_id.clone(),
                    slot_id: slot.clone(),
                }
            })?;
            let generation = placement.binding_generations.get(slot).copied();
            if generation != Some(registered.binding_generation) {
                return Err(DeregistrationPlanError::PlacementBindingMismatch {
                    version_id: version.version_id.clone(),
                    registered: Box::new(registered.identity()),
                    placement_generation: generation,
                });
            }
        }
        Ok(())
    }
    /// All versions, including non-HEAD, Purged and Migrated records, in stable
    /// version-ID order. A row says whether this target affects that version.
    pub fn versions(&self) -> VersionImpactScan<'_> {
        VersionImpactScan {
            remaining: self.graph.versions.len(),
            rows: Box::new(self.graph.versions.keys().map(move |id| self.version(id))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetPlacementRole {
    Primary,
    BackupOnly,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistrationEffect {
    Unaffected,
    TargetExclusive,
    SharedWithRegistered,
}

/// Logical placement holders, not proof that bytes are available or healthy.
/// Iterator allocation is constant; no unbounded per-version holder Vec.
#[derive(Clone, Copy)]
pub struct RegisteredHolders<'a> {
    registry: CurrentDiskRegistry<'a>,
    placement: &'a Placement,
    excluded: RegisteredDisk<'a>,
}
impl<'a> RegisteredHolders<'a> {
    pub fn iter(self) -> impl Iterator<Item = RegisteredDisk<'a>> {
        self.placement
            .route_slot_ids
            .iter()
            .filter_map(move |slot| {
                let disk = self
                    .registry
                    .slot(slot)
                    .expect("invariant: impact validates every placement binding");
                (disk != self.excluded).then_some(disk)
            })
    }
}
pub struct VersionDeregistrationImpact<'a> {
    pub version_id: &'a str,
    pub file_id: &'a str,
    pub history_tree_id: &'a str,
    pub logical_size: u64,
    pub content_state: ContentState,
    pub target_role: Option<TargetPlacementRole>,
    pub effect: RegistrationEffect,
    pub remaining_holders: RegisteredHolders<'a>,
    /// Removing a primary never promotes a backup or reduces protection policy.
    pub original_primary_slot_id: &'a str,
    pub target_copies: u32,
}
pub struct VersionImpactScan<'a> {
    rows: Box<dyn Iterator<Item = PlanResult<VersionDeregistrationImpact<'a>>> + 'a>,
    remaining: usize,
}
impl<'a> VersionImpactScan<'a> {
    pub fn next_page(
        &mut self,
        size: ImpactPageSize,
    ) -> ImpactPage<PlanResult<VersionDeregistrationImpact<'a>>> {
        let rows: Vec<_> = self.rows.by_ref().take(size.0).collect();
        self.remaining -= rows.len();
        ImpactPage {
            rows,
            has_more: self.remaining != 0,
        }
    }
}

/// Exact frozen scope. Historical scopes are explicitly distinguished from HEAD.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeregistrationNamespace<'a> {
    BranchHead(&'a str),
    DirectoryHead(&'a str),
    Commit(&'a str),
    DirectoryTree(&'a str),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OccurrenceVisibility {
    /// Its existing namespace owner remains registered. Not a write capability.
    RegisteredNamespace,
    /// Content survives elsewhere, without inventing a primary-root entry.
    RetainedContentOnly,
    Hidden,
}
pub struct NamespaceDeregistrationPreview<'a> {
    plan: &'a DeregistrationPreview<'a>,
    scope: DeregistrationNamespace<'a>,
    frozen_head_id: &'a str,
    namespace_owner: RegisteredDisk<'a>,
    entries: &'a [Entry],
}
pub struct NamespaceOccurrenceImpact<'a> {
    /// Identity is (scope, entry_id), never file_id alone.
    pub scope: DeregistrationNamespace<'a>,
    pub entry_id: &'a str,
    pub path: &'a str,
    pub kind: NamespaceEntryKind,
    pub version: Option<VersionDeregistrationImpact<'a>>,
    pub visibility_after: OccurrenceVisibility,
}
impl<'a> DeregistrationPreview<'a> {
    pub fn namespace(
        &self,
        scope: DeregistrationNamespace<'_>,
    ) -> PlanResult<NamespaceDeregistrationPreview<'_>> {
        let missing = |id: &str| DeregistrationPlanError::NamespaceNotFound(id.into());
        let (scope, frozen_head_id, entries, repository) = match scope {
            DeregistrationNamespace::BranchHead(id) => {
                let branch = self.graph.branches.get(id).ok_or_else(|| missing(id))?;
                let commit = self
                    .graph
                    .commits
                    .get(&branch.head_commit_id)
                    .ok_or_else(|| missing(&branch.head_commit_id))?;
                (
                    DeregistrationNamespace::BranchHead(branch.branch_id.as_str()),
                    commit.commit_id.as_str(),
                    commit.entries.as_slice(),
                    self.repository_for_history(&branch.history_tree_id)?,
                )
            }
            DeregistrationNamespace::Commit(id) => {
                let commit = self.graph.commits.get(id).ok_or_else(|| missing(id))?;
                (
                    DeregistrationNamespace::Commit(commit.commit_id.as_str()),
                    commit.commit_id.as_str(),
                    commit.entries.as_slice(),
                    self.repository_for_history(&commit.history_tree_id)?,
                )
            }
            DeregistrationNamespace::DirectoryHead(id) => {
                let branch = self
                    .graph
                    .directory_branches
                    .get(id)
                    .ok_or_else(|| missing(id))?;
                (
                    DeregistrationNamespace::DirectoryHead(branch.directory_branch_id.as_str()),
                    branch.head_tree_id.as_str(),
                    branch.entries.as_slice(),
                    self.repository(&branch.repository_id)?,
                )
            }
            DeregistrationNamespace::DirectoryTree(id) => {
                let tree = self
                    .graph
                    .directory_trees
                    .get(id)
                    .ok_or_else(|| missing(id))?;
                let branch = self
                    .graph
                    .directory_branches
                    .get(&tree.directory_branch_id)
                    .ok_or_else(|| missing(&tree.directory_branch_id))?;
                (
                    DeregistrationNamespace::DirectoryTree(tree.tree_id.as_str()),
                    tree.tree_id.as_str(),
                    tree.entries.as_slice(),
                    self.repository(&branch.repository_id)?,
                )
            }
        };
        let owner = self
            .registry
            .slot(&repository.primary_slot_id)
            .ok_or_else(|| {
                DeregistrationPlanError::NamespaceReferenceMissing(
                    repository.primary_slot_id.clone(),
                )
            })?;
        Ok(NamespaceDeregistrationPreview {
            plan: self,
            scope,
            frozen_head_id,
            namespace_owner: owner,
            entries,
        })
    }
    fn repository(&self, id: &str) -> PlanResult<&'a Repository> {
        self.graph
            .repositories
            .get(id)
            .ok_or_else(|| DeregistrationPlanError::NamespaceReferenceMissing(id.into()))
    }
    fn repository_for_history(&self, id: &str) -> PlanResult<&'a Repository> {
        let history = self
            .graph
            .histories
            .get(id)
            .ok_or_else(|| DeregistrationPlanError::NamespaceReferenceMissing(id.into()))?;
        self.repository(&history.repository_id)
    }
}
impl<'a> NamespaceDeregistrationPreview<'a> {
    pub fn scope(&self) -> DeregistrationNamespace<'a> {
        self.scope
    }
    pub fn frozen_head_id(&self) -> &'a str {
        self.frozen_head_id
    }
    pub fn namespace_owner(&self) -> RegisteredDisk<'a> {
        self.namespace_owner
    }
    fn owner_remains(&self) -> bool {
        self.namespace_owner != self.plan.target
    }
    fn retained_file(&self, entry: &Entry) -> PlanResult<bool> {
        Ok(self
            .plan
            .version(&entry.version_id)?
            .remaining_holders
            .iter()
            .next()
            .is_some())
    }
    /// Directories with no content survive when their namespace owner remains.
    /// Otherwise only retained descendants keep their logical occurrence alive.
    /// This does not create a new primary root on any backup.
    pub fn root_visibility_after(&self) -> PlanResult<OccurrenceVisibility> {
        if self.owner_remains() {
            return Ok(OccurrenceVisibility::RegisteredNamespace);
        }
        for entry in self.entries {
            if entry.kind == NamespaceEntryKind::File && self.retained_file(entry)? {
                return Ok(OccurrenceVisibility::RetainedContentOnly);
            }
        }
        Ok(OccurrenceVisibility::Hidden)
    }
    fn occurrence(&self, entry: &'a Entry) -> PlanResult<NamespaceOccurrenceImpact<'a>> {
        let (version, retained) = match entry.kind {
            NamespaceEntryKind::File => {
                let impact = self.plan.version(&entry.version_id)?;
                let retained = impact.remaining_holders.iter().next().is_some();
                (Some(impact), retained)
            }
            NamespaceEntryKind::Directory => {
                let mut retained = self.owner_remains();
                if !retained {
                    for child in self.entries {
                        if child.kind == NamespaceEntryKind::File
                            && child
                                .lookup_key()
                                .strip_prefix(entry.lookup_key())
                                .is_some_and(|suffix| suffix.starts_with('/'))
                            && self.retained_file(child)?
                        {
                            retained = true;
                            break;
                        }
                    }
                }
                (None, retained)
            }
        };
        let visibility_after = if !retained {
            OccurrenceVisibility::Hidden
        } else if self.owner_remains() {
            OccurrenceVisibility::RegisteredNamespace
        } else {
            OccurrenceVisibility::RetainedContentOnly
        };
        Ok(NamespaceOccurrenceImpact {
            scope: self.scope,
            entry_id: &entry.entry_id,
            path: entry.path(),
            kind: entry.kind,
            version,
            visibility_after,
        })
    }
    pub fn occurrences(&self) -> NamespaceOccurrenceScan<'_> {
        NamespaceOccurrenceScan {
            remaining: self.entries.len(),
            rows: Box::new(self.entries.iter().map(move |entry| self.occurrence(entry))),
        }
    }
}
pub struct NamespaceOccurrenceScan<'a> {
    rows: Box<dyn Iterator<Item = PlanResult<NamespaceOccurrenceImpact<'a>>> + 'a>,
    remaining: usize,
}
impl<'a> NamespaceOccurrenceScan<'a> {
    pub fn next_page(
        &mut self,
        size: ImpactPageSize,
    ) -> ImpactPage<PlanResult<NamespaceOccurrenceImpact<'a>>> {
        let rows: Vec<_> = self.rows.by_ref().take(size.0).collect();
        self.remaining -= rows.len();
        ImpactPage {
            rows,
            has_more: self.remaining != 0,
        }
    }
}
