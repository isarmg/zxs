//! One version metadata constructor, with I/O and entropy retained at their old gates.
use crate::{
    ChecksumManifest, ContentHash, ContentState, Entry, FileVersion, Placement, Result, State,
};
use std::borrow::Cow;

/// Logical constructor input, not a source handle or a placement authorization.
pub(super) struct Input<'a> {
    pub(super) history: &'a str,
    pub(super) file: &'a str,
    pub(super) path: &'a name_policy::PortablePath,
    pub(super) primary: &'a str,
    pub(super) copies: u32,
    pub(super) sources: Vec<String>,
    pub(super) checkpoint: bool,
}

/// Sealed data facts. The caller remains responsible for the actual BODY proof.
pub(super) struct Content {
    pub(super) size: u64,
    pub(super) hash: ContentHash,
    pub(super) chunk_bytes: u32,
}

/// Calls remain lazy so rejected I/O, allocation, policy and route boundaries
/// retain their original order. Implementations are internal to workspace planning.
pub(super) trait Effects {
    fn version_id(&mut self) -> Result<storage_contract::VersionId>;
    fn created_at(&mut self) -> Result<String>;
    fn manifest_hash(&self) -> Result<String>;
    fn route(&self, primary: &str, copies: u32) -> Result<Vec<String>>;
    fn main_entries(&self, branch: &str) -> Result<Cow<'_, [Entry]>>;
    fn binding_generation(&self, slot: &str) -> Result<u64>;
    fn current(&self) -> Option<&crate::engine::CurrentNamespaces>;
}

pub(super) struct Live<'a, 'context> {
    context: &'a crate::engine::PlanningContext<'context>,
    manifest: &'a ChecksumManifest,
}

impl<'a, 'context> Live<'a, 'context> {
    pub(super) fn new(
        context: &'a crate::engine::PlanningContext<'context>,
        manifest: &'a ChecksumManifest,
    ) -> Self {
        Self { context, manifest }
    }
}

impl Effects for Live<'_, '_> {
    fn version_id(&mut self) -> Result<storage_contract::VersionId> {
        Ok(storage_contract::VersionId::generate()?)
    }

    fn created_at(&mut self) -> Result<String> {
        crate::container_storage::version_created_at()
    }

    fn manifest_hash(&self) -> Result<String> {
        Ok(blake3::hash(&self.manifest.encode()?).to_hex().to_string())
    }

    fn route(&self, primary: &str, copies: u32) -> Result<Vec<String>> {
        self.context.route(primary, copies)
    }

    fn main_entries(&self, branch: &str) -> Result<Cow<'_, [Entry]>> {
        self.context.current_branch_entries(branch).map(Cow::Borrowed)
    }

    fn binding_generation(&self, slot: &str) -> Result<u64> {
        Ok(self.context.disk(slot)?.binding_generation)
    }

    fn current(&self) -> Option<&crate::engine::CurrentNamespaces> {
        self.context.current_namespaces.map(AsRef::as_ref)
    }
}

pub(super) fn construct(
    state: &mut State,
    input: Input<'_>,
    content: Content,
    effects: &mut impl Effects,
) -> Result<(FileVersion, Placement)> {
    let Input { history, file, path, primary, copies, sources, checkpoint } = input;
    let version_id = effects.version_id()?;
    let storage_id =
        crate::container_storage::allocate_version_storage_id(state, file, version_id)?;
    let created_at = effects.created_at()?;
    let version = FileVersion {
        history_tree_id: history.into(),
        version_id: version_id.to_string(),
        file_id: file.into(),
        creation_primary_slot_id: primary.into(),
        logical_size: content.size,
        version_sequence: state.sequence,
        content_hash: content.hash,
        checksum_chunk_bytes: content.chunk_bytes,
        checksum_manifest_hash: effects.manifest_hash()?,
        creation_name: path.file_name().into(),
        storage_id,
        created_at,
        source_version_ids: sources,
        created_as_full_checkpoint: checkpoint,
    };
    let route = effects.route(primary, copies)?;
    if let Some(repository) = state
        .histories
        .get(history)
        .map(|history| history.repository_id.clone())
    {
        let main_branch = &state.repositories[&repository].main_branch_id;
        let current_entries = effects.main_entries(main_branch)?;
        crate::protection::apply_backup_paths(
            state,
            current_entries.as_ref(),
            &repository,
            file,
            path.as_str(),
            &route,
            effects.current(),
        )?;
    }
    let placement = Placement {
        version_id: version.version_id.clone(),
        primary_slot_id: primary.into(),
        target_copies: copies,
        route_slot_ids: route.clone(),
        topology_generation: state.topology_generation,
        binding_generations: route
            .iter()
            .map(|slot| Ok((slot.clone(), effects.binding_generation(slot)?)))
            .collect::<Result<_>>()?,
    };
    state
        .versions
        .insert(version.version_id.clone(), version.clone());
    state
        .placements
        .insert(version.version_id.clone(), placement.clone());
    state
        .content_states
        .insert(version.version_id.clone(), ContentState::Available);
    Ok((version, placement))
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/business/workspace_version_build_tests.rs"]
mod tests;
