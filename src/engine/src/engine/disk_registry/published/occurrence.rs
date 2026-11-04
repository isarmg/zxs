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

pub(in crate::engine) mod projection;
use projection::WriteProjection;

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
    #[error("namespace has no retained writable owner or is historical")]
    NotWritable,
    #[error("projected write would reuse a hidden or foreign identity: {0}")]
    IdentityReuse(String),
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
    fn borrowed(&self) -> DeregistrationNamespace<'_> {
        match self {
            Self::BranchHead(id) => DeregistrationNamespace::BranchHead(id),
            Self::DirectoryHead(id) => DeregistrationNamespace::DirectoryHead(id),
            Self::Commit(id) => DeregistrationNamespace::Commit(id),
            Self::DirectoryTree(id) => DeregistrationNamespace::DirectoryTree(id),
        }
    }
    fn key(&self) -> Result<String> {
        object_hash("disk-occurrence-scope", self)
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
/// This first-stage source deliberately rejects unactivated successor registries.
/// A later activation path must establish its own admitted current-source proof.
pub(in crate::engine) struct FrozenDetachSource<'a> {
    engine: &'a Engine,
    registry: &'a VerifiedDiskRegistry,
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
        if registry.state != engine.disk_registry_base_state() {
            return Err(ProjectionError::SourceMismatch(
                "registry is not the admitted base",
            ));
        }
        if registry.state.source.snapshot_hash.is_none() {
            return Err(ProjectionError::SourceMismatch(
                "source has no published snapshot",
            ));
        }
        let preview = engine.preview_disk_deregistration(target)?;
        let known_file_ids = engine
            .state
            .versions
            .values()
            .map(|version| version.file_id.as_str())
            .chain(
                engine
                    .state
                    .branches
                    .values()
                    .flat_map(|branch| branch.pending_file_versions.keys().map(String::as_str)),
            )
            .chain(
                engine
                    .state
                    .commits
                    .values()
                    .flat_map(|commit| commit.pending_file_versions.keys().map(String::as_str)),
            )
            .collect();
        let known_entry_ids =
            engine
                .state
                .commits
                .values()
                .flat_map(|commit| commit.entries.iter().map(|entry| entry.entry_id.as_str()))
                .chain(
                    engine.state.directory_branches.values().flat_map(|branch| {
                        branch.entries.iter().map(|entry| entry.entry_id.as_str())
                    }),
                )
                .chain(
                    engine
                        .state
                        .directory_trees
                        .values()
                        .flat_map(|tree| tree.entries.iter().map(|entry| entry.entry_id.as_str())),
                )
                .collect();
        let pending_version_ids =
            engine
                .state
                .branches
                .values()
                .flat_map(|branch| branch.pending_file_versions.values().map(String::as_str))
                .chain(
                    engine.state.commits.values().flat_map(|commit| {
                        commit.pending_file_versions.values().map(String::as_str)
                    }),
                )
                .collect();
        Ok(Self {
            engine,
            registry,
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
    pub(in crate::engine) fn project(
        &self,
        successor: &VerifiedDiskRegistry,
        scope: &NamespaceScope,
    ) -> ProjectionResult<WriteProjection<'s, 'e>> {
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
        let mask = self
            .state
            .scopes
            .get(&scope.key()?)
            .ok_or(ProjectionError::BaselineMismatch)?;
        let namespace = self.source.preview.namespace(scope.borrowed())?;
        Ok(WriteProjection::from_verified(
            self.source,
            successor.commitment.clone(),
            &self.hash,
            mask,
            namespace.entries(),
        ))
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
                    object_hash("disk-occurrence-entry", entry)?,
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
            owner: namespace.namespace_owner().identity(),
            entries_hash: object_hash("disk-occurrence-entries", &entries)?,
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
