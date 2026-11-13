//! Full upload generations, working files and atomic content publication.
pub(super) mod file_location;
use super::*;
use crate::{
    disk::*,
    engine::{MutationPlan, Prepared, ReadSource},
};
use json_codec::json;
use platform_io::fs_safety::EntryName;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::Path,
};

fn preparation_fs_error(error: platform_io::fs_safety::Error) -> Error {
    let message = error.to_string();
    Error::from(error).context(storage_contract::ErrorCode::UnsafePath, message)
}
struct UploadPreparationClaim {
    workspace: String,
    claims: std::sync::Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
}
impl Drop for UploadPreparationClaim {
    fn drop(&mut self) {
        self.claims
            .lock()
            .expect("invariant: upload preparation lock poisoned")
            .remove(&self.workspace);
    }
}

pub struct UploadPreparation {
    workspace: Workspace,
    item: String,
    path: name_policy::PortablePath,
    io: UploadIo,
    expected_content: Option<(u64, ContentHash)>,
    generation: u64,
    base_version_id: Option<String>,
    directory: std::path::PathBuf,
    disk_resources: std::sync::Arc<DiskResources>,
    target: std::path::PathBuf,
    previous: Option<StagedFile>,
    existing: bool,
    maximum: u64,
    max_file_bytes: u64,
    max_workspace_bytes: u64,
    chunk_bytes: u32,
    claim: UploadPreparationClaim,
    // Cross-disk writer fencing stays separate from the target upload lease.
    // Revoking this upload must not release another disk's fencing ownership.
    _locks: WriterLocks,
}

pub struct PreparedUpload {
    plan: UploadPreparation,
    size: u64,
    hash: ContentHash,
}

impl UploadPreparation {
    fn target_name(&self) -> Result<EntryName> {
        EntryName::new(
            self.target
                .file_name()
                .expect("invariant: upload target has a filename"),
        )
        .map_err(preparation_fs_error)
    }

    fn verify_content(&self, size: u64, hash: &ContentHash) -> Result<()> {
        if self
            .expected_content
            .as_ref()
            .is_some_and(|(expected_size, expected_hash)| {
                *expected_size != size || expected_hash != hash
            })
        {
            return Err(Error::new(
                storage_contract::ErrorCode::ChecksumMismatch,
                "staged bytes differ from the verified upload",
            ));
        }
        Ok(())
    }

    /// Full-file I/O runs here, with no Engine reference or mutex guard.
    pub fn execute(self) -> Result<PreparedUpload> {
        let _keep_claim = &self.claim;
        if let Some(old) = self
            .previous
            .as_ref()
            .filter(|old| old.dirty_generation == self.generation)
        {
            let (size, hash) = self.io.fingerprint_source(self.max_file_bytes)?;
            self.verify_content(size, &hash)?;
            if size != old.size || hash != old.content_hash {
                return Err(Error::new(
                    storage_contract::ErrorCode::UploadGenerationConflict,
                    "one upload generation cannot name different bytes",
                ));
            }
            if old.source_path != self.target {
                return Err(Error::new(
                    storage_contract::ErrorCode::WorkspaceChanged,
                    "retained upload path differs from its generation",
                ));
            }
            self.io
                .verify_retained(&self.target_name()?, size, &hash, self.max_file_bytes)?;
            return Ok(PreparedUpload {
                plan: self,
                size,
                hash,
            });
        }
        if self.existing {
            let incoming = self.io.fingerprint_source(self.max_file_bytes)?;
            self.verify_content(incoming.0, &incoming.1)?;
            self.io
                .verify_retained(
                    &self.target_name()?,
                    incoming.0,
                    &incoming.1,
                    self.max_file_bytes,
                )
                .map_err(|error| {
                    if error.code() == storage_contract::ErrorCode::ChecksumMismatch {
                        error.context(
                            ErrorCode::UploadGenerationConflict,
                            "one upload generation cannot name different bytes",
                        )
                    } else {
                        error
                    }
                })?;
            return Ok(PreparedUpload {
                plan: self,
                size: incoming.0,
                hash: incoming.1,
            });
        }
        let scratch_name = EntryName::new(format!(".zxs-upload-{}.prepared", new_id("req")?))
            .map_err(preparation_fs_error)?;
        let (size, hash) = self.io.copy(scratch_name, self.chunk_bytes, self.maximum)?;
        self.verify_content(size, &hash)?;
        Ok(PreparedUpload {
            plan: self,
            size,
            hash,
        })
    }
}

pub(crate) struct WorkspaceMutation {
    pub mutation: MutationPlan,
    pub workspace: Workspace,
    pub request: CommitRequest,
    pub wire: Option<crate::wire::WireCommit>,
}

// The native request envelope is shared by first publication and receipt replay.
// Workspace state/fencing may advance after publication; the sealed files and
// namespace input remain frozen and must still reproduce the original digest.
fn workspace_commit_input(
    workspace: &Workspace,
    request: &CommitRequest,
) -> Result<json_codec::Value> {
    let sealed = object_hash(
        "workspace",
        &json!({"workspace_id":workspace.workspace_id,"base_head":workspace.base_head,"files":workspace.files.values().map(|f|json!({"item_id":f.item_id,"path":f.path,"hash":f.content_hash,"size":f.size.to_string(),"generation":f.dirty_generation.to_string()})).collect::<Vec<_>>(),"namespace_operations":workspace.namespace_operations}),
    )?;
    let mut request = request.clone();
    request.request_digest = None;
    crate::persist::container_edits::bind_workspace_input(
        json!({"request":request,"sealed_manifest_hash":sealed}),
        workspace.container_draft_commit.as_ref(),
    )
}

pub(crate) fn native_commit_digest(
    workspace: &Workspace,
    request: &CommitRequest,
) -> Result<String> {
    object_hash(
        "request",
        &json!({"kind":"workspace.commit","principal":request.principal,"body":workspace_commit_input(workspace, request)?}),
    ).map_err(Into::into)
}

fn validate_directory_selection_key(state: &State, workspace: &Workspace, key: &str) -> Result<()> {
    let branch = state.branches.get(&workspace.branch_id).ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::BranchNotFound,
            "workspace branch missing",
        )
    })?;
    let base = state.commits.get(&workspace.base_head).ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::StateInvalid,
            "workspace base missing",
        )
    })?;
    let current = state.commits.get(&branch.head_commit_id).ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::StateInvalid,
            "current namespace missing",
        )
    })?;
    if state.directory_selection_generations.is_empty() {
        return Ok(());
    }
    let changed = |id: &str| {
        state
            .directory_selection_generations
            .get(id)
            .is_some_and(|generation| {
                workspace
                    .directory_selection_generations
                    .get(id)
                    .copied()
                    .unwrap_or(0)
                    != *generation
            })
    };
    // Visit this path's roots and ancestors once, not the whole namespace for
    // every selection generation retained anywhere in the pool. Both views
    // matter: an ancestor can disappear or first appear after workspace open.
    let root_changed = state
        .repositories
        .values()
        .any(|repo| repo.history_tree_id == branch.history_tree_id && changed(&repo.repository_id));
    if root_changed
        || base.entries.iter().chain(&current.entries).any(|entry| {
            entry.kind == NamespaceEntryKind::Directory
                && (key == entry.lookup_key() || namespace::is_descendant(key, entry.lookup_key()))
                && changed(&entry.entry_id)
        })
    {
        return Err(Error::new(
            storage_contract::ErrorCode::DirectoryBranchConflict,
            "directory branch changed after the workspace began; staged bytes are preserved",
        ));
    }
    Ok(())
}

// Callers validate directory selections first, including for new file paths.
fn validate_file_context(
    state: &State,
    workspace: &Workspace,
    file: &str,
    key: &str,
) -> Result<()> {
    let selected =
        directory::branch_context_at_key(state, &workspace.branch_id, key).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::FileNotFound,
                "file context no longer exists",
            )
        })?;
    if selected.inherited_from.is_none()
        && (state.file_branch_aliases.contains_key(&selected.branch_id)
            || state
                .branches
                .get(&selected.branch_id)
                .is_none_or(|branch| branch.state != BranchState::Active))
    {
        return Err(Error::new(
            storage_contract::ErrorCode::BranchNotActive,
            "selected file branch is not active; select a writable branch",
        ));
    }
    let repo = repository::main_for_branch(state, &workspace.branch_id);
    let captured = repo.and_then(|r| {
        workspace
            .selected_file_contexts
            .get(&directory::context_key_from_key(&r.repository_id, key))
    });
    let changed = if let Some(captured) = captured {
        captured != &selected
    } else {
        workspace
            .selected_file_branches
            .get(file)
            .is_some_and(|id| id != &selected.branch_id)
    };
    if changed {
        return Err(Error::new(
            storage_contract::ErrorCode::FileBranchConflict,
            "file path branch or write intent changed after workspace began; staged bytes are preserved",
        ));
    }
    Ok(())
}

pub(crate) struct VersionBuild<'a> {
    pub(crate) history: &'a str,
    pub(crate) file_id: &'a str,
    pub(crate) path: &'a name_policy::PortablePath,
    pub(crate) source: &'a ReadSource,
    pub(crate) primary: &'a str,
    pub(crate) copies: u32,
    pub(crate) source_ids: Vec<String>,
    pub(crate) checkpoint: bool,
}

impl Engine {
    /// Check a captured destination before SMB reopens a successfully saved
    /// workspace. This also accepts committed workspaces as immutable receipts.
    /// The caller must retain this engine's workspace view and exclusive owner
    /// throughout admission; this borrowed view is not a reusable authority grant.
    pub fn validate_workspace_file_selection(
        &self,
        workspace: &Workspace,
        path: &name_policy::PortablePath,
    ) -> Result<()> {
        let key = path.key();
        validate_directory_selection_key(&self.state, workspace, key)?;
        if self.state.branches.get(&workspace.branch_id)
            .is_some_and(|branch| branch.file_id.is_some())
        {
            // Completion binds the just-published destination. Its body is no
            // longer the pre-save base; the saved context still fences reuse.
            let published;
            let baseline;
            let entries = if workspace.state == WorkspaceState::Committed {
                published = self.namespace_snapshot(&workspace.branch_id)?;
                published.entries()
            } else {
                baseline = self.workspace_namespace_entries(workspace)?;
                baseline.entries()
            };
            let projected = file_location::project(
                self.state(),
                self.admitted_current_namespaces()?,
                workspace,
                entries,
                path,
            )?;
            let entry = projected.binary_search_by(|entry| entry.lookup_key().cmp(key))
                .ok().map(|index| &projected[index])
                .ok_or(Error::PathChange(crate::PathChangeError::WorkspaceLocation))?;
            return validate_file_context(&self.state, workspace, &entry.file_id, key);
        }
        let baseline = self.workspace_namespace_entries(workspace)?;
        let tree = crate::NamespaceTree::from_published_entries(baseline.entries());
        let mut entry = tree.get_key(key);
        // First-created files are absent from the original base, but completion
        // records their published destination. The current entry supplies only
        // its identity; the saved context still decides whether it is writable.
        if entry.is_none()
            && repository::main_for_branch(&self.state, &workspace.branch_id).is_some_and(|repo| {
                workspace
                    .selected_file_contexts
                    .contains_key(&directory::context_key_from_key(&repo.repository_id, key))
            })
        {
            entry = Some(
                self.namespace_tree(&workspace.branch_id)?
                    .get_key(key)
                    .ok_or_else(|| {
                        Error::new(
                            storage_contract::ErrorCode::FileNotFound,
                            "published workspace file is absent",
                        )
                    })?,
            );
        }
        if let Some(entry) = entry {
            validate_file_context(&self.state, workspace, &entry.file_id, key)?;
        }
        Ok(())
    }
    pub fn create_workspace(&mut self, branch_id: &str, principal: &str) -> Result<Workspace> {
        self.create_workspace_with_intent(branch_id, principal, |_| Ok(()))
    }
    /// Retain the allocated identity before namespace admission or workspace I/O.
    /// A failed intent write prevents those writes; successful intent alone is
    /// not evidence that a workspace was created or that any content published.
    pub(crate) fn create_workspace_with_intent(
        &mut self,
        branch_id: &str,
        principal: &str,
        persist_intent: impl FnOnce(&Workspace) -> Result<()>,
    ) -> Result<Workspace> {
        Self::require_actor(principal)?;
        self.require_write()?;
        self.repository_for_branch(branch_id)?;
        if self
            .workspaces
            .values()
            .filter(|w| matches!(w.state, WorkspaceState::Open | WorkspaceState::Sealed))
            .count()
            >= self.config.max_workspaces
        {
            return Err(Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                "workspace count budget exhausted",
            ));
        }
        let branch = self.state.branches.get(branch_id).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::BranchNotFound,
                "branch missing",
            )
        })?;
        if branch.state != BranchState::Active {
            return Err(Error::new(
                storage_contract::ErrorCode::BranchArchived,
                "branch is not writable",
            ));
        }
        if branch
            .file_id
            .as_deref()
            .is_some_and(|file| crate::node_retirement_engine::is_terminal_file(&self.state, file))
        {
            return Err(Error::new(
                storage_contract::ErrorCode::FileNodeNotFound,
                "file identity is permanently terminated; retained branch HEADs are authentication evidence only",
            ));
        }
        let head_entries = self.list_tree_ref(branch_id)?;
        let context_repository = self.repository_for_branch(branch_id)?;
        let context_entries = if branch.file_id.is_some() {
            self.list_tree_ref(&context_repository.main_branch_id)?
        } else {
            head_entries
        };
        let selected_contexts = context_entries.iter()
            .filter(|_| branch.file_id.is_some() || context_repository.main_branch_id == branch_id)
            .filter(|entry| entry.kind == NamespaceEntryKind::File)
            .filter_map(|entry| {
                self.file_context_for_current_entry(&context_repository.repository_id, entry)
                    .filter(|context| branch.file_id.is_none() || context.branch_id == branch_id)
                    .map(|context| (
                        directory::context_key_from_key(
                            &context_repository.repository_id, entry.lookup_key(),
                        ),
                        context.into_owned(),
                    ))
            }).collect();
        let w = Workspace {
            container_draft_commit: None,
            workspace_id: new_id("ws")?,
            purpose: WorkspacePurpose::Normal,
            branch_id: branch_id.into(),
            principal: principal.into(),
            base_head: branch.head_commit_id.clone(),
            fencing_token: self.next_slot,
            files: BTreeMap::new(),
            selected_file_contexts: selected_contexts,
            selected_directory_branches: (*self.state.directory_selections).clone(),
            directory_selection_generations: (*self.state.directory_selection_generations).clone(),
            file_selection_generations: (*self.state.file_selection_generations).clone(),
            selected_file_branches: head_entries
                .iter()
                .filter(|entry| entry.kind == NamespaceEntryKind::File)
                .filter_map(|entry| {
                    super::file::selected_file_branch(&self.state, branch_id, &entry.file_id)
                        .map(|selected| (entry.file_id.clone(), selected.branch_id.clone()))
                })
                .collect(),
            namespace_operations: Vec::new(),
            created_at: now()?,
            state: WorkspaceState::Open,
        };
        persist_intent(&w)?;
        let baseline = self.admit_namespace_workspace(&w)?;
        self.persist_workspace(&w)?;
        if let Some(baseline) = baseline {
            self.workspace_namespace_baselines
                .insert(w.workspace_id.clone(), baseline);
        }
        self.workspaces.insert(w.workspace_id.clone(), w.clone());
        Ok(w)
    }
    fn writable_workspace(&self, id: &str) -> Result<Workspace> {
        self.require_write()?;
        let w = self.workspace(id)?;
        if w.state != WorkspaceState::Open {
            return Err(Error::new(
                storage_contract::ErrorCode::WorkspaceClosed,
                "workspace is not open",
            ));
        }
        if now()?.saturating_sub(w.created_at) > self.config.workspace_ttl_seconds {
            return Err(Error::new(
                storage_contract::ErrorCode::FencingTokenExpired,
                "workspace lease expired; pending bytes are preserved",
            ));
        }
        self.repository_for_branch(&w.branch_id)?;
        self.workspace_namespace_entries(&w)?;
        Ok(w)
    }
    pub fn stage_file(&mut self, workspace: &str, path: &str, source: &Path) -> Result<StagedFile> {
        let path = name_policy::PortablePath::parse(path)?;
        let item = self
            .workspace(workspace)?
            .files
            .values()
            .find(|f| f.path.key() == path.key())
            .map(|f| Ok(f.item_id.clone()))
            .unwrap_or_else(|| new_id("entry"))?;
        self.stage_file_as_parsed(workspace, &item, &path, source)
    }
    pub fn stage_file_as(
        &mut self,
        workspace: &str,
        item: &str,
        path: &str,
        source: &Path,
    ) -> Result<StagedFile> {
        let path = name_policy::PortablePath::parse(path)?;
        self.stage_file_as_parsed(workspace, item, &path, source)
    }
    fn stage_file_as_parsed(
        &mut self,
        workspace: &str,
        item: &str,
        path: &name_policy::PortablePath,
        source: &Path,
    ) -> Result<StagedFile> {
        let generation = self
            .workspace(workspace)?
            .files
            .get(item)
            .map(|f| f.dirty_generation.checked_add(1))
            .unwrap_or(Some(1))
            .ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::ResourceLimit,
                    "upload generation exhausted",
                )
            })?;
        let prepared = self
            .prepare_upload_file(
                workspace,
                item,
                path,
                open_regular(source)?,
                generation,
                None,
            )?
            .execute()?;
        self.finish_upload(prepared)
    }
    /// Persist an explicitly numbered HTTP upload generation. Retrying the same
    /// generation may only replay the same complete bytes; older files remain
    /// available until the workspace is explicitly cleaned up.
    fn validate_upload_target(
        &self,
        workspace: &str,
        item: &str,
        path: &name_policy::PortablePath,
        generation: u64,
    ) -> Result<(Workspace, Option<Entry>)> {
        if generation == 0 {
            return Err(Error::new(
                storage_contract::ErrorCode::InvalidRequest,
                "upload generation starts at one",
            ));
        }
        validate_id(item, "entry")?;
        let w = self.writable_workspace(workspace)?;
        if w.purpose == WorkspacePurpose::Normal {
            validate_directory_selection_key(&self.state, &w, path.key())?;
        }
        let key = path.key();
        // Reject invalid targets before publishing an immutable generation.
        // Otherwise an IS_DIRECTORY failure leaves {item}-{generation}.full
        // behind and a subsequent valid retry collides with those orphan bytes.
        let baseline = self.workspace_namespace_entries(&w)?;
        let base = file_location::project(
            self.state(), self.admitted_current_namespaces()?, &w,
            baseline.entries(), path,
        )?;
        if let Some(view) = &self.current_namespaces {
            view.validate_upload_identity(&base, key, item)?;
        }
        let base_entry = base.binary_search_by(|entry| entry.lookup_key().cmp(key))
            .ok().map(|index| base[index].clone());
        if w.purpose == WorkspacePurpose::Normal
            && let Some(entry) = base_entry
                .as_ref()
                .filter(|e| e.kind == NamespaceEntryKind::File)
        {
            validate_file_context(&self.state, &w, &entry.file_id, path.key())?;
        }
        if base_entry
            .as_ref()
            .is_some_and(|entry| entry.kind != NamespaceEntryKind::File)
        {
            return Err(Error::new(
                storage_contract::ErrorCode::IsDirectory,
                "cannot upload over a directory",
            ));
        }
        if let Some(file_id) = self
            .state
            .branches
            .get(&w.branch_id)
            .and_then(|b| b.file_id.as_deref())
        {
            let current = base.iter().find(|e| e.file_id == file_id);
            if current.is_none_or(|entry| entry.lookup_key() != key) {
                return Err(Error::new(
                    storage_contract::ErrorCode::FileBranchScope,
                    "a file branch can only edit its own file",
                ));
            }
        }
        if self.workspaces.values().any(|other| {
            other.workspace_id != workspace
                && other.branch_id == w.branch_id
                && matches!(other.state, WorkspaceState::Open | WorkspaceState::Sealed)
                && other.files.values().any(|f| f.path.key() == key)
        }) {
            return Err(Error::new(
                storage_contract::ErrorCode::FileBusy,
                "another workspace owns this file",
            ));
        }
        if w.files
            .values()
            .any(|f| f.item_id != item && f.path.key() == key)
        {
            return Err(Error::new(
                storage_contract::ErrorCode::NameCollision,
                "workspace contains an equivalent file name",
            ));
        }
        let previous = w.files.get(item).cloned();
        if previous.as_ref().is_some_and(|f| f.path.key() != key) {
            return Err(Error::new(
                storage_contract::ErrorCode::ItemPathMismatch,
                "upload item is bound to a different path",
            ));
        }
        Ok((w, base_entry))
    }
    pub fn stage_upload(
        &mut self,
        workspace: &str,
        item: &str,
        path: &str,
        source: &Path,
        generation: u64,
    ) -> Result<StagedFile> {
        let prepared = self
            .prepare_upload(workspace, item, path, source, generation)?
            .execute()?;
        self.finish_upload(prepared)
    }

    /// Trusted local path adapter: open the source once at admission. Network
    /// transports must use `prepare_upload_file` with their verified length/hash.
    /// The returned claim serializes preparation within one workspace; different
    /// workspaces can copy/hash concurrently.
    pub fn prepare_upload(
        &self,
        workspace: &str,
        item: &str,
        path: &str,
        source: &Path,
        generation: u64,
    ) -> Result<UploadPreparation> {
        self.prepare_upload_file(
            workspace,
            item,
            &name_policy::PortablePath::parse(path)?,
            open_regular(source)?,
            generation,
            None,
        )
    }

    /// Keep the already opened source inode and bind copied bytes to the caller's
    /// verified stream. Path replacement cannot retarget a preparation.
    /// `None` is reserved for trusted local callers without a prior stream digest;
    /// transport callers must provide both the expected length and hash.
    pub fn prepare_upload_file(
        &self,
        workspace: &str,
        item: &str,
        path: &name_policy::PortablePath,
        source: std::fs::File,
        generation: u64,
        expected_content: Option<(u64, ContentHash)>,
    ) -> Result<UploadPreparation> {
        self.prepare_upload_source(
            workspace, item, path, UploadSource::Local(source), generation, expected_content,
        )
    }

    pub(crate) fn prepare_upload_managed(
        &self,
        workspace: &str,
        item: &str,
        path: &name_policy::PortablePath,
        source: DiskFile,
        generation: u64,
        expected_content: (u64, ContentHash),
    ) -> Result<UploadPreparation> {
        self.prepare_upload_source(
            workspace, item, path, UploadSource::Managed(source), generation,
            Some(expected_content),
        )
    }

    fn prepare_upload_source(
        &self,
        workspace: &str,
        item: &str,
        path: &name_policy::PortablePath,
        source: UploadSource,
        generation: u64,
        expected_content: Option<(u64, ContentHash)>,
    ) -> Result<UploadPreparation> {
        let expected = source.admitted_length()?;
        let (w, base_entry) = self.validate_upload_target(workspace, item, path, generation)?;
        if expected > self.config.max_file_bytes {
            return Err(Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                "workspace or file byte budget exceeded",
            ));
        }
        let previous = w.files.get(item).cloned();
        if previous
            .as_ref()
            .is_some_and(|old| generation < old.dirty_generation)
        {
            return Err(Error::new(
                storage_contract::ErrorCode::StaleUploadGeneration,
                "upload generation is older than staged bytes",
            ));
        }
        let directory = self.workspace_data_directory(&w)?;
        let mut claims = self
            .upload_preparations
            .lock()
            .expect("invariant: upload preparation lock poisoned");
        if !claims.insert(workspace.into()) {
            return Err(Error::new(
                storage_contract::ErrorCode::FileWriteBusy,
                "another upload is preparing this workspace",
            ));
        }
        drop(claims);
        let claim = UploadPreparationClaim {
            workspace: workspace.into(),
            claims: self.upload_preparations.clone(),
        };
        let disk_root = self.planning_context().workspace_data_root(&w)?;
        let io = UploadIo::with_source(disk_root, &directory, source)?;
        let (retained, generations) = io.retained_usage()?;
        let target = directory.join(format!("{item}-{generation}.full"));
        let target_name = EntryName::new(
            target
                .file_name()
                .expect("invariant: upload target has a filename"),
        )
        .map_err(preparation_fs_error)?;
        let existing = io.target_exists(&target_name)?;
        if generations > self.config.max_entries
            || (!existing && generations >= self.config.max_entries)
            || retained > self.config.max_workspace_bytes
            || (!existing && retained.saturating_add(expected) > self.config.max_workspace_bytes)
        {
            return Err(Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                "new upload and retained generations exceed the workspace byte or count budget",
            ));
        }
        Ok(UploadPreparation {
            workspace: w,
            item: item.into(),
            path: path.clone(),
            io,
            expected_content,
            generation,
            base_version_id: base_entry.map(|entry| entry.version_id),
            directory,
            disk_resources: std::sync::Arc::clone(&self.disk_resources),
            target,
            previous,
            existing,
            maximum: self
                .config
                .max_file_bytes
                .min(self.config.max_workspace_bytes.saturating_sub(retained)),
            max_file_bytes: self.config.max_file_bytes,
            max_workspace_bytes: self.config.max_workspace_bytes,
            chunk_bytes: self.config.checksum_chunk_bytes,
            claim,
            _locks: self.retain_writer_locks()?,
        })
    }

    /// Revalidate the frozen workspace and authorization before making copied
    /// bytes visible through its manifest. Stale work never replaces new state.
    pub fn finish_upload(&mut self, prepared: PreparedUpload) -> Result<StagedFile> {
        let plan = &prepared.plan;
        let (mut w, base_entry) = self.validate_upload_target(
            &plan.workspace.workspace_id,
            &plan.item,
            &plan.path,
            plan.generation,
        )?;
        if canonical_json(&w)? != canonical_json(&plan.workspace)?
            || !std::sync::Arc::ptr_eq(&self.disk_resources, &plan.disk_resources)
            || base_entry.map(|entry| entry.version_id) != plan.base_version_id
            || self.config.max_file_bytes != plan.max_file_bytes
            || self.config.max_workspace_bytes != plan.max_workspace_bytes
            || self.config.checksum_chunk_bytes != plan.chunk_bytes
        {
            return Err(Error::new(
                storage_contract::ErrorCode::WorkspaceChanged,
                "workspace changed during upload preparation; retry with current state",
            ));
        }
        if self.workspace_data_directory(&w)? != plan.directory {
            return Err(Error::new(
                storage_contract::ErrorCode::WorkspaceChanged,
                "workspace storage changed during upload preparation",
            ));
        }
        let current_directory = self
            .planning_context()
            .workspace_data_root(&w)?
            .directory_identity(&plan.directory)?;
        let replay = plan
            .previous
            .as_ref()
            .filter(|old| old.dirty_generation == plan.generation);
        let finish = if replay.is_some() {
            UploadFinish::Replay
        } else {
            UploadFinish::Publish {
                max_workspace_bytes: self.config.max_workspace_bytes,
                max_entries: self.config.max_entries,
            }
        };
        plan.io
            .finish(&plan.target_name()?, current_directory, finish)?;
        if let Some(old) = replay {
            return Ok(old.clone());
        }
        let staged = StagedFile {
            item_id: plan.item.clone(),
            path: plan.path.clone(),
            source_path: plan.target.clone(),
            size: prepared.size,
            content_hash: prepared.hash,
            dirty_generation: plan.generation,
            base_version_id: plan.base_version_id.clone(),
        };
        w.files.insert(plan.item.clone(), staged.clone());
        self.persist_workspace(&w)?;
        self.workspaces.insert(w.workspace_id.clone(), w);
        Ok(staged)
    }
    pub fn stage_bytes(&mut self, workspace: &str, path: &str, bytes: &[u8]) -> Result<StagedFile> {
        let mut f = tempfile::NamedTempFile::new_in(&self.root)?;
        f.write_all(bytes)?;
        f.as_file().sync_all()?;
        self.stage_file(workspace, path, f.path())
    }
    pub fn stage_range(
        &mut self,
        workspace: &str,
        item: &str,
        path: &str,
        offset: u64,
        bytes: &[u8],
        truncate_to: Option<u64>,
    ) -> Result<StagedFile> {
        use std::io::{Seek, SeekFrom};
        let w = self.writable_workspace(workspace)?;
        validate_id(item, "entry")?;
        let parsed = name_policy::PortablePath::parse(path)?;
        if w.purpose == WorkspacePurpose::Normal {
            validate_directory_selection_key(&self.state, &w, parsed.key())?;
        }
        let end = offset.checked_add(bytes.len() as u64).ok_or_else(|| {
            Error::new(storage_contract::ErrorCode::FileTooLarge, "range overflow")
        })?;
        if end > self.config.max_file_bytes
            || truncate_to.is_some_and(|n| n > self.config.max_file_bytes)
        {
            return Err(Error::new(
                storage_contract::ErrorCode::FileTooLarge,
                "range exceeds file budget",
            ));
        }
        let baseline = self.workspace_namespace_entries(&w)?;
        let base = file_location::project(
            self.state(), self.admitted_current_namespaces()?, &w,
            baseline.entries(), &parsed,
        )?;
        let base_entry = base.binary_search_by(|entry| entry.lookup_key().cmp(parsed.key()))
            .ok().map(|index| &base[index]);
        if w.purpose == WorkspacePurpose::Normal && let Some(entry) = base_entry {
            validate_file_context(&self.state, &w, &entry.file_id, parsed.key())?;
            let snapshot = self.namespace_snapshot(&w.branch_id)?;
            let current = file_location::project(
                self.state(), self.admitted_current_namespaces()?, &w,
                snapshot.entries(), &parsed,
            )?;
            let selected = current.binary_search_by(|current| {
                current.lookup_key().cmp(entry.lookup_key())
            }).ok().map(|index| &current[index]);
            if selected.is_none_or(|current| current.version_id != entry.version_id) {
                return Err(Error::new(
                    storage_contract::ErrorCode::FileVersionConflict,
                    "file changed before range staging; no source bytes were copied",
                ));
            }
        }
        let previous = w.files.get(item);
        let mut temporary = tempfile::NamedTempFile::new_in(&self.root)?;
        let expected = if let Some(previous) = previous {
            let source = self
                .planning_context()
                .staged_source(&w, previous)?
                .open()?;
            std::io::copy(
                &mut source.take(previous.size.saturating_add(1)),
                &mut temporary,
            )?;
            Some((previous.size, previous.content_hash))
        } else if let Some(entry) = base_entry {
            let mut source = self.open_content(&entry.version_id)?;
            std::io::copy(
                &mut (&mut source.file).take(source.size.saturating_add(1)),
                &mut temporary,
            )?;
            Some((source.size, source.content_hash))
        } else {
            None
        };
        if let Some((size, hash)) = expected {
            let digest = hash.digest();
            // Verify the copied snapshot before applying edits, so corruption
            // (including changes during copying) cannot become authenticated
            // untouched bytes in a new staged generation.
            platform_io::payload::verify_full(
                temporary.as_file_mut(),
                platform_io::payload::Expectation {
                    bytes: size,
                    blake3: Some(digest),
                },
            )
            .map_err(|error| {
                let message = error.to_string();
                Error::from(error).context(ErrorCode::ChecksumMismatch, message)
            })?;
        }
        temporary.seek(SeekFrom::Start(offset))?;
        temporary.write_all(bytes)?;
        if let Some(n) = truncate_to {
            temporary.as_file().set_len(n)?;
        }
        temporary.as_file().sync_all()?;
        self.stage_file_as_parsed(workspace, item, &parsed, temporary.path())
    }
    pub fn namespace_operation(
        &mut self,
        workspace: &str,
        operation: NamespaceOperation,
    ) -> Result<Workspace> {
        let mut w = self.writable_workspace(workspace)?;
        if self
            .state
            .branches
            .get(&w.branch_id)
            .is_some_and(|b| b.file_id.is_some())
        {
            return Err(Error::new(
                storage_contract::ErrorCode::FileBranchScope,
                "directory operations belong to the disk namespace",
            ));
        }
        if let Some(view) = &self.current_namespaces {
            view.require_namespace_operation(&operation)?;
        }
        w.namespace_operations.push(operation);
        self.persist_workspace(&w)?;
        self.workspaces.insert(workspace.into(), w.clone());
        Ok(w)
    }
    pub fn abort_workspace(&mut self, id: &str, principal: &str) -> Result<()> {
        let mut w = self.workspace(id)?;
        if w.principal != principal {
            return Err(Error::new(
                storage_contract::ErrorCode::PermissionDenied,
                "workspace belongs to another principal",
            ));
        }
        if self.blocked {
            return Err(Error::new(
                storage_contract::ErrorCode::InDoubt,
                "cannot abort while a decision is unresolved",
            ));
        }
        w.state = WorkspaceState::Aborted;
        w.fencing_token = w.fencing_token.saturating_add(1);
        self.persist_terminal_workspace(&w)
    }
    pub fn commit_workspace(&mut self, request: CommitRequest) -> Result<OperationResult> {
        self.commit_workspace_inner(request, None)
    }
    pub(crate) fn commit_workspace_inner(
        &mut self,
        request: CommitRequest,
        wire: Option<crate::wire::WireCommit>,
    ) -> Result<OperationResult> {
        let plan = self.plan_workspace_commit_inner(request, wire)?;
        self.persist_workspace_commit_request(&plan)?;
        let result = match plan.mutation {
            MutationPlan::Existing(result) => return Ok(result),
            MutationPlan::Prepared {
                next,
                prepared,
                result,
            } => {
                self.persist(*next, prepared, result.clone())?;
                result
            }
        };
        self.complete_workspace_commit(plan.workspace, result)
    }
    pub(crate) fn plan_workspace_commit_inner(
        &self,
        request: CommitRequest,
        wire: Option<crate::wire::WireCommit>,
    ) -> Result<WorkspaceMutation> {
        let workspace = self.workspace(&request.workspace_id)?;
        let replay = if self.state.operations.contains_key(&request.operation_id) {
            Some(self.operation_record(request.operation_id)?.into_result())
        } else {
            None
        };
        self.planning_context()
            .plan_workspace_commit_inner(workspace, request, wire, replay)
    }
    pub(crate) fn persist_workspace_commit_request(&self, plan: &WorkspaceMutation) -> Result<()> {
        if matches!(plan.mutation, MutationPlan::Existing(_)) {
            return Ok(());
        }
        self.write_workspace_commit_request(&plan.workspace, &plan.request, plan.wire.as_ref())
    }
    pub(crate) fn write_workspace_commit_request(
        &self,
        w: &Workspace,
        request: &CommitRequest,
        wire: Option<&crate::wire::WireCommit>,
    ) -> Result<()> {
        write_json(
            &self
                .root
                .join("workspaces")
                .join(&w.workspace_id)
                .join("commit-request.json"),
            &request,
            true,
        )?;
        self.planning_context().workspace_data_root(w)?.write_json(
            &self
                .workspace_data_directory(w)?
                .join("commit-request.json"),
            &request,
            true,
        )?;
        if let Some(wire) = wire {
            write_json(
                &self
                    .root
                    .join("workspaces")
                    .join(&w.workspace_id)
                    .join("wire-request.json"),
                &wire.body,
                true,
            )?;
            self.planning_context().workspace_data_root(w)?.write_json(
                &self.workspace_data_directory(w)?.join("wire-request.json"),
                &wire.body,
                true,
            )?;
        }
        Ok(())
    }
    /// Keep ordinary completion and restart reconstruction on one definition.
    pub(crate) fn refresh_workspace_contexts(state: &State, completed: &mut Workspace) {
        let file_branch = state.branches.get(&completed.branch_id)
            .filter(|branch| branch.file_id.is_some());
        let repository = repository::main_for_branch(state, &completed.branch_id)
            .or_else(|| {
                file_branch.and_then(|branch| {
                    state.repositories.values()
                        .find(|repo| repo.history_tree_id == branch.history_tree_id)
                })
            });
        if let Some(repo) = repository {
            for file in completed.files.values() {
                if let Some(context) =
                    directory::context_at_key(state, &repo.repository_id, file.path.key())
                {
                    // Only this workspace's published occurrence may refresh an
                    // independent file branch, not another alias or selection.
                    if file_branch.is_some() && context.branch_id != completed.branch_id {
                        continue;
                    }
                    completed.selected_file_contexts.insert(
                        directory::context_key_from_key(&repo.repository_id, file.path.key()),
                        context,
                    );
                }
            }
        }
    }

    pub(crate) fn complete_workspace_commit(
        &mut self,
        w: Workspace,
        result: OperationResult,
    ) -> Result<OperationResult> {
        let mut completed = w;
        // A first inherited save can create a public file branch. The same SMB
        // handle subsequently continues its own published destination; later
        // external selections are still fenced against this post-save receipt.
        Self::refresh_workspace_contexts(&self.state, &mut completed);
        completed.state = WorkspaceState::Committed;
        completed.fencing_token += 1;
        if let Err(source) = self.persist_terminal_workspace(&completed) {
            // Publication is already authoritative. Even pre-write budget/path
            // failures leave local completion unresolved, never a rejected save.
            self.blocked = true;
            return Err(Error::PublishedWorkspaceReconciliation(Box::new(
                PublishedWorkspaceReconciliation::new(result, &completed, source),
            )));
        }
        Ok(result)
    }
    pub(crate) fn build_version(
        &self,
        state: &mut State,
        pending: &mut Vec<Prepared>,
        input: VersionBuild<'_>,
    ) -> Result<FileVersion> {
        self.planning_context().build_version(state, pending, input)
    }
    pub(crate) fn build_copied_version(
        &self,
        state: &mut State,
        pending: &mut Vec<Prepared>,
        input: VersionBuild<'_>,
        expected: &FileVersion,
    ) -> Result<FileVersion> {
        self.build_version_checked(state, pending, input, Some(expected))
    }
    fn build_version_checked(
        &self,
        state: &mut State,
        pending: &mut Vec<Prepared>,
        input: VersionBuild<'_>,
        expected: Option<&FileVersion>,
    ) -> Result<FileVersion> {
        self.planning_context()
            .build_version_checked(state, pending, input, expected)
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/business/workspace_published_selection_tests.rs"]
mod published_selection_tests;
#[cfg(test)]
#[path = "../../../../test/engine/unit/business/workspace_selection_tests.rs"]
mod selection_tests;

impl crate::engine::PlanningContext<'_> {
    pub(crate) fn build_version(
        &self,
        state: &mut State,
        pending: &mut Vec<Prepared>,
        input: VersionBuild<'_>,
    ) -> Result<FileVersion> {
        self.build_version_checked(state, pending, input, None)
    }
}

impl crate::engine::PlanningContext<'_> {
    fn build_version_checked(
        &self,
        state: &mut State,
        pending: &mut Vec<Prepared>,
        input: VersionBuild<'_>,
        expected: Option<&FileVersion>,
    ) -> Result<FileVersion> {
        let VersionBuild {
            history,
            file_id,
            path,
            source,
            primary,
            copies,
            source_ids,
            checkpoint,
        } = input;
        let mut file = source.open()?;
        let size = file.metadata()?.len();
        if size > self.config.max_file_bytes {
            return Err(Error::new(
                storage_contract::ErrorCode::FileTooLarge,
                "file exceeds limit",
            ));
        }
        let mut full = blake3::Hasher::new();
        let mut chunks = Vec::new();
        let mut buf = vec![0; self.config.checksum_chunk_bytes as usize];
        let mut read = 0u64;
        loop {
            let mut used = 0;
            while used < buf.len() {
                let n = file.read(&mut buf[used..])?;
                if n == 0 {
                    break;
                }
                used += n;
            }
            if used == 0 {
                break;
            }
            read = read.checked_add(used as u64).ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::SourceChanged,
                    "source length overflow",
                )
            })?;
            if read > size || read > self.config.max_file_bytes {
                return Err(Error::new(
                    storage_contract::ErrorCode::SourceChanged,
                    "source grew while sealing",
                ));
            }
            full.update(&buf[..used]);
            chunks.push(*blake3::hash(&buf[..used]).as_bytes());
        }
        if read != size {
            return Err(Error::new(
                storage_contract::ErrorCode::SourceChanged,
                "source length changed while sealing",
            ));
        }
        let manifest = ChecksumManifest {
            chunk_bytes: self.config.checksum_chunk_bytes,
            logical_size: size,
            chunks,
        };
        let digest = full.finalize();
        let content_hash = ContentHash::from_digest(digest);
        if expected.is_some_and(|original| {
            original.logical_size != size || original.content_hash != content_hash
        }) {
            return Err(Error::new(
                storage_contract::ErrorCode::SourceChanged,
                "copied content differs from the published source version",
            ));
        }
        let version_id = storage_contract::VersionId::generate()?;
        let storage_id =
            crate::container_storage::allocate_version_storage_id(state, file_id, version_id)?;
        let created_at = crate::container_storage::version_created_at()?;
        let version = FileVersion {
            history_tree_id: history.into(),
            version_id: version_id.to_string(),
            file_id: file_id.into(),
            creation_primary_slot_id: primary.into(),
            logical_size: size,
            version_sequence: state.sequence,
            content_hash,
            checksum_chunk_bytes: self.config.checksum_chunk_bytes,
            checksum_manifest_hash: blake3::hash(&manifest.encode()?).to_hex().to_string(),
            creation_name: path.file_name().into(),
            storage_id,
            created_at,
            source_version_ids: source_ids,
            created_as_full_checkpoint: checkpoint,
        };
        let route = self.route(primary, copies)?;
        if let Some(repository) = state
            .histories
            .get(history)
            .map(|history| history.repository_id.clone())
        {
            let main_branch = &state.repositories[&repository].main_branch_id;
            let current_entries = self.current_branch_entries(main_branch)?;
            crate::protection::apply_backup_paths(
                state,
                current_entries,
                &repository,
                file_id,
                path.as_str(),
                &route,
                self.current_namespaces.map(AsRef::as_ref),
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
                .map(|slot| Ok((slot.clone(), self.disk(slot)?.binding_generation)))
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
        pending.push(Prepared {
            version: version.clone(),
            placement,
            source: source.clone(),
        });
        Ok(version)
    }
}

impl crate::engine::PlanningContext<'_> {
    pub(crate) fn plan_workspace_commit_inner(
        &self,
        workspace: Workspace,
        request: CommitRequest,
        wire: Option<crate::wire::WireCommit>,
        replay: Option<OperationResult>,
    ) -> Result<WorkspaceMutation> {
        if let Some(result) = replay {
            let digest = native_commit_digest(&workspace, &request)?;
            self.validate_workspace_commit_replay(&workspace, &request, &digest)?;
            self.require_saved_workspace_request(&workspace, &request)?;
            return Ok(WorkspaceMutation {
                mutation: MutationPlan::Existing(result),
                workspace,
                request,
                wire,
            });
        }
        let w = workspace;
        if let Some(binding) = &w.container_draft_commit {
            crate::engine::container_drafts::commit::validate_workspace(
                self.state,
                self.current_namespaces.map(std::sync::Arc::as_ref),
                self.current_branch_entries(&w.branch_id)?,
                &w,
                &request,
                binding,
            )?;
        }
        if w.purpose == WorkspacePurpose::HistoricalBranch {
            return Err(Error::new(
                storage_contract::ErrorCode::HistoricalSaveRequiresBranch,
                "commit a historical-save plan to create a full branch",
            ));
        }
        if let Some(view) = self.current_namespaces {
            view.validate_workspace(
                self.state,
                &w,
                self.workspace_namespace_baselines.get(&w.workspace_id),
            )?;
        }
        self.require_write()?;
        if !matches!(w.state, WorkspaceState::Open | WorkspaceState::Sealed) {
            return Err(Error::new(
                storage_contract::ErrorCode::WorkspaceClosed,
                "workspace is not writable",
            ));
        }
        if now()?.saturating_sub(w.created_at) > self.config.workspace_ttl_seconds {
            return Err(Error::new(
                storage_contract::ErrorCode::FencingTokenExpired,
                "workspace lease expired",
            ));
        }
        self.repository_for_branch(&w.branch_id)?;
        if w.principal != request.principal {
            return Err(Error::new(
                storage_contract::ErrorCode::PermissionDenied,
                "workspace belongs to another principal",
            ));
        }
        if w.fencing_token != request.fencing_token {
            return Err(Error::new(
                storage_contract::ErrorCode::FencingTokenInvalid,
                "workspace fencing token differs",
            ));
        }
        if request.expected_head != w.base_head {
            return Err(Error::new(
                storage_contract::ErrorCode::BaseHeadMismatch,
                "request does not identify the workspace base",
            ));
        }
        let input = match &wire {
            Some(wire) => wire.body.clone(),
            None => workspace_commit_input(&w, &request)?,
        };
        let digest = if let Some(wire) = &wire {
            wire.digest.clone()
        } else {
            object_hash(
                "request",
                &json!({"kind":"workspace.commit","principal":request.principal,"body":input}),
            )?
        };
        if request
            .request_digest
            .as_ref()
            .is_some_and(|d| d != &digest)
        {
            return Err(Error::new(
                storage_contract::ErrorCode::RequestDigestMismatch,
                "server request digest differs",
            ));
        }

        Engine::require_actor(&request.principal)?;
        let mutation = self.plan_new_mutation(
            "workspace.commit", &request.principal, request.operation_id, digest,
            |engine, state, pending| {
                let branch = state.branches.get(&w.branch_id).cloned()
                    .ok_or_else(|| Error::new(storage_contract::ErrorCode::BranchNotFound, "branch disappeared"))?;
                if branch.state != BranchState::Active {
                    return Err(Error::new(storage_contract::ErrorCode::BranchArchived, "branch is not writable"));
                }
                // Freeze the source records without copying the current Commit.
                // Helpers may insert candidate commits before namespace replay;
                // the draft below still owns its one mutable entries copy.
                let source_commits = state.commits.clone();
                let current = source_commits.get(&branch.head_commit_id)
                    .ok_or_else(|| Error::new(storage_contract::ErrorCode::StateInvalid, "branch HEAD missing"))?;
                let repo = engine.repository_for_branch(&w.branch_id)?;

                // One explicit mutation-local index serves both conflict checks
                // and staged replacements, reusing admitted source keys.
                let current_entries = engine.current_branch_entries(&w.branch_id)?;
                let named_base = engine.workspace_namespace_entries(&w)?;
                let base_entries = named_base.entries();
                let projected_current;
                let projected_base;
                let (current_entries, base_entries) = if let Some(file) = w.files.values().next() {
                    projected_current = file_location::project(
                        state, engine.current_namespaces.map(std::sync::Arc::as_ref),
                        &w, current_entries, &file.path,
                    )?;
                    projected_base = file_location::project(
                        state, engine.current_namespaces.map(std::sync::Arc::as_ref),
                        &w, base_entries, &file.path,
                    )?;
                    (projected_current.as_ref(), projected_base.as_ref())
                } else {
                    (current_entries, base_entries)
                };
                let mut draft = namespace::NamespaceDraft::new(current_entries.to_vec());
                if let Some(view) = engine.current_namespaces {
                    view.remove_hidden_contexts(state, &repo.repository_id, &w.branch_id);
                }
                for file in w.files.values() {
                    validate_directory_selection_key(state, &w, file.path.key())?;
                    let existing = draft.get_key(file.path.key());
                    if existing.map(|entry| &entry.version_id) != file.base_version_id.as_ref() {
                        return Err(Error::new(storage_contract::ErrorCode::FileVersionConflict, "file changed after this workspace began"));
                    }
                    if let Some(entry) = existing {
                        validate_file_context(state, &w, &entry.file_id,file.path.key())?;
                    }
                }
                {
                    for operation in &w.namespace_operations {
                        validate_directory_selection_key(state, &w, operation.key())?;
                        if let Some(destination) = &operation.destination() { validate_directory_selection_key(state, &w, destination.key())?; }
                        let key = operation.key().to_owned();
                        let prefix = format!("{key}/");
                        // A same-name replacement is not the entry this workspace
                        // opened. In particular, every directory has an empty
                        // version_id, so path/content alone cannot fence it.
                        let selected = |entries: &[Entry]| entries.iter()
                            .filter(|entry| entry.lookup_key() == key || entry.lookup_key().starts_with(&prefix))
                            .map(|entry| (entry.entry_id.clone(), entry.file_id.clone(), entry.kind, entry.path().to_owned(), entry.version_id.clone())).collect::<Vec<_>>();
                        if selected(current_entries) != selected(base_entries) {
                            return Err(Error::new(storage_contract::ErrorCode::NamespaceConflict, "namespace changed after the workspace began"));
                        }
                    }
                }
                let upload_locations = namespace::workspace_upload_locations(current_entries, &w)?;
                let commit_id = new_id("commit")?;
                let mut written_files = std::collections::BTreeSet::new();
                let mut written_paths = Vec::new();
                for file in w.files.values() {
                    let existing = draft.get_key(file.path.key()).cloned();
                    // An insensitive lookup is not a rename. Keep the existing
                    // display path; explicit namespace operations own renames.
                    let location = existing.as_ref().map(|entry| entry.location()).unwrap_or(&file.path);
                    let path = location.as_str();
                    let final_location = upload_locations.get(&file.item_id).expect("invariant: workspace preview covers every upload");
                    written_paths.push(final_location.as_str().to_owned());
                    // An unchanged save retains the selected content node and
                    // pending source. Fork only when the visible bytes change.
                    if existing.as_ref().is_some_and(|entry| {
                        state.versions.get(&entry.version_id).is_some_and(|version| {
                            version.content_hash == file.content_hash
                                && version.logical_size == file.size
                        })
                    }) {
                        continue;
                    }
                    let file_id = match existing.as_ref() {
                        Some(entry) => entry.file_id.clone(),
                        None => {
                            let file = new_id("file")?;
                            state.container_tags.insert(
                                file.parse()?,
                                storage_contract::file_container_tags::ContainerTags::empty(),
                            );
                            file
                        }
                    };
                    if existing.is_some() {
                        super::directory::prepare_file_write(engine,state,&w.branch_id,&file_id,location.key(),&request.principal,request.operation_id)?;
                    }
                    // A removed path is a genuinely new file. Raw path contexts
                    // and pending reservations cannot select its old identity.
                    let file_context = existing.as_ref().and_then(|_| directory::branch_context_at_key(state,&w.branch_id,location.key()));
                    let pending_source = file_context.as_ref().and_then(|c|state.branches.get(&c.branch_id)).and_then(|b|b.pending_file_versions.get(&file_id)).cloned()
                        .or_else(||existing.as_ref().and_then(|_| directory::unwritten_source_at(state,&w.branch_id,location.key()).map(str::to_owned)));
                    draft.ensure_parents(existing.as_ref().map(|entry| entry.location()).unwrap_or(&file.path))?;
                    let source_ids:Vec<String> = pending_source.as_ref().cloned()
                        .or_else(|| existing.as_ref().map(|entry| entry.version_id.clone()))
                        .into_iter().collect();
                    let (primary, copies) = if let Some(placement) = wire.as_ref().and_then(|wire| wire.placements.get(&file.item_id)).cloned() {
                        placement
                    } else {
                        engine.new_file_placement_for_file(&repo.repository_id,Some(&file_id), path)?
                    };
                    let source = engine.staged_source(&w, file)?;
                    let version = engine.build_version(
                        state, pending, crate::business::VersionBuild { history: &branch.history_tree_id, file_id: &file_id, path: location, source: &source, primary: &primary, copies, source_ids: source_ids.clone(), checkpoint: pending_source.is_some() },
                    )?;
                    if version.logical_size != file.size || version.content_hash != file.content_hash {
                        return Err(Error::new(storage_contract::ErrorCode::ChecksumMismatch, "workspace source changed after sealing"));
                    }
                    if let Some(node)=file_context.as_ref().and_then(|c|c.node_id.as_ref()) {
                        file_paths::source_node(state,&file_id,source_ids.first().map(String::as_str).unwrap_or(""),node)?;
                        state.file_node_parents.insert(version.version_id.clone(),vec![node.clone()]);
                    }
                    if repo.main_branch_id == w.branch_id
                        && final_location.as_str() == path
                        && let Some(context) = state.file_contexts.get_mut(&directory::context_key_from_key(&repo.repository_id, location.key()))
                    {
                        context.node_id = None;
                    }
                    let location = location.clone();
                    let entry = Entry { entry_id: existing.as_ref().map(|entry| entry.entry_id.clone()).unwrap_or_else(|| file.item_id.clone()), path: location, kind: NamespaceEntryKind::File, file_id: file_id.clone(), version_id: version.version_id, size: version.logical_size };
                    draft.replace(entry);
                    state.file_locations.entry(file_id.clone()).or_insert_with(||final_location.as_str().to_owned());
                    state.file_names.entry(file_id.clone()).or_insert_with(||final_location.file_name().into());
                    written_files.insert(file_id);
                }
                let mut entries = draft.into_entries();
                let deleted = namespace::apply_current_workspace_namespace(
                    state, &repo, &branch, &mut entries, current_entries,
                    &w.namespace_operations,
                )?;
                let renamed_directories=directory::prepare_current_renamed_directories(state,&repo.repository_id,current_entries,&entries,&w.namespace_operations)?;
                let path_changes = super::directory::namespace_locations(state,&repo.repository_id,current_entries,&entries,&w.namespace_operations,&request.principal,request.operation_id)?;
                validate_entries(&mut entries, engine.config.max_entries)?;
                if let Some(view) = engine.current_namespaces { view.validate_workspace_draft(engine.state, state, &w, &entries)?; }
                if entries.as_slice() == current_entries && deleted.is_empty() {
                    let result = crate::persist::path_changes::attach(json!({"commit_id":current.commit_id,"branch_id":branch.branch_id,"history_tree_id":branch.history_tree_id,"no_change":true}), path_changes)?;
                    if let Some(binding) = &w.container_draft_commit {
                        crate::engine::container_drafts::commit::apply(state, binding)?;
                    }
                    return crate::persist::container_edits::bind_workspace_input(
                        result, w.container_draft_commit.as_ref(),
                    );
                }
                let live_files = entries.iter().filter(|entry| entry.kind == NamespaceEntryKind::File)
                    .map(|entry| entry.file_id.clone()).collect::<std::collections::BTreeSet<_>>();
                let mut snapshot_pending = current.pending_file_versions.clone();
                snapshot_pending.retain(|file,_| live_files.contains(file) && !written_files.contains(file));
                state.commits.insert(commit_id.clone(), Commit {
                    namespace_deletions: namespace::deletion_record(deleted, now()?),
                    commit_id: commit_id.clone(), history_tree_id: branch.history_tree_id.clone(),
                    kind: "content".into(), parent_commit_ids: vec![current.commit_id.clone()],
                    tree_id: new_id("tree")?, sequence: state.sequence, operation_id: request.operation_id,
                    author: request.principal.clone(), message: request.message.clone(), entries, pending_file_versions: snapshot_pending, provenance: None,
                });
                let target = state.branches.get_mut(&w.branch_id).expect("branch checked");
                target.head_commit_id = commit_id.clone();
                target.generation = target.generation.checked_add(1).ok_or_else(|| Error::new(storage_contract::ErrorCode::ResourceLimit, "branch generation exhausted"))?;
                target.pending_file_versions.retain(|file, _| live_files.contains(file) && !written_files.contains(file));
                if !written_files.is_empty(){target.file_node_id=None;}
                if branch.file_id.is_some() {
                    if let Some(path)=w.files.values().find(|file|directory::context_at_key(state,&repo.repository_id, file.path.key()).is_some_and(|c|c.branch_id==w.branch_id&&c.inherited_from.is_none())).map(|f|f.path.clone()) {
                        super::file::publish_selected_file_at_projected(state,&w.branch_id,Some(&path),&request.principal,request.operation_id,engine.current_namespaces.map(std::sync::Arc::as_ref))?;
                    }
                } else if repo.main_branch_id == w.branch_id {
                    super::file::synchronize_directory_edits(state, &commit_id, &written_paths, &request.principal, request.operation_id)?;
                    let changed_paths = written_paths.into_iter()
                        .chain(w.namespace_operations.iter().flat_map(|n| std::iter::once(n.path().to_owned()).chain(n.destination().map(|path|path.as_str().to_owned()))))
                        .chain(renamed_directories)
                        .chain(path_changes.iter().flat_map(|fact| {
                            [fact.before.path.as_str().to_owned(), fact.after.path.as_str().to_owned()]
                        }))
                        .collect::<Vec<_>>();
                    super::directory::record_projected_directory_edits(state, &repo.repository_id, &changed_paths, None, engine.current_namespaces.map(std::sync::Arc::as_ref))?;
                }
                let result = crate::persist::path_changes::attach(json!({"commit_id":commit_id,"branch_id":w.branch_id,"history_tree_id":branch.history_tree_id}), path_changes)?;
                if let Some(binding) = &w.container_draft_commit {
                    crate::engine::container_drafts::commit::apply(state, binding)?;
                }
                crate::persist::container_edits::bind_workspace_input(
                    result, w.container_draft_commit.as_ref(),
                )
            },
        )?;
        Ok(WorkspaceMutation {
            mutation,
            workspace: w,
            request,
            wire,
        })
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/business/upload_admission_tests.rs"]
mod upload_admission_tests;
