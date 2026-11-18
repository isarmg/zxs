//! The destination half of an independent Worktree copy. It only produces
//! the same logical first FILE and Prepared FULL used by native ordinary
//! creation. It neither publishes this state nor removes a Worktree entry.
use super::{DraftError as Failure, PublishedContent, allocations::Allocation, invalid, trace, unborn};
use crate::business::namespace::working::WorkingNamespace;
use crate::engine::{Prepared, ReadSource};
use crate::snapshot_worktree::copy_source::PublishedSnapshotFile;
use crate::{Engine, Error, Result, State};
use std::sync::Arc;
use storage_contract::file_container::ContainerNamespace;
use storage_contract::file_draft::{BirthRequest, DraftLocation};
use storage_contract::{EntryId, OperationId, Principal, Sequence};

pub(crate) struct IndependentFirst {
    target: DraftLocation,
    namespace: ContainerNamespace,
    content: PublishedContent,
    next: State,
    physical: Prepared,
    source: Arc<PublishedSnapshotFile>,
}

impl IndependentFirst {
    /// An enclosing authenticated caller resolves original receipt/retry and
    /// exact occurrence inclusion first. These are independent proof duties:
    /// presenting a genuine .zxs body cannot itself choose copy vs relocate.
    pub(crate) fn plan(
        engine: &Engine, source: Arc<PublishedSnapshotFile>, destination: &BirthRequest,
        principal: &Principal, operation: OperationId,
    ) -> Result<Self> {
        let context = engine.planning_context();
        if context.state.operations.contains_key(&operation) { return Err(invalid()); }
        context.require_write()?;
        if context.current_namespaces.is_some() || context.config.generation().is_some()
            || !crate::container_storage::namespace_host::ordinary_path(&destination.path)
        {
            return Err(Error::FileDraft(Failure::UnsupportedScope));
        }
        source.require_context(&context).map_err(super::worktree::failure)?;
        let previous = context.state;
        let namespace = WorkingNamespace::from_main_state(previous, destination.repository, context.config.max_entries)?;
        let repo = previous.repositories.get(&destination.repository.to_string()).ok_or_else(invalid)?;
        let main = previous.branches.get(&repo.main_branch_id).ok_or_else(invalid)?;
        if main.head_commit_id != destination.expected_namespace_head.to_string() {
            return Err(Error::FileDraft(Failure::SourceChanged));
        }
        if namespace.entries().len() >= context.config.max_entries { return Err(Error::FileDraft(Failure::Size)); }
        namespace.vacant_parent(&destination.path)?;
        let owner = context.namespace_host_root(destination.repository, namespace.branch())?;
        let root = owner.binding();
        if root.registration != source.host().registration || root.root_entity != source.host().root_entity
            || root.roots.common_data != source.host().common_data || !root.source_chain.is_empty()
        {
            return Err(Error::FileDraft(Failure::UnsupportedScope));
        }
        owner.common()?;
        // A case-equivalent, noncurrent or initialized nested directory is not
        // the specified ordinary destination. Every ancestor is a registered
        // same-owner ordinary namespace entry; never plan beneath a mount.
        let mut ancestor = destination.path.parent();
        let mut parents = Vec::new();
        while let Some(path) = ancestor {
            let lookup = name_policy::LookupPath::parse(path.as_str())?;
            let row = namespace.get(&lookup).filter(|row| row.is_directory() && row.path() == &path)
                .ok_or_else(invalid)?;
            let id = row.entry_id()?;
            let registered = previous.namespace_directories.get(&id.to_string()).ok_or_else(invalid)?;
            let registered_owner = root.registration.disk_id.parse()?;
            if registered.repository != destination.repository || registered.branch != namespace.branch()
                || registered.owner_disk != registered_owner || registered.replicas.len() != 1
                || !registered.replicas.get(&registered_owner).is_some_and(|r|
                    r.root_id == root.root_id && r.lifecycle == storage_contract::working_tree::BindingLifecycle::Current)
                || previous.directory_git_repositories.contains_key(&id.to_string())
            {
                return Err(Error::FileDraft(Failure::UnsupportedScope));
            }
            ancestor = path.parent();
            parents.push((id,path));
        }
        let original = engine.snapshot_receipts().map_err(super::worktree::failure)?;
        if original.contains_nested_draft_host(destination.repository, namespace.branch(), &parents) {
            return Err(Error::FileDraft(Failure::UnsupportedScope));
        }

        let mut next = unborn::begin(previous, Sequence::new(context.next_slot))?;
        let file = crate::business::file_tags::new_file_identity(&mut next)?;
        let entry = EntryId::generate()?;
        unborn::require_fresh(previous, file, entry)?;
        let binding = namespace.namespace_for_birth(&destination.path, entry)?;
        let target = DraftLocation { repository: destination.repository, path: destination.path.clone(), entry, file };
        let original_body = source.material();
        let path = &target.path;
        let (primary, copies) = context.new_file_placement_for_file(
            &target.repository.to_string(), Some(&file.to_string()), path.as_str(),
        )?;
        if copies != 1 || primary != owner.disk().slot_id || file == original_body.file_id {
            return Err(Error::FileDraft(Failure::UnsupportedScope));
        }
        source.require_current(engine).map_err(super::worktree::failure)?;
        let mut prepared = Vec::new();
        let material = ReadSource::published_snapshot_file(Arc::clone(&source));
        let version = context.build_version(&mut next, &mut prepared, crate::business::VersionBuild {
            history: &namespace.history().to_string(), file_id: &file.to_string(), path,
            source: &material, primary: &primary, copies, source_ids: Vec::new(), checkpoint: false,
        })?;
        if version.version_id == original_body.version_id.to_string() || previous.versions.contains_key(&version.version_id)
            || version.logical_size != original_body.size || version.content_hash != original_body.content_hash
        {
            return Err(Error::FileDraft(Failure::WorkingBodyChanged));
        }
        let [physical]: [Prepared; 1] = prepared.try_into().map_err(|_| invalid())?;
        let mut allocations = Vec::new();
        super::unborn::first::publish_first_namespace(
            &namespace, &mut next, operation, &target, &version,
            "Copy published Worktree file into the ordinary directory", principal,
            &mut |prefix| {
                if allocations.len() == trace::MAX_ALLOCATIONS { return Err(Error::FileDraft(Failure::Size)); }
                let allocation = Allocation::generate(prefix)?;
                let text = allocation.text(prefix)?;
                allocations.push(allocation);
                Ok(text)
            },
        )?;
        trace::require_item(&allocations, false, true, &target)?;
        let content = PublishedContent { version, placement: physical.placement.clone(), allocations };
        crate::container_storage::capture_changed_names(previous, &mut next);
        // Physical registration remains with normal ContainerPreparation. It
        // alone can exclusively allocate the new FILE inode and owner rows.
        if next.file_containers.contains_key(&file) { return Err(invalid()); }
        Ok(Self { target, namespace: binding, content, next, physical, source })
    }

    pub(crate) fn target(&self) -> &DraftLocation { &self.target }
    pub(crate) fn namespace(&self) -> &ContainerNamespace { &self.namespace }
    pub(crate) fn content(&self) -> &PublishedContent { &self.content }

    /// Loan only to the combined operation, never as a standalone ordinary
    /// Workspace submission followed by unmapping. The caller still owns
    /// fresh authorization, physical budgets, full pre/post-fence, original
    /// receipt, and the single native decision covering both sides.
    pub(crate) fn into_parts(self, engine: &Engine) -> Result<(State, Prepared)> {
        self.source.require_current(engine).map_err(super::worktree::failure)?;
        Ok((self.next, self.physical))
    }
}
