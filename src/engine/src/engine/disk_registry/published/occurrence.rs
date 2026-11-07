//! Exact occurrence removal derived from an admitted frozen source, not a
//! caller-selected list of files. No object here is active serving authority.
use super::{
    DiskRegistrationIdentity, DiskRegistryCommitment, Engine, RegistryExitEffect, RegistrySource,
    VerifiedDiskRegistry, bounded_decode, digest, map,
};
use crate::{
    Entry, Error, Result,
    disk::MAX_METADATA,
    engine::disk_lifecycle::{
        DeregistrationNamespace, DeregistrationPlanError, DeregistrationPreview,
        NamespaceDeregistrationPreview, OccurrenceVisibility,
    },
    journal::{AuthenticatedMetadataObject, PreparedMetadataObject},
    metadata_map::{Encoder, Reference},
    object_hash,
};
use data_serialization::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

mod continuation;

const MASK_KIND: &str = "disk-occurrence-mask";

#[derive(Debug, error_definition::Error)]
pub(in crate::engine) enum ProjectionError {
    #[error(transparent)]
    Metadata(#[from] Error),
    #[error(transparent)]
    Preview(#[from] DeregistrationPlanError),
    #[error("detach source mismatch: {0}")]
    SourceMismatch(&'static str),
    #[error("occurrence mask differs from the complete frozen source")]
    MaskMismatch,
    #[error("projection baseline differs from workspace, scope, head or registry view")]
    BaselineMismatch,
}
type ProjectionResult<T> = std::result::Result<T, ProjectionError>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(
    tag = "kind",
    content = "id",
    rename_all = "kebab-case",
    deny_unknown_fields
)]
pub(in crate::engine) enum NamespaceScope {
    BranchHead(String),
    DirectoryHead(String),
    Commit(String),
    DirectoryTree(String),
}
impl NamespaceScope {
    fn key(&self) -> Result<String> {
        object_hash("disk-occurrence-scope", self).map_err(Error::from)
    }
}
impl From<DeregistrationNamespace<'_>> for NamespaceScope {
    fn from(value: DeregistrationNamespace<'_>) -> Self {
        match value {
            DeregistrationNamespace::BranchHead(id) => Self::BranchHead(id.into()),
            DeregistrationNamespace::DirectoryHead(id) => Self::DirectoryHead(id.into()),
            DeregistrationNamespace::Commit(id) => Self::Commit(id.into()),
            DeregistrationNamespace::DirectoryTree(id) => Self::DirectoryTree(id.into()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct ScopeBinding {
    scope: NamespaceScope,
    frozen_head_id: String,
    owner: DiskRegistrationIdentity,
    // Full entry bytes include path, kind, entry/file/version identity and size.
    entries_hash: String,
    disposition: ScopeDisposition,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "kebab-case")]
enum ScopeDisposition {
    RegisteredNamespace,
    RetainedContentOnly,
    Hidden,
}
impl From<OccurrenceVisibility> for ScopeDisposition {
    fn from(value: OccurrenceVisibility) -> Self {
        match value {
            OccurrenceVisibility::RegisteredNamespace => Self::RegisteredNamespace,
            OccurrenceVisibility::RetainedContentOnly => Self::RetainedContentOnly,
            OccurrenceVisibility::Hidden => Self::Hidden,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct MaskBinding {
    predecessor: DiskRegistryCommitment,
    source: RegistrySource,
    target: DiskRegistrationIdentity,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct MaskRoot {
    format: MaskFormat,
    binding: MaskBinding,
    scopes: Reference,
}
#[derive(Serialize, Deserialize)]
enum MaskFormat {
    #[serialization(rename = "authenticated-occurrence-mask")]
    Authenticated,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct ScopeRoot {
    binding: ScopeBinding,
    hidden: Reference,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct ScopeMask {
    binding: ScopeBinding,
    // Canonical occurrence path -> exact entry digest, including entry/file/version.
    // One file identity may have aliases with different versions in this scope.
    hidden: BTreeMap<String, String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct MaskState {
    binding: MaskBinding,
    scopes: BTreeMap<String, ScopeMask>,
}

/// The Engine borrow freezes all source tables. The registry must equal the
/// entire admitted base, not merely have a plausible hash or matching voter IDs.
/// A continuation also borrows the already admitted predecessor view; raw
/// historical heads cannot reconstruct its earlier occurrence removals.
pub(in crate::engine) struct FrozenDetachSource<'a> {
    state: &'a crate::State,
    registry: &'a VerifiedDiskRegistry,
    prior: Option<&'a super::super::current::CurrentNamespaces>,
    preview: DeregistrationPreview<'a>,
    known_file_ids: BTreeSet<&'a str>,
    known_entry_ids: BTreeSet<&'a str>,
    pending_version_ids: BTreeSet<&'a str>,
}
impl<'a> FrozenDetachSource<'a> {
    pub(in crate::engine) fn capture(
        engine: &'a Engine,
        registry: &'a VerifiedDiskRegistry,
        target: &DiskRegistrationIdentity,
    ) -> ProjectionResult<Self> {
        if let Some(prior) = &engine.current_namespaces {
            return Self::capture_current(
                &engine.state,
                &engine.config,
                engine
                    .snapshot
                    .as_deref()
                    .ok_or(ProjectionError::BaselineMismatch)?,
                engine
                    .config
                    .generation_hash()
                    .ok_or(ProjectionError::BaselineMismatch)?,
                prior,
                registry,
                target,
            );
        }
        Self::capture_state(
            &engine.state,
            &engine.config,
            engine.snapshot.as_deref(),
            engine.config.generation_hash(),
            registry,
            target,
        )
    }
    pub(in crate::engine) fn capture_state(
        state: &'a crate::State,
        config: &'a crate::ValidatedConfig,
        snapshot: Option<&'a str>,
        source_authority: Option<&str>,
        registry: &'a VerifiedDiskRegistry,
        target: &DiskRegistrationIdentity,
    ) -> ProjectionResult<Self> {
        if !registry.matches_frozen_source(config, state, snapshot, source_authority) {
            return Err(ProjectionError::SourceMismatch(
                "registry is not the authenticated frozen base",
            ));
        }
        if registry.state.source.snapshot_hash.is_none() {
            return Err(ProjectionError::SourceMismatch(
                "source has no published snapshot",
            ));
        }
        Self::capture_verified(state, config, snapshot, registry, target, None)
    }

    pub(in crate::engine) fn capture_current(
        state: &'a crate::State,
        config: &'a crate::ValidatedConfig,
        snapshot: &'a str,
        authority: &str,
        prior: &'a super::super::current::CurrentNamespaces,
        registry: &'a VerifiedDiskRegistry,
        target: &DiskRegistrationIdentity,
    ) -> ProjectionResult<Self> {
        if !prior.matches_source(state)
            || !registry.matches_current_source(
                &prior.registry_state,
                config,
                state,
                snapshot,
                authority,
            )?
        {
            return Err(ProjectionError::SourceMismatch(
                "registry continuation differs from its admitted current namespace source",
            ));
        }
        Self::capture_verified(state, config, Some(snapshot), registry, target, Some(prior))
    }

    fn capture_verified(
        state: &'a crate::State,
        config: &'a crate::ValidatedConfig,
        snapshot: Option<&'a str>,
        registry: &'a VerifiedDiskRegistry,
        target: &DiskRegistrationIdentity,
        prior: Option<&'a super::super::current::CurrentNamespaces>,
    ) -> ProjectionResult<Self> {
        let preview = match prior {
            Some(prior) => DeregistrationPreview::capture_projected(
                state,
                super::super::CurrentDiskRegistry::from_verified(config, registry),
                snapshot,
                target,
                Some(prior),
            )?,
            None => DeregistrationPreview::capture(
                state,
                super::super::CurrentDiskRegistry::from_config(config),
                snapshot,
                target,
            )?,
        };
        let known_file_ids = state
            .versions
            .values()
            .map(|version| version.file_id.as_str())
            .chain(
                state
                    .branches
                    .values()
                    .flat_map(|branch| branch.pending_file_versions.keys().map(String::as_str)),
            )
            .chain(
                state
                    .commits
                    .values()
                    .flat_map(|commit| commit.pending_file_versions.keys().map(String::as_str)),
            )
            .chain(
                prior
                    .into_iter()
                    .flat_map(|view| view.known_files.iter().map(String::as_str)),
            )
            .collect();
        let known_entry_ids = state
            .commits
            .values()
            .flat_map(|commit| commit.entries.iter().map(|entry| entry.entry_id.as_str()))
            .chain(
                state
                    .directory_branches
                    .values()
                    .flat_map(|branch| branch.entries.iter().map(|entry| entry.entry_id.as_str())),
            )
            .chain(
                state
                    .directory_trees
                    .values()
                    .flat_map(|tree| tree.entries.iter().map(|entry| entry.entry_id.as_str())),
            )
            .chain(
                prior
                    .into_iter()
                    .flat_map(|view| view.known_entries.iter().map(String::as_str)),
            )
            .collect();
        let pending_version_ids = state
            .branches
            .values()
            .flat_map(|branch| branch.pending_file_versions.values().map(String::as_str))
            .chain(
                state
                    .commits
                    .values()
                    .flat_map(|commit| commit.pending_file_versions.values().map(String::as_str)),
            )
            .collect();
        Ok(Self {
            state,
            registry,
            prior,
            preview,
            known_file_ids,
            known_entry_ids,
            pending_version_ids,
        })
    }
    fn binding(&self) -> MaskBinding {
        MaskBinding {
            predecessor: self.registry.commitment.clone(),
            source: self.registry.state.source.clone(),
            target: self.preview.target().identity(),
        }
    }
    fn expected(&self) -> ProjectionResult<MaskState> {
        let scopes = self
            .preview
            .namespaces()
            .map(|scope| {
                let namespace = self.preview.namespace(scope)?;
                let mask = scope_mask(&namespace)?;
                Ok((mask.binding.scope.key()?, mask))
            })
            .collect::<ProjectionResult<_>>()?;
        Ok(MaskState {
            binding: self.binding(),
            scopes,
        })
    }
    pub(in crate::engine) fn prepare_mask(&self) -> ProjectionResult<PreparedOccurrenceMask> {
        Ok(encode_mask(&self.expected()?)?)
    }
    /// Authenticate every supplied page, then compare the complete semantic graph
    /// to this source. Rehashing an omission or a shared-file deletion cannot pass.
    /// The reader must apply MAX_METADATA before allocating/reading object bytes.
    pub(in crate::engine) fn verify_mask(
        &self,
        expected_hash: &str,
        read: impl FnMut(&'static str, &str) -> Result<Vec<u8>>,
    ) -> ProjectionResult<VerifiedOccurrenceMask<'_, 'a>> {
        let state = load_mask(expected_hash, read)?;
        if state != self.expected()? {
            return Err(ProjectionError::MaskMismatch);
        }
        Ok(VerifiedOccurrenceMask {
            source: self,
            hash: expected_hash.into(),
            state,
        })
    }
}

pub(in crate::engine) struct PreparedOccurrenceMask {
    hash: String,
    objects: Vec<PreparedMetadataObject>,
}
impl PreparedOccurrenceMask {
    pub(in crate::engine) fn hash(&self) -> &str {
        &self.hash
    }
    pub(in crate::engine) fn objects(
        &self,
    ) -> impl ExactSizeIterator<Item = (&'static str, &str, &[u8])> {
        self.objects
            .iter()
            .map(|object| (object.kind(), object.hash.as_str(), object.bytes()))
    }
}
/// Semantic proof with a live source borrow, not a deserializable certificate.
pub(in crate::engine) struct VerifiedOccurrenceMask<'s, 'e> {
    source: &'s FrozenDetachSource<'e>,
    hash: String,
    state: MaskState,
}
impl<'s, 'e> VerifiedOccurrenceMask<'s, 'e> {
    fn require_successor(&self, successor: &VerifiedDiskRegistry) -> ProjectionResult<()> {
        let pending = successor
            .state
            .pending_exit
            .as_ref()
            .ok_or(ProjectionError::BaselineMismatch)?;
        if Some(successor.commitment.view_generation())
            != self
                .source
                .registry
                .commitment
                .view_generation()
                .checked_add(1)
            || successor.state.predecessor.as_ref() != Some(self.source.registry.commitment())
            || successor.state.source != self.state.binding.source
            || pending.target != self.state.binding.target
            || !matches!(&pending.effect, RegistryExitEffect::Detach { occurrence_mask_hash } if occurrence_mask_hash == &self.hash)
        {
            return Err(ProjectionError::BaselineMismatch);
        }
        Ok(())
    }
}

impl VerifiedOccurrenceMask<'_, '_> {
    pub(in crate::engine) fn prepare_current_namespaces(
        &self,
        successor: &VerifiedDiskRegistry,
    ) -> ProjectionResult<super::super::current::PreparedCurrentNamespaces> {
        self.require_successor(successor)?;
        self.source
            .own_current_namespaces(successor, Some(&self.hash), Some(&self.state.scopes))
    }
}
impl FrozenDetachSource<'_> {
    pub(in crate::engine) fn prepare_disabled_namespaces(
        &self,
        successor: &VerifiedDiskRegistry,
    ) -> ProjectionResult<super::super::current::PreparedCurrentNamespaces> {
        let pending = successor
            .state
            .pending_exit
            .as_ref()
            .ok_or(ProjectionError::BaselineMismatch)?;
        if successor.state.predecessor.as_ref() != Some(self.registry.commitment())
            || successor.state.source != self.registry.state.source
            || pending.target != self.preview.target().identity()
            || !matches!(pending.effect, RegistryExitEffect::Disable)
            || successor.commitment.view_generation()
                != self
                    .registry
                    .commitment
                    .view_generation()
                    .checked_add(1)
                    .ok_or(ProjectionError::BaselineMismatch)?
        {
            return Err(ProjectionError::BaselineMismatch);
        }
        self.own_current_namespaces(successor, None, None)
    }

    fn own_current_namespaces(
        &self,
        successor: &VerifiedDiskRegistry,
        mask_hash: Option<&str>,
        masks: Option<&BTreeMap<String, ScopeMask>>,
    ) -> ProjectionResult<super::super::current::PreparedCurrentNamespaces> {
        use super::super::current::{CurrentNamespaces, CurrentScope, PreparedCurrentNamespaces};
        use std::sync::Arc;
        let mut branches = BTreeMap::new();
        let mut directories = BTreeMap::new();
        for scope in self.preview.namespaces() {
            let owned: NamespaceScope = scope.into();
            let target = match &owned {
                NamespaceScope::BranchHead(id) => (&mut branches, id),
                NamespaceScope::DirectoryHead(id) => (&mut directories, id),
                NamespaceScope::Commit(_) | NamespaceScope::DirectoryTree(_) => continue,
            };
            let namespace = self.preview.namespace(scope)?;
            let mask = masks.map(|masks| {
                &masks[&owned
                    .key()
                    .expect("invariant: an admitted scope serializes")]
            });
            let entries: Vec<Entry> = namespace
                .entries()
                .iter()
                .filter(|entry| {
                    mask.is_none_or(|mask| !mask.hidden.contains_key(entry.lookup_key()))
                })
                .cloned()
                .collect();
            let prior_scope = self.prior.and_then(|prior| match &owned {
                NamespaceScope::BranchHead(id) => prior.branches.get(id),
                NamespaceScope::DirectoryHead(id) => prior.directories.get(id),
                NamespaceScope::Commit(_) | NamespaceScope::DirectoryTree(_) => None,
            });
            let writable = namespace.owner_writable_after()
                && prior_scope.is_none_or(|scope| scope.writable)
                && mask.is_none_or(|mask| {
                    mask.binding.disposition == ScopeDisposition::RegisteredNamespace
                });
            let mut hidden_paths = prior_scope
                .filter(|scope| scope.head == scope.source_head)
                .map(|scope| scope.hidden_paths.clone())
                .unwrap_or_default();
            if let Some(mask) = mask {
                hidden_paths.extend(mask.hidden.keys().cloned());
            }
            let entries = Arc::new(entries);
            target.0.insert(
                target.1.clone(),
                Arc::new(CurrentScope {
                    source_head: namespace.frozen_head_id().to_owned(),
                    source_entries: Arc::clone(&entries),
                    head: namespace.frozen_head_id().to_owned(),
                    entries,
                    writable,
                    hidden_paths,
                }),
            );
        }
        let mut unavailable_versions = self
            .prior
            .map(|prior| (*prior.unavailable_versions).clone())
            .unwrap_or_default();
        if masks.is_some() {
            for id in self.state.versions.keys() {
                if self
                    .preview
                    .version(id)?
                    .remaining_holders
                    .iter()
                    .next()
                    .is_none()
                {
                    unavailable_versions.insert(id.clone());
                }
            }
            unavailable_versions.extend(
                self.pending_version_ids
                    .iter()
                    .filter(|id| !self.state.versions.contains_key(**id))
                    .map(|id| (*id).to_owned()),
            );
        }
        Ok(PreparedCurrentNamespaces {
            view: CurrentNamespaces {
                registry: successor.commitment.clone(),
                registry_state: Arc::new(successor.clone()),
                mask_hash: mask_hash.map(str::to_owned),
                source_snapshot: self
                    .registry
                    .state
                    .source
                    .snapshot_hash
                    .clone()
                    .ok_or(ProjectionError::BaselineMismatch)?,
                source_sequence: self.state.sequence,
                activation_sequence: self.state.sequence,
                source_business_generation: self.state.business_generation,
                branches: Arc::new(branches),
                directories: Arc::new(directories),
                known_entries: Arc::new(
                    self.known_entry_ids
                        .iter()
                        .map(|id| (*id).to_owned())
                        .collect(),
                ),
                known_files: Arc::new(
                    self.known_file_ids
                        .iter()
                        .map(|id| (*id).to_owned())
                        .collect(),
                ),
                unavailable_versions: Arc::new(unavailable_versions),
                // Initial registry activation precedes native recursive configuration.
                // Registry cold recovery replays the entire admitted chain, not a checkpoint.
                recursive_policies: self
                    .prior
                    .map(|prior| Arc::clone(&prior.recursive_policies))
                    .unwrap_or_default(),
            },
        })
    }
}

fn scope_mask(namespace: &NamespaceDeregistrationPreview<'_>) -> ProjectionResult<ScopeMask> {
    let entries = namespace.entries();
    let mut hidden = BTreeMap::new();
    let mut scan = namespace.occurrences();
    let mut entry_scan = entries.iter();
    loop {
        let page = scan.next_page(256.try_into()?);
        for row in page.rows {
            let row = row?;
            let entry = entry_scan
                .next()
                .expect("invariant: occurrence scan enumerates the same frozen entries");
            if row.visibility_after == OccurrenceVisibility::Hidden {
                hidden.insert(
                    entry.lookup_key().to_owned(),
                    object_hash("disk-occurrence-entry", entry).map_err(Error::from)?,
                );
            }
        }
        if !page.has_more {
            break;
        }
    }
    Ok(ScopeMask {
        binding: ScopeBinding {
            scope: namespace.scope().into(),
            frozen_head_id: namespace.frozen_head_id().into(),
            owner: namespace.namespace_owner().clone(),
            entries_hash: object_hash("disk-occurrence-entries", &entries).map_err(Error::from)?,
            disposition: namespace.root_visibility_after()?.into(),
        },
        hidden,
    })
}
fn encode_mask(state: &MaskState) -> Result<PreparedOccurrenceMask> {
    let mut encoder = Encoder::new(MAX_METADATA);
    let mut scopes = BTreeMap::new();
    for (key, scope) in &state.scopes {
        scopes.insert(
            key.clone(),
            ScopeRoot {
                binding: scope.binding.clone(),
                hidden: map(&mut encoder, &scope.hidden)?,
            },
        );
    }
    let root = MaskRoot {
        format: MaskFormat::Authenticated,
        binding: state.binding.clone(),
        scopes: map(&mut encoder, &scopes)?,
    };
    Ok(PreparedOccurrenceMask {
        hash: encoder.object(MASK_KIND, &root)?,
        objects: encoder.finish(),
    })
}
fn load_mask(
    hash: &str,
    mut read: impl FnMut(&'static str, &str) -> Result<Vec<u8>>,
) -> Result<MaskState> {
    digest(hash)?;
    let mut load = |kind, hash: &str| {
        AuthenticatedMetadataObject::authenticate(kind, hash, bounded_decode(&read(kind, hash)?)?)
    };
    let root: MaskRoot = json_codec::from_value(load(MASK_KIND, hash)?.into_value())?;
    let mut scopes = BTreeMap::new();
    for (key, value) in crate::metadata_map::read_map(&root.scopes, &mut load)? {
        let scope: ScopeRoot = json_codec::from_value(value)?;
        let hidden = crate::metadata_map::read_map(&scope.hidden, &mut load)?
            .into_iter()
            .map(|(key, value)| Ok((key, json_codec::from_value(value)?)))
            .collect::<Result<_>>()?;
        scopes.insert(
            key,
            ScopeMask {
                binding: scope.binding,
                hidden,
            },
        );
    }
    Ok(MaskState {
        binding: root.binding,
        scopes,
    })
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/disk_occurrence_mask_tests.rs"]
mod tests;
