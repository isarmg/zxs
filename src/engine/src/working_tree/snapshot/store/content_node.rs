//! Incomplete nodes are not snapshots. Preparation copies immutable originals
//! and verifies the reserved sparse inodes, but cannot move any of them.
use super::*;
use crate::directory_worktree::ledger::EditRecord;
use crate::directory_worktree::store::HeldSparseWorktree;
use crate::persist::snapshot_worktree::content_batch::PreparedNode;
use crate::snapshot_worktree::OriginalContentInput;
use std::collections::BTreeMap;
use storage_contract::working_tree::DirectoryId;

type Store<T> = Result<T, SnapshotStoreError>;

pub(crate) struct HeldContentNode {
    preparation: DirectoryId,
    node: HeldSnapshotNode,
}
impl HeldContentNode {
    pub(crate) fn stage(
        input: &OriginalContentInput, permit: &crate::engine::BudgetedOriginalContent,
    ) -> Result<Self, SnapshotPreparationError> {
        let rejected = SnapshotPreparationError::Rejected;
        let (source, repository, sparse) = input.sources();
        permit.require(input).map_err(SnapshotStoreError::Disk).map_err(rejected)?;
        let repo = repository.batch_repository(source).map_err(rejected)?;
        repository.require_free_content_node(input.manifest().node_id()).map_err(rejected)?;
        let manifest = input.manifest().clone();
        let preparation = input.preparation();
        let name = layout::batch(preparation);
        let reference = manifest.reference();
        let _activity = repo.retain_io_activity().map_err(SnapshotStoreError::Disk).map_err(rejected)?;
        let (node, id) = match repo.create_private_entry(&name) {
            DirectoryCreateOutcome::Rejected(error) => return Err(rejected(SnapshotStoreError::Directory(error))),
            DirectoryCreateOutcome::Uncertain { identity, source } =>
                return Err(incomplete(reference, identity, SnapshotStoreError::Directory(source))),
            DirectoryCreateOutcome::Created { directory, identity, durability } => {
                durability.map_err(|e| incomplete(reference, Some(identity), SnapshotStoreError::Directory(e)))?;
                (directory, identity)
            }
        };
        let write = || -> Store<Self> {
            let (files, files_id) = create_child(&node, &layout::FILES)?;
            let (_, trees_id) = create_child(&node, &layout::WORKTREES)?;
            let dirty: BTreeMap<_, _> = input.rows().iter().map(|r| (r.edit.material.version_id, &r.edit)).collect();
            for (version, payload) in manifest.payloads() {
                let material = &manifest.files()[version];
                if let Some(edit) = dirty.get(version) {
                    if edit.material != *material { return Err(SnapshotStoreError::Version); }
                    let verified = verify_sparse(sparse, edit)?.ok_or(SnapshotStoreError::Entries)?;
                    verified.sync_all().map_err(SnapshotStoreError::Io)?;
                    // The dirty destination remains absent, even for equal bytes.
                    continue;
                }
                if source.manifest().files().get(version) != Some(material) { return Err(SnapshotStoreError::Version); }
                files.copy_new_entry_verified_synced(&layout::file(payload), &mut source.open_payload(*version)?,
                    platform_io::payload::Expectation { bytes: material.size, blake3: Some(material.content_hash.digest()) })
                    .map_err(SnapshotStoreError::Copy)?;
            }
            node.write_relative_atomic(&layout::MANIFEST.as_relative(), manifest.bytes(), false)
                .map_err(SnapshotStoreError::Disk)?;
            node.sync_private_entry_matching(&layout::FILES, files_id).map_err(SnapshotStoreError::Directory)?;
            node.sync_private_entry_matching(&layout::WORKTREES, trees_id).map_err(SnapshotStoreError::Directory)?;
            repo.sync_private_entry_matching(&name, id).map_err(SnapshotStoreError::Directory)?;
            let binding = SnapshotNodeBinding { reference, repository_entity: source.binding().repository_entity,
                node_entity: local_entity(id), files_entity: local_entity(files_id), worktrees_entity: local_entity(trees_id) };
            if !binding.independent_node(source.binding()) { return Err(SnapshotStoreError::Entries); }
            let held = Self { preparation, node: HeldSnapshotNode::named_parts(repo, &name, binding)? };
            if held.node.manifest.bytes() != manifest.bytes() { return Err(SnapshotStoreError::Entries); }
            held.require_before(source, repository, sparse, input.rows().iter().map(|r| &r.edit))?;
            Ok(held)
        };
        write().map_err(|e| incomplete(reference, Some(id), e))
    }

    /// Borrowed only for owner-verified scope material. Not an installed node.
    pub(crate) fn staged_node(&self) -> &HeldSnapshotNode { &self.node }

    pub(crate) fn facts(&self) -> PreparedNode {
        PreparedNode { preparation: self.preparation, binding: self.node.binding,
            entries: self.node.manifest.entries().to_vec() }
    }

    pub(crate) fn require_before<'a>(&self, original: &HeldSnapshotNode,
        pinned: &PinnedSnapshotRepository, sparse: &HeldSparseWorktree,
        edits: impl Iterator<Item=&'a EditRecord>,
    ) -> Store<()> {
        let repository = pinned.batch_repository(original)?;
        let phase = self.position(repository)?;
        if phase != Position::Staged { return Err(SnapshotStoreError::Entries); }
        self.inspect(repository, sparse, edits, Requirement::Before)?;
        Ok(())
    }

    fn position(&self, repository: &DiskRoot) -> Store<Position> {
        let (staged, installed) = (
            directory_at(repository, &layout::batch(self.preparation), self.node.binding.node_entity)?,
            directory_at(repository, &layout::node(self.node.manifest.node_id()), self.node.binding.node_entity)?,
        );
        match (staged, installed) {
            (true, false) => Ok(Position::Staged), (false, true) => Ok(Position::Installed),
            _ => Err(SnapshotStoreError::Entries),
        }
    }

    fn inspect<'a>(&self, repository: &DiskRoot, sparse: &HeldSparseWorktree,
        edits: impl Iterator<Item=&'a EditRecord>, required: Requirement) -> Store<()> {
        let position = self.position(repository)?;
        if (required == Requirement::Before && position != Position::Staged)
            || (required == Requirement::Installed && position != Position::Installed)
        { return Err(SnapshotStoreError::Entries); }
        let name = match position { Position::Staged => layout::batch(self.preparation),
            Position::Installed => layout::node(self.node.manifest.node_id()) };
        let reopened = HeldSnapshotNode::named_parts(repository, &name, self.node.binding)?;
        if reopened.manifest.bytes() != self.node.manifest.bytes() { return Err(SnapshotStoreError::Entries); }
        let mut missing = BTreeSet::new();
        let mut dirty = BTreeSet::new();
        for edit in edits {
            let version = edit.material.version_id;
            if !dirty.insert(version) || reopened.manifest.files().get(&version) != Some(&edit.material) {
                return Err(SnapshotStoreError::Version);
            }
            let target = layout::file(&reopened.manifest.payloads()[&version]);
            let source = verify_sparse(sparse, edit)?;
            let destination = verify_optional(&reopened.files, &target, edit)?;
            match (source, destination) {
                (Some(_), None) if position == Position::Staged && required != Requirement::Installed => {
                    missing.insert(target);
                }
                (None, Some(_)) if required != Requirement::Before => {}
                _ => return Err(SnapshotStoreError::Entries),
            }
        }
        reopened.verify_inventory_missing(&missing)?;
        for version in reopened.manifest.files().keys().filter(|v| !dirty.contains(*v)) {
            reopened.open_payload(*version)?;
        }
        repository.private_entry_matching(&name, file_identity(reopened.binding.node_entity))
            .map_err(SnapshotStoreError::Directory)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Position { Staged, Installed }
#[derive(Clone, Copy, Eq, PartialEq)]
enum Requirement { Before, Decided, Installed }

fn directory_at(parent: &DiskRoot, name: &EntryName, expected: LocalEntityIdentity) -> Store<bool> {
    let metadata = parent.optional_metadata(&parent.path().join(name.as_path())).map_err(SnapshotStoreError::Disk)?;
    let Some(metadata) = metadata else { return Ok(false); };
    if !metadata.is_dir() || FileIdentity::from_metadata(&metadata) != file_identity(expected) {
        return Err(SnapshotStoreError::Entries);
    }
    parent.private_entry_matching(name, file_identity(expected)).map_err(SnapshotStoreError::Directory)?;
    Ok(true)
}

fn verify_sparse(sparse: &HeldSparseWorktree, edit: &EditRecord) -> Store<Option<ReadFile>> {
    verify_optional(sparse.directory(), edit.leaf.entry(), edit)
}

fn verify_optional(parent: &DiskRoot, name: &EntryName, edit: &EditRecord) -> Store<Option<ReadFile>> {
    let Some(mut body) = parent.open_optional_read_entry(name).map_err(SnapshotStoreError::Disk)? else { return Ok(None); };
    let proof = body.verify_full(platform_io::payload::Expectation {
        bytes: edit.material.size, blake3: Some(edit.material.content_hash.digest()),
    }).map_err(SnapshotStoreError::Payload)?;
    if proof.identity != file_identity(edit.entity) { return Err(SnapshotStoreError::Entries); }
    body.rewind().map_err(SnapshotStoreError::Io)?;
    Ok(Some(body))
}

impl HeldContentNode {
    /// An original Candidate's prepared binding describes exactly two allowed
    /// node positions. No scan, latest temporary or same-byte replacement.
    pub(crate) fn observe(
        original: &HeldSnapshotNode, pinned: &PinnedSnapshotRepository,
        sparse: &HeldSparseWorktree, plan: &crate::persist::snapshot_worktree::content_batch::Reservation,
        value: &crate::persist::snapshot_worktree::content_batch::Publication, finalized_or_published: bool,
    ) -> Store<Self> {
        let facts = &value.prepared_node;
        if !facts.binding.independent_node(original.binding()) || original.binding() != plan.source_binding
            || sparse.binding() != plan.instance || facts.binding.reference().node != plan.destination_node
        { return Err(SnapshotStoreError::Repository); }
        let repo = pinned.batch_repository(original)?;
        let temp = layout::batch(facts.preparation);
        let final_name = layout::node(plan.destination_node);
        let at_temp = directory_at(repo, &temp, facts.binding.node_entity)?;
        let at_final = directory_at(repo, &final_name, facts.binding.node_entity)?;
        let name = match (at_temp, at_final) {
            (true, false) => &temp, (false, true) if finalized_or_published => &final_name,
            _ => return Err(SnapshotStoreError::Entries),
        };
        let node = HeldSnapshotNode::named_parts(repo, name, facts.binding)?;
        if node.manifest.entries() != facts.entries || node.manifest.operation_id() != plan.publish_operation {
            return Err(SnapshotStoreError::Entries);
        }
        let held = Self { preparation: facts.preparation, node };
        held.inspect(repo, sparse, plan.rows.iter().map(|r| &r.edit),
            if finalized_or_published { Requirement::Decided } else { Requirement::Before })?;
        Ok(held)
    }

    /// Physical readback only. Equal bytes at another inode, both names or
    /// neither name are not an observed move. Opening these FDs cannot move it.
    pub(crate) fn scope_dirty_at_destination(&self, sparse: &HeldSparseWorktree, edit: &EditRecord) -> Store<bool> {
        let version = edit.material.version_id;
        let payload = self.node.manifest.payloads().get(&version).ok_or(SnapshotStoreError::Version)?;
        let target = layout::file(payload);
        match (verify_sparse(sparse, edit)?, verify_optional(&self.node.files, &target, edit)?) {
            (Some(_), None) => Ok(false), (None, Some(_)) => Ok(true), _ => Err(SnapshotStoreError::Entries),
        }
    }

    pub(crate) fn require_observed(&self, original: &HeldSnapshotNode, pinned: &PinnedSnapshotRepository,
        sparse: &HeldSparseWorktree, plan: &crate::persist::snapshot_worktree::content_batch::Reservation,
        cut: &crate::engine::ContentReplayCut) -> Store<()> {
        let need = if cut.must_be_installed() { Requirement::Installed } else if cut.can_have_moved() { Requirement::Decided }
            else { Requirement::Before };
        self.inspect(pinned.batch_repository(original)?, sparse, plan.rows.iter().map(|r| &r.edit), need)
    }

    pub(crate) fn require_installed(&self, original: &HeldSnapshotNode, pinned: &PinnedSnapshotRepository,
        sparse: &HeldSparseWorktree, plan: &crate::persist::snapshot_worktree::content_batch::Reservation,
    ) -> Store<()> {
        self.inspect(pinned.batch_repository(original)?, sparse, plan.rows.iter().map(|r| &r.edit), Requirement::Installed)
    }

    /// Only the actual native decision may call this. Each move independently
    /// checks both named parents and the original inode before and after
    /// NOREPLACE, and repairs its file and both parent syncs on replay.
    pub(crate) fn install(&self, decision: &crate::engine::FinalizedOriginalContent<'_>,
        original: &HeldSnapshotNode, pinned: &PinnedSnapshotRepository,
        sparse: &HeldSparseWorktree, plan: &crate::persist::snapshot_worktree::content_batch::Reservation,
        value: &crate::persist::snapshot_worktree::content_batch::Publication,
        cut: &mut impl FnMut(&str) -> crate::Result<()>,
    ) -> Store<HeldSnapshotNode> {
        decision.require(plan, value).map_err(SnapshotStoreError::Disk)?;
        if self.facts() != value.prepared_node || original.binding() != plan.source_binding || sparse.binding() != plan.instance {
            return Err(SnapshotStoreError::Entries);
        }
        let repo = pinned.batch_repository(original)?;
        self.inspect(repo, sparse, plan.rows.iter().map(|r| &r.edit), Requirement::Decided)?;
        for (i, row) in plan.rows.iter().enumerate() {
            let observed = HeldSparseWorktree::reopen(original, plan.instance)
                .map_err(SnapshotStoreError::Sparse)?;
            let edit = &row.edit;
            let target = layout::file(&self.node.manifest.payloads()[&edit.material.version_id]);
            let at = self.position(repo)?;
            let name = match at { Position::Staged => layout::batch(self.preparation),
                Position::Installed => layout::node(plan.destination_node) };
            let actual = repo.private_entry_matching(&name, file_identity(self.node.binding.node_entity))
                .map_err(SnapshotStoreError::Directory)?;
            let files = actual.private_entry_matching(&layout::FILES, file_identity(self.node.binding.files_entity))
                .map_err(SnapshotStoreError::Directory)?;
            observed.directory().resume_decided_file_move(edit.leaf.entry(), &files, &target,
                crate::disk::FrozenFileMove { bytes: edit.material.size, content_hash: edit.material.content_hash, entity: edit.entity })
                .map_err(SnapshotStoreError::Move)?;
            cut(&format!("after_worktree_content_move_{}", i + 1)).map_err(SnapshotStoreError::Disk)?;
        }
        // All leaves and their independent immutable copies must be complete
        // before exposing even the physical node's canonical name.
        self.node.verify_inventory()?;
        for version in self.node.manifest.files().keys() { self.node.open_payload(*version)?; }
        let temp = layout::batch(self.preparation);
        let name = layout::node(plan.destination_node);
        if self.position(repo)? == Position::Staged {
            repo.sync_private_entry_matching(&temp, file_identity(self.node.binding.node_entity))
                .map_err(SnapshotStoreError::Directory)?;
            repo.rename_noreplace_synced(&repo.path().join(temp.as_path()), &repo.path().join(name.as_path()))
                .map_err(SnapshotStoreError::Disk)?;
        }
        repo.sync_private_entry_matching(&name, file_identity(self.node.binding.node_entity))
            .map_err(SnapshotStoreError::Directory)?;
        self.require_installed(original, pinned, sparse, plan)?;
        let final_node = HeldSnapshotNode::reopen(repo, self.node.binding)?;
        cut("after_worktree_content_node").map_err(SnapshotStoreError::Disk)?;
        Ok(final_node)
    }
}
