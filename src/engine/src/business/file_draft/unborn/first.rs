//! First explicit historical node. It uses the existing Version constructor
//! and FILE branch initializer, never an empty VersionId or synthetic base.
use crate::business::file_draft::allocations::{Allocation, Replay};
use crate::business::file_draft::{DraftError as Failure, PublishedContent, invalid};
use crate::business::file_draft::unborn::selected;
use crate::business::namespace::working::WorkingNamespace;
use crate::business::workspace::version_build::{self, Content, Effects, Input};
use crate::container_storage::{PreparedUnbornFull, ReadWorkingDigest};
use crate::{Commit, Entry, Error, FileContext, NamespaceEntryKind, Result, State, ValidatedConfig};
use std::borrow::Cow;
use std::sync::Arc;
use storage_contract::file_container::FileContainerRecord;
use storage_contract::file_draft::{DraftLocation, WorkingRevision};
use storage_contract::{Generation, OperationId, Principal, Sequence, VersionId};

pub(crate) struct FirstCommit<'a> {
    pub(crate) target: &'a DraftLocation,
    pub(crate) expected: WorkingRevision,
    pub(crate) message: &'a str,
    pub(crate) principal: &'a Principal,
    pub(crate) content: &'a PublishedContent,
    pub(crate) read: &'a ReadWorkingDigest<'a>,
}

struct VersionFacts<'a> {
    previous: &'a State,
    content: &'a PublishedContent,
    disk: &'a crate::DiskConfig,
    consumed: bool,
}

impl Effects for VersionFacts<'_> {
    fn version_id(&mut self) -> Result<VersionId> {
        if self.consumed {
            return Err(invalid());
        }
        self.consumed = true;
        Ok(self.content.version.version_id.parse()?)
    }

    fn created_at(&mut self) -> Result<String> {
        Ok(self.content.version.created_at.clone())
    }

    fn manifest_hash(&self) -> Result<String> {
        Ok(self.content.version.checksum_manifest_hash.clone())
    }

    fn route(&self, primary: &str, copies: u32) -> Result<Vec<String>> {
        if primary != self.disk.slot_id || copies != 1 {
            return Err(invalid());
        }
        Ok(vec![self.disk.slot_id.clone()])
    }

    fn main_entries(&self, branch: &str) -> Result<Cow<'_, [Entry]>> {
        let branch = self.previous.branches.get(branch).ok_or_else(invalid)?;
        let current = self.previous.commits.get(&branch.head_commit_id).ok_or_else(invalid)?;
        let mut entries = current.entries.clone();
        let names = crate::business::namespace::container_name::current_names(
            self.previous, &entries,
        )?;
        crate::business::namespace::container_name::apply_file_names(&mut entries, &names)?;
        Ok(Cow::Owned(entries))
    }

    fn binding_generation(&self, slot: &str) -> Result<u64> {
        if slot != self.disk.slot_id {
            return Err(invalid());
        }
        Ok(self.disk.binding_generation)
    }

    fn current(&self) -> Option<&crate::engine::CurrentNamespaces> {
        None
    }
}

fn publish_first_namespace(
    namespace: &WorkingNamespace<'_>,
    next: &mut State,
    operation: OperationId,
    target: &DraftLocation,
    version: &crate::FileVersion,
    message: &str,
    principal: &Principal,
    ids: &mut impl FnMut(&str) -> Result<String>,
) -> Result<()> {
    let branch_id = namespace.branch().to_string();
    // The namespace fixed the original owner/branch. Its current accumulated
    // HEAD, not that original State's HEAD, preserves earlier selected items.
    let previous_branch = next.branches.get(&branch_id).ok_or_else(invalid)?;
    let previous = next.commits.get(&previous_branch.head_commit_id).ok_or_else(invalid)?;
    let first = Entry {
        entry_id: target.entry.to_string(),
        path: target.path.clone(),
        kind: NamespaceEntryKind::File,
        file_id: target.file.to_string(),
        version_id: version.version_id.clone(),
        size: version.logical_size,
    };
    // Only this explicit file enters history. Other current Unborn owner rows
    // remain current-only and cannot leak through a directory snapshot/export.
    let mut entries = previous.entries.clone();
    let names = crate::business::namespace::container_name::current_names(
        next, &entries,
    )?;
    crate::business::namespace::container_name::apply_file_names(&mut entries, &names)?;
    entries.push(first.clone());
    let count = entries.len();
    crate::business::namespace::validate_entries(&mut entries, count)?;
    let id = ids("commit")?;
    let main = Commit {
        namespace_deletions: None,
        commit_id: id.clone(),
        history_tree_id: namespace.history().to_string(),
        kind: "content".into(),
        parent_commit_ids: vec![previous.commit_id.clone()],
        tree_id: ids("tree")?,
        sequence: next.sequence,
        operation_id: operation,
        author: principal.as_str().into(),
        message: message.into(),
        entries,
        pending_file_versions: previous.pending_file_versions.clone(),
        provenance: None,
    };
    next.commits.insert(id.clone(), main.clone());
    let branch = next.branches.get_mut(&branch_id).ok_or_else(invalid)?;
    branch.head_commit_id = id;
    branch.generation = Generation::new(branch.generation).next()
        .map_err(|source| Error::FileDraft(Failure::PublicationGeneration(source)))?.get();
    branch.file_node_id = None;
    let file_branch = crate::business::file::ensure_shared_file_branch_with_ids(
        next, &main, &first, ids,
    )?;
    next.file_contexts.insert(
        crate::business::namespace::file_context::context_key_from_key(
            &target.repository.to_string(), target.path.key(),
        ),
        FileContext {
            node_id: None,
            selection_generation: 1,
            branch_id: file_branch,
            inherited_from: None,
        },
    );
    next.file_locations.insert(target.file.to_string(), target.path.as_str().into());
    next.file_names.insert(target.file.to_string(), target.path.file_name().into());
    Ok(())
}

fn require_first_source(
    config: &ValidatedConfig,
    record: &storage_contract::file_container::FileContainerRecord,
    target: &DraftLocation,
    read: &ReadWorkingDigest<'_>,
) -> Result<()> {
    // Current and directory-wide publication need their own complete native
    // transaction. This root-level first-file producer cannot mint either.
    if config.generation().is_some() || target.path.parent().is_some() {
        return Err(Error::FileDraft(Failure::UnsupportedScope));
    }
    let working = record.working.unborn().ok_or_else(invalid)?;
    read.require_source()?;
    let source = read.source();
    source.require_record(target.file, record)?;
    let manifest = match working.body() {
        storage_contract::file_draft::UnbornBody::Empty {} => None,
        storage_contract::file_draft::UnbornBody::Saved { manifest } => Some(manifest),
    };
    if source.file() != target.file
        || source.commitment() != manifest.map(|manifest| manifest.commitment())
        || source.generation() != manifest.map(|manifest| manifest.generation())
        || source.logical_size() != working.logical_size()
    {
        return Err(invalid());
    }
    Ok(())
}

pub(in crate::business::file_draft) fn project(
    previous: &State,
    config: &ValidatedConfig,
    namespace: &WorkingNamespace<'_>,
    next: &mut State,
    operation: OperationId,
    commit: &FirstCommit<'_>,
) -> Result<()> {
    let record = selected(namespace, commit.target, commit.expected)?;
    require_first_source(config, record, commit.target, commit.read)?;
    let working = record.working.unborn().ok_or_else(invalid)?;
    let source = commit.read.source();
    if next.file_containers.get(&commit.target.file) != Some(record)
        || commit.content.version.content_hash != commit.read.content_hash()
        || commit.content.version.logical_size != source.logical_size()
        || next.versions.contains_key(&commit.content.version.version_id)
        || next.placements.contains_key(&commit.content.version.version_id)
        || next.content_states.contains_key(&commit.content.version.version_id)
        || commit.content.allocations.len()
            > crate::business::file_draft::PublicationGraph::First.limit(next, commit.target)?
    {
        return Err(invalid());
    }
    replay_version(previous, config, next, commit.target.file, &commit.target.path,
        record, commit.content, commit.read)?;
    // Earlier selected items already own their allocations in this local
    // projection. Reusing one must fail rather than overwrite its graph row.
    let mut replay = Replay::new(next, &commit.content.allocations)?;
    publish_first_namespace(
        namespace, next, operation, commit.target, &commit.content.version,
        commit.message, commit.principal, &mut |prefix| replay.next(prefix),
    )?;
    replay.finish()?;
    next.file_containers.get_mut(&commit.target.file).ok_or_else(invalid)?.working =
        working.after_first_commit()
            .map_err(|source| Error::FileDraft(Failure::RevisionExhausted(source)))?;
    commit.read.require_source()
}

/// Shared first-Version reconstruction. The source must still be the actual
/// selected Unborn reader; the caller separately owns its namespace publication.
pub(crate) fn replay_version(
    previous: &State, config: &ValidatedConfig, next: &mut State,
    file: storage_contract::FileId, path: &name_policy::PortablePath,
    record: &FileContainerRecord, content: &PublishedContent, read: &ReadWorkingDigest<'_>,
) -> Result<()> {
    read.source().require_record(file, record)?;
    if content.version.logical_size != read.source().logical_size()
        || content.version.content_hash != read.content_hash()
        || next.versions.contains_key(&content.version.version_id)
    {
        return Err(invalid());
    }
    let owner = record.owner_disk.to_string();
    let disk = config.disks.iter().find(|disk| disk.disk_id == owner).ok_or_else(invalid)?;
    if disk.level != 1 || disk.upstream_slot_id.is_some() {
        return Err(Error::FileDraft(Failure::UnsupportedScope));
    }
    let mut facts = VersionFacts { previous, content, disk, consumed: false };
    let (version, placement) = version_build::construct(
        next, Input {
            history: &record.history_tree_id.to_string(), file: &file.to_string(), path,
            primary: &disk.slot_id, copies: 1, sources: Vec::new(), checkpoint: false,
        }, Content {
            size: read.source().logical_size(), hash: read.content_hash(),
            chunk_bytes: content.version.checksum_chunk_bytes,
        }, &mut facts,
    )?;
    if version != content.version || placement != content.placement {
        return Err(invalid());
    }
    Ok(())
}

/// The original first-file policy and material checks, before beginning a
/// projection. It borrows real prepared material; no digest/identity constructor
/// can replace its held source. This is not a receipt or publication proof.
pub(in crate::business::file_draft) struct FirstContentConstruction<'source> {
    namespace: WorkingNamespace<'source>,
    record: &'source FileContainerRecord,
    target: &'source DraftLocation,
    expected: WorkingRevision,
    message: &'source str,
    principal: &'source Principal,
    operation: OperationId,
    material: &'source Arc<PreparedUnbornFull>,
    read: ReadWorkingDigest<'source>,
    primary: name_policy::SlotId,
}

impl<'source> FirstContentConstruction<'source> {
    pub(crate) fn prepare(
        context: &'source crate::engine::PlanningContext<'_>,
        target: &'source DraftLocation,
        expected: WorkingRevision,
        message: &'source str,
        principal: &'source Principal,
        operation: OperationId,
        material: &'source Arc<PreparedUnbornFull>,
    ) -> Result<Self> {
        context.require_write()?;
        material.require_context(context)?;
        if context.current_namespaces.is_some()
            || context.state.operations.contains_key(&operation)
        {
            return Err(invalid());
        }
        crate::directory_conversion::require_import_not_stopped(context.state, operation)?;
        let namespace = WorkingNamespace::from_main_state(
            context.state, target.repository, context.config.max_entries,
        )?;
        let record = selected(&namespace, target, expected)?;
        let read = material.digest()?;
        require_first_source(context.config, record, target, &read)?;
        let (primary, copies) = context.new_file_placement_for_file(
            &target.repository.to_string(), Some(&target.file.to_string()), target.path.as_str(),
        )?;
        let disk = context.disk(&primary)?;
        if copies != 1 || disk.level != 1 || disk.upstream_slot_id.is_some()
            || disk.disk_id.parse::<storage_contract::DiskId>()? != record.owner_disk
        {
            return Err(Error::FileDraft(Failure::UnsupportedScope));
        }
        Ok(Self {
            namespace, record, target, expected, message, principal, operation,
            material, read, primary: name_policy::SlotId::parse(&primary)?,
        })
    }

    /// Append one real first-file construction to the caller's accumulating
    /// State. The caller owns begin/final validation and the eventual one native
    /// receipt. No independent FULL, payload, FILE or directory is written here.
    pub(crate) fn append(
        &self,
        context: &crate::engine::PlanningContext<'_>,
        next: &mut State,
        prepared: &mut Vec<crate::engine::Prepared>,
    ) -> Result<PublishedContent> {
        self.material.require_context(context)?;
        if !std::ptr::eq(self.namespace.state(), context.state.as_ref())
            || next.sequence != context.next_slot
            || next.file_containers.get(&self.target.file) != Some(self.record)
        {
            return Err(invalid());
        }
        let source = crate::engine::ReadSource::working_full(Arc::clone(self.material));
        let version = context.build_version(
            next, prepared,
            crate::business::VersionBuild {
                history: &self.record.history_tree_id.to_string(),
                file_id: &self.target.file.to_string(),
                path: &self.target.path, source: &source, primary: self.primary.as_str(), copies: 1,
                source_ids: Vec::new(), checkpoint: false,
            },
        )?;
        if version.logical_size != self.read.source().logical_size()
            || version.content_hash != self.read.content_hash()
        {
            return Err(Error::FileDraft(Failure::Body));
        }
        let placement = next.placements.get(&version.version_id).cloned().ok_or_else(invalid)?;
        let limit = crate::business::file_draft::PublicationGraph::First.limit(next, self.target)?;
        let mut allocations = Vec::new();
        publish_first_namespace(
            &self.namespace, next, self.operation, self.target, &version,
            self.message, self.principal,
            &mut |prefix| {
                if allocations.len() == limit { return Err(Error::FileDraft(Failure::Size)); }
                let allocation = Allocation::generate(prefix)?;
                let text = allocation.text(prefix)?;
                allocations.push(allocation);
                Ok(text)
            },
        )?;
        next.file_containers.get_mut(&self.target.file).ok_or_else(invalid)?.working =
            self.record.working.unborn().ok_or_else(invalid)?.after_first_commit()
                .map_err(|source| Error::FileDraft(Failure::RevisionExhausted(source)))?;
        self.material.require_context(context)?;
        Ok(PublishedContent { version, placement, allocations })
    }

    fn original_description<'a>(&'a self, content: &'a PublishedContent) -> FirstCommit<'a> {
        FirstCommit {
            target: self.target, expected: self.expected, message: self.message,
            principal: self.principal, content, read: &self.read,
        }
    }
}

/// Single-file publication keeps its one begin and complete canonical replay.
/// Batch callers may append to one next, but get no native admission from that
/// construction alone and must retain the complete selected set and materials.
pub(crate) fn build(
    context: &crate::engine::PlanningContext<'_>,
    target: &DraftLocation,
    expected: WorkingRevision,
    message: &str,
    principal: &Principal,
    operation: OperationId,
    material: &Arc<PreparedUnbornFull>,
) -> Result<(super::Projection, Vec<crate::engine::Prepared>, PublishedContent)> {
    let construction = FirstContentConstruction::prepare(
        context, target, expected, message, principal, operation, material,
    )?;
    let mut next = super::begin(context.state, Sequence::new(context.next_slot))?;
    let mut prepared = Vec::new();
    let content = construction.append(context, &mut next, &mut prepared)?;
    crate::container_storage::capture_changed_names(context.state, &mut next);
    let projected = super::project(
        context.state, context.config, operation, Sequence::new(context.next_slot),
        super::Edit::FirstCommit(construction.original_description(&content)),
        context.config.max_entries, context.config.max_file_bytes,
    )?;
    if projected.state() != &next { return Err(invalid()); }
    crate::file_deletion::freeze::verify(context.state, &next)?;
    crate::business::file_tags::preserve_existing(context.state, &next)?;
    material.require_context(context)?;
    Ok((projected, prepared, content))
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/business/unborn_first_construction.rs"]
mod tests;
