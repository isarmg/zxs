//! Full upload generations, working files and atomic content publication.
use super::*;
use crate::{
    disk::*,
    engine::{MutationPlan, Prepared, ReadSource},
    file_stamp::FileStamp,
};
use json_codec::json;
use platform_io::fs_safety::{
    EntryName, RelativePath,
    linux::{MountPolicy, OpenAt2Root},
};
use std::os::unix::ffi::OsStrExt;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::Path,
};

fn upload_fingerprint_file(input: std::fs::File, maximum: u64) -> Result<(u64, ContentHash)> {
    let mut digest = blake3::Hasher::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        let n = std::os::unix::fs::FileExt::read_at(&input, &mut buffer, size)?;
        if n == 0 {
            break;
        }
        size = size.checked_add(n as u64).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                "upload length overflow",
            )
        })?;
        if size > maximum {
            return Err(Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                "upload grew beyond file budget",
            ));
        }
        digest.update(&buffer[..n]);
    }
    Ok((size, ContentHash::from_digest(digest.finalize())))
}

fn preparation_fs_error(error: platform_io::fs_safety::Error) -> Error {
    let message = error.to_string();
    Error::from(error).context(storage_contract::ErrorCode::UnsafePath, message)
}
fn retained_upload_usage(directory: &OpenAt2Root) -> Result<(u64, usize)> {
    let mut retained = 0u64;
    let mut generations = 0usize;
    let entries =
        rustix::fs::Dir::read_from(directory.directory()).map_err(std::io::Error::from)?;
    for entry in entries {
        let entry = entry.map_err(std::io::Error::from)?;
        let path = Path::new(std::ffi::OsStr::from_bytes(entry.file_name().to_bytes()));
        if path
            .extension()
            .is_some_and(|ext| ext == "full" || ext == "prepared")
            // A crash during full_copy_held can leave its unpublished copy
            // temporary behind. It still consumes this workspace's capacity.
            || entry.file_name().to_bytes().starts_with(b".zxs-full-")
        {
            generations = generations.checked_add(1).ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::ResourceLimit,
                    "retained generation count overflow",
                )
            })?;
            let relative = RelativePath::new(path).map_err(preparation_fs_error)?;
            retained = retained
                .checked_add(
                    directory
                        .open_file(&relative, false)
                        .map_err(preparation_fs_error)?
                        .metadata()?
                        .len(),
                )
                .ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::ResourceLimit,
                        "retained workspace byte count overflow",
                    )
                })?;
        }
    }
    Ok((retained, generations))
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
    source: std::fs::File,
    expected_content: Option<(u64, ContentHash)>,
    generation: u64,
    base_version_id: Option<String>,
    directory: std::path::PathBuf,
    directory_handle: OpenAt2Root,
    disk_resources: std::sync::Arc<DiskResources>,
    target: std::path::PathBuf,
    previous: Option<StagedFile>,
    existing: bool,
    maximum: u64,
    max_file_bytes: u64,
    max_workspace_bytes: u64,
    chunk_bytes: u32,
    claim: UploadPreparationClaim,
    _locks: Vec<std::fs::File>,
}

pub struct PreparedUpload {
    plan: UploadPreparation,
    size: u64,
    // None exists only while the scratch cleanup guard owns an unfinished copy.
    hash: Option<ContentHash>,
    scratch: Option<std::path::PathBuf>,
    verified: Option<VerifiedUploadFile>,
}

struct VerifiedUploadFile {
    file: std::fs::File,
    stamp: FileStamp,
}
impl Drop for PreparedUpload {
    fn drop(&mut self) {
        if let Some(path) = &self.scratch
            && let Some(name) = path.file_name()
        {
            match rustix::fs::unlinkat(
                self.plan.directory_handle.directory(),
                name,
                rustix::fs::AtFlags::empty(),
            ) {
                Ok(()) | Err(rustix::io::Errno::NOENT) => (),
                Err(error) => tracing::error!(%error, "upload scratch cleanup failed"),
            }
        }
    }
}
impl UploadPreparation {
    fn retained_file(&self) -> Result<std::fs::File> {
        let relative =
            RelativePath::new(Path::new(self.target.file_name().expect("target filename")))
                .map_err(preparation_fs_error)?;
        self.directory_handle
            .open_file(&relative, false)
            .map_err(preparation_fs_error)
    }

    fn verify_retained(&self, size: u64, hash: &ContentHash) -> Result<VerifiedUploadFile> {
        let file = self.retained_file()?;
        let stamp = FileStamp::of(&file)?;
        let actual = upload_fingerprint_file(file.try_clone()?, self.max_file_bytes)?;
        if actual.0 != size || actual.1 != *hash || FileStamp::of(&file)? != stamp {
            return Err(Error::new(
                storage_contract::ErrorCode::ChecksumMismatch,
                "retained upload generation no longer matches its receipt",
            ));
        }
        Ok(VerifiedUploadFile { file, stamp })
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
            let (size, hash) =
                upload_fingerprint_file(self.source.try_clone()?, self.max_file_bytes)?;
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
            let retained = self.verify_retained(size, &hash)?;
            return Ok(PreparedUpload {
                plan: self,
                size,
                hash: Some(hash),
                scratch: None,
                verified: Some(retained),
            });
        }
        if self.existing {
            let incoming = upload_fingerprint_file(self.source.try_clone()?, self.max_file_bytes)?;
            self.verify_content(incoming.0, &incoming.1)?;
            let retained = self
                .verify_retained(incoming.0, &incoming.1)
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
                hash: Some(incoming.1),
                scratch: None,
                verified: Some(retained),
            });
        }
        let scratch = self
            .directory
            .join(format!(".zxs-upload-{}.prepared", new_id("req")));
        let mut prepared = PreparedUpload {
            plan: self,
            size: 0,
            hash: None,
            scratch: Some(scratch.clone()),
            verified: None,
        };
        let scratch_name = EntryName::new(scratch.file_name().expect("scratch filename"))
            .map_err(preparation_fs_error)?;
        let (size, hash, _, file) = full_copy_held(
            prepared.plan.source.try_clone()?,
            &prepared.plan.directory_handle,
            &scratch_name,
            prepared.plan.chunk_bytes,
            prepared.plan.maximum,
        )?;
        prepared.plan.verify_content(size, &hash)?;
        prepared.size = size;
        prepared.hash = Some(hash);
        prepared.verified = Some(VerifiedUploadFile {
            stamp: FileStamp::of(&file)?,
            file,
        });
        Ok(prepared)
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
    Ok(json!({"request":request,"sealed_manifest_hash":sealed}))
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

fn validate_directory_selection_key(
    state: &State,
    workspace: &Workspace,
    key: &str,
    source: &dyn crate::namespace::NamespaceSource,
) -> Result<()> {
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
    // The ancestors of one key are its path closure: one point descent per view.
    let root_changed = state
        .repositories
        .values()
        .any(|repo| repo.history_tree_id == branch.history_tree_id && changed(&repo.repository_id));
    let path_changed = |namespace: &storage_contract::CommitNamespace| -> Result<bool> {
        Ok(crate::namespace::NamespaceView::new(namespace, source)
            .ancestor_closure(key)?
            .iter()
            .any(|entry| entry.kind == NamespaceEntryKind::Directory && changed(&entry.entry_id)))
    };
    if root_changed || path_changed(&base.namespace)? || path_changed(&current.namespace)? {
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
    source: &dyn crate::namespace::NamespaceSource,
) -> Result<()> {
    let selected = directory::branch_context_at_key(state, &workspace.branch_id, key, source)?
        .ok_or_else(|| {
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
    pub fn validate_workspace_file_selection(
        &self,
        workspace: &str,
        path: &name_policy::PortablePath,
    ) -> Result<()> {
        let key = path.key();
        let workspace = self.workspace_view(workspace)?;
        self.with_namespace_source(|source| {
            validate_directory_selection_key(&self.state, &workspace, key, source)
        })?;
        let mut entry = match self.commit_namespace(&workspace.base_head) {
            Some(tree) => tree.get_key(key)?,
            None => None,
        };
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
                self.state
                    .branches
                    .get(&workspace.branch_id)
                    .and_then(|branch| self.commit_namespace(&branch.head_commit_id))
                    .map(|tree| tree.get_key(key))
                    .transpose()?
                    .flatten()
                    .ok_or_else(|| {
                        Error::new(
                            storage_contract::ErrorCode::FileNotFound,
                            "published workspace file is absent",
                        )
                    })?,
            );
        }
        if let Some(entry) = entry {
            self.with_namespace_source(|source| {
                validate_file_context(&self.state, &workspace, &entry.file_id, key, source)
            })?;
        }
        Ok(())
    }
    pub fn create_workspace(&mut self, branch_id: &str, principal: &str) -> Result<Workspace> {
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
        let head = self
            .state
            .commits
            .get(&branch.head_commit_id)
            .ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::StateInvalid,
                    "branch HEAD missing",
                )
            })?;
        // The workspace freezes a per-file selection snapshot for conflict
        // fencing, so opening one is O(head files) by contract; file entries
        // stream once through the paged head rather than from a flat commit.
        let head_files = {
            let mut files = Vec::new();
            self.with_namespace_source(|source| {
                crate::namespace::NamespaceView::new(&head.namespace, source).walk(
                    |location, entry| {
                        if let storage_contract::NamespacePageEntry::File { .. } = entry {
                            files.push(crate::namespace::materialized_entry(
                                location.clone(),
                                entry,
                            ));
                        }
                        Ok(true)
                    },
                )
            })?;
            files
        };
        let w = Workspace {
            workspace_id: new_id("ws"),
            purpose: WorkspacePurpose::Normal,
            branch_id: branch_id.into(),
            principal: principal.into(),
            base_head: branch.head_commit_id.clone(),
            fencing_token: self.next_slot,
            files: BTreeMap::new(),
            selected_file_contexts: repository::main_for_branch(&self.state, branch_id)
                .map(|r| {
                    head_files
                        .iter()
                        .filter_map(|e| {
                            self.file_context_for_current_entry(&r.repository_id, e)
                                .map(|c| {
                                    (
                                        directory::context_key_from_key(
                                            &r.repository_id,
                                            e.lookup_key(),
                                        ),
                                        c.into_owned(),
                                    )
                                })
                        })
                        .collect()
                })
                .unwrap_or_default(),
            selected_directory_branches: (*self.state.directory_selections).clone(),
            directory_selection_generations: (*self.state.directory_selection_generations).clone(),
            file_selection_generations: (*self.state.file_selection_generations).clone(),
            selected_file_branches: head_files
                .iter()
                .filter_map(|entry| {
                    super::file::selected_file_branch(&self.state, branch_id, &entry.file_id)
                        .map(|selected| (entry.file_id.clone(), selected.branch_id.clone()))
                })
                .collect(),
            namespace_operations: Vec::new(),
            created_at: now()?,
            state: WorkspaceState::Open,
        };
        self.persist_workspace(&w)?;
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
        Ok(w)
    }
    pub fn stage_file(&mut self, workspace: &str, path: &str, source: &Path) -> Result<StagedFile> {
        let path = name_policy::PortablePath::parse(path)?;
        let item = self
            .workspace(workspace)?
            .files
            .values()
            .find(|f| f.path.key() == path.key())
            .map(|f| f.item_id.clone())
            .unwrap_or_else(|| new_id("entry"));
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
            self.with_namespace_source(|source| {
                validate_directory_selection_key(&self.state, &w, path.key(), source)
            })?;
        }
        let key = path.key();
        // Reject invalid targets before publishing an immutable generation.
        // Otherwise an IS_DIRECTORY failure leaves {item}-{generation}.full
        // behind and a subsequent valid retry collides with those orphan bytes.
        let base = self.commit_namespace(&w.base_head).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "workspace base is absent",
            )
        })?;
        let base_entry = base.get_key(key)?;
        if w.purpose == WorkspacePurpose::Normal
            && let Some(entry) = base_entry
                .as_ref()
                .filter(|e| e.kind == NamespaceEntryKind::File)
        {
            self.with_namespace_source(|source| {
                validate_file_context(&self.state, &w, &entry.file_id, path.key(), source)
            })?;
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
            // A file branch's head holds its one file entry plus ancestors;
            // this early-stopping walk reads a single page.
            let current = match self.state.commits.get(&w.base_head) {
                Some(c) => self.with_namespace_source(|source| {
                    crate::namespace::NamespaceView::new(&c.namespace, source).find_file(file_id)
                })?,
                None => None,
            };
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
        platform_io::fs_safety::linux::SingleLinkRequirement::check(&source)
            .map_err(preparation_fs_error)?;
        let (w, base_entry) = self.validate_upload_target(workspace, item, path, generation)?;
        let expected = source.metadata()?.len();
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
        let directory_handle = OpenAt2Root::from_private_directory(
            disk_root.private_directory(&directory)?,
            MountPolicy::SameMount,
        )
        .map_err(preparation_fs_error)?;
        let (retained, generations) = retained_upload_usage(&directory_handle)?;
        let target = directory.join(format!("{item}-{generation}.full"));
        let existing = disk_root.open_optional_regular(&target)?.is_some();
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
            source,
            expected_content,
            generation,
            base_version_id: base_entry.map(|entry| entry.version_id),
            directory,
            directory_handle,
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
    pub fn finish_upload(&mut self, mut prepared: PreparedUpload) -> Result<StagedFile> {
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
            .directory(&plan.directory)?;
        if platform_io::fs_safety::linux::FileIdentity::from_fd(&current_directory)?
            != plan.directory_handle.identity()
        {
            return Err(Error::new(
                storage_contract::ErrorCode::UnsafePath,
                "workspace data directory identity changed during preparation",
            ));
        }
        if let Some(verified) = &prepared.verified {
            let current = if let Some(scratch) = &prepared.scratch {
                let relative =
                    RelativePath::new(Path::new(scratch.file_name().expect("scratch filename")))
                        .map_err(preparation_fs_error)?;
                plan.directory_handle
                    .open_file(&relative, false)
                    .map_err(preparation_fs_error)?
            } else {
                plan.retained_file()?
            };
            // The single-pass copy retains the inode it wrote. Validate both
            // that handle and its pathname before publishing its receipt;
            // readonly permissions alone do not prove unchanged contents.
            if FileStamp::of(&verified.file)? != verified.stamp
                || FileStamp::of(&current)? != verified.stamp
            {
                return Err(Error::new(
                    storage_contract::ErrorCode::ChecksumMismatch,
                    "upload generation changed during preparation",
                ));
            }
        }
        if let Some(old) = plan
            .previous
            .as_ref()
            .filter(|old| old.dirty_generation == plan.generation)
        {
            return Ok(old.clone());
        }
        let (retained, generations) = retained_upload_usage(&plan.directory_handle)?;
        if retained > self.config.max_workspace_bytes || generations > self.config.max_entries {
            return Err(Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                "retained generations exceed the workspace byte or count budget",
            ));
        }
        if let Some(scratch) = &prepared.scratch {
            // Unique immutable scratch is already included in the quota above.
            // Rename, never copy or hash, while holding the serialized writer.
            rustix::fs::renameat_with(
                plan.directory_handle.directory(),
                scratch.file_name().expect("scratch filename"),
                plan.directory_handle.directory(),
                plan.target.file_name().expect("target filename"),
                rustix::fs::RenameFlags::NOREPLACE,
            )
            .map_err(std::io::Error::from)?;
            prepared.scratch = None;
        }
        plan.directory_handle.directory().sync_all()?;
        let staged = StagedFile {
            item_id: plan.item.clone(),
            path: plan.path.clone(),
            source_path: plan.target.clone(),
            size: prepared.size,
            content_hash: prepared
                .hash
                .expect("invariant: completed upload has a body digest"),
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
            self.with_namespace_source(|source| {
                validate_directory_selection_key(&self.state, &w, parsed.key(), source)
            })?;
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
        let base = self.commit_namespace(&w.base_head).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "workspace base missing",
            )
        })?;
        if w.purpose == WorkspacePurpose::Normal
            && let Some(entry) = base.get_key(parsed.key())?
        {
            self.with_namespace_source(|source| {
                validate_file_context(&self.state, &w, &entry.file_id, parsed.key(), source)
            })?;
            let current = self
                .state
                .branches
                .get(&w.branch_id)
                .and_then(|branch| self.commit_namespace(&branch.head_commit_id))
                .map(|tree| tree.get_key(entry.lookup_key()))
                .transpose()?
                .flatten();
            if current.is_none_or(|current| current.version_id != entry.version_id) {
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
        } else if let Some(entry) = base.get_key(parsed.key())? {
            let source = self.open_content(&entry.version_id)?;
            std::io::copy(
                &mut (&source.file).take(source.size.saturating_add(1)),
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
        self.persist_aborted_workspace(w)
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
                namespace,
                result,
            } => {
                self.persist_with_objects(*next, prepared, namespace, result.clone())?;
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
        let replay = if self.find_receipt(request.operation_id)?.is_some() {
            Some(self.prepare_operation_health(self.operation_record(request.operation_id)?))
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
    pub(crate) fn refresh_workspace_contexts(
        state: &State,
        completed: &mut Workspace,
        source: &dyn crate::namespace::NamespaceSource,
    ) -> Result<()> {
        if let Some(repo) = repository::main_for_branch(state, &completed.branch_id) {
            for file in completed.files.values() {
                if let Some(context) =
                    directory::context_at_key(state, &repo.repository_id, file.path.key(), source)?
                {
                    completed.selected_file_contexts.insert(
                        directory::context_key_from_key(&repo.repository_id, file.path.key()),
                        context,
                    );
                }
            }
        }
        Ok(())
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
        let refreshed = self.with_namespace_source(|source| {
            Self::refresh_workspace_contexts(&self.state, &mut completed, source)
        });
        completed.state = WorkspaceState::Committed;
        completed.fencing_token += 1;
        if let Err(source) = refreshed.and_then(|()| self.persist_workspace(&completed)) {
            // Publication is already authoritative. Even pre-write budget/path
            // failures leave local completion unresolved, never a rejected save.
            self.blocked = true;
            return Err(Error::PublishedWorkspaceReconciliation(Box::new(
                PublishedWorkspaceReconciliation::new(result, &completed, source),
            )));
        }
        self.workspaces
            .insert(completed.workspace_id.clone(), completed);
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
        let (storage_name, created_at) =
            crate::container_storage::version_identity(state, file_id, path, &digest)?;
        let version = FileVersion {
            history_tree_id: history.into(),
            version_id: new_id("ver"),
            file_id: file_id.into(),
            creation_primary_slot_id: primary.into(),
            logical_size: size,
            version_sequence: state.sequence,
            content_hash,
            checksum_chunk_bytes: self.config.checksum_chunk_bytes,
            checksum_manifest_hash: blake3::hash(&manifest.encode()?).to_hex().to_string(),
            creation_name: path.file_name().into(),
            storage_name,
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
            self.with_namespace_source(|source| {
                crate::protection::apply_backup_paths(
                    state,
                    &repository,
                    file_id,
                    path,
                    &route,
                    source,
                )
            })?;
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
        replay: Option<crate::engine::OperationReadPlan>,
    ) -> Result<WorkspaceMutation> {
        if let Some(result) = replay {
            let digest = native_commit_digest(&workspace, &request)?;
            self.validate_workspace_commit_replay(&workspace, &request, &digest)?;
            self.require_saved_workspace_request(&workspace, &request)?;
            return Ok(WorkspaceMutation {
                mutation: MutationPlan::Existing(result.check().into_result()),
                workspace,
                request,
                wire,
            });
        }
        let w = workspace;
        if w.purpose == WorkspacePurpose::HistoricalBranch {
            return Err(Error::new(
                storage_contract::ErrorCode::HistoricalSaveRequiresBranch,
                "commit a historical-save plan to create a full branch",
            ));
        }
        validate_workspace_dependencies(&w)?;
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
            |engine, state, scratch| engine.with_namespace_source(|published| {
                let branch = state.branches.get(&w.branch_id).cloned()
                    .ok_or_else(|| Error::new(storage_contract::ErrorCode::BranchNotFound, "branch disappeared"))?;
                if branch.state != BranchState::Active {
                    return Err(Error::new(storage_contract::ErrorCode::BranchArchived, "branch is not writable"));
                }
                // Freeze the source records without copying the current Commit.
                // Helpers may insert candidate commits before namespace replay;
                // the editor below still owns its one mutable draft.
                let source_commits = state.commits.clone();
                let current = source_commits.get(&branch.head_commit_id)
                    .ok_or_else(|| Error::new(storage_contract::ErrorCode::StateInvalid, "branch HEAD missing"))?;
                let base = state.commits.get(&w.base_head).cloned()
                    .ok_or_else(|| Error::new(storage_contract::ErrorCode::StateInvalid, "workspace base missing"))?;
                let repo = engine.repository_for_branch(&w.branch_id)?;

                // Path-copy edit: staged writes and namespace operations touch
                // only their own pages and ancestor routes; untouched subtree
                // references are shared with the current head.
                let current_view = crate::namespace::NamespaceView::new(&current.namespace, published);
                let mut editor = crate::namespace::NamespaceEditor::open(&current.namespace, published)?;
                for file in w.files.values() {
                    validate_directory_selection_key(state, &w, file.path.key(), published)?;
                    let existing = current_view.resolve_key(file.path.key())?;
                    if existing.as_ref().map(|entry| &entry.version_id) != file.base_version_id.as_ref() {
                        return Err(Error::new(storage_contract::ErrorCode::FileVersionConflict, "file changed after this workspace began"));
                    }
                    if let Some(entry) = existing {
                        validate_file_context(state, &w, &entry.file_id, file.path.key(), published)?;
                    }
                }
                {
                    let base_view = crate::namespace::NamespaceView::new(&base.namespace, published);
                    for operation in &w.namespace_operations {
                        validate_directory_selection_key(state, &w, operation.key(), published)?;
                        if let Some(destination) = &operation.destination() { validate_directory_selection_key(state, &w, destination.key(), published)?; }
                        let key = operation.key();
                        // A same-name replacement is not the entry this workspace
                        // opened. In particular, every directory has an empty
                        // version_id, so path/content alone cannot fence it.
                        // Each view enumerates only this operation's subtree.
                        let selected = |view: &crate::namespace::NamespaceView<'_, dyn crate::namespace::NamespaceSource + '_>| -> Result<Vec<(String, String, NamespaceEntryKind, String, String)>> {
                            let mut out = Vec::new();
                            match view.resolve_key(key)? {
                                None => {}
                                Some(entry) if entry.kind == NamespaceEntryKind::File => {
                                    out.push((entry.entry_id.clone(), entry.file_id.clone(), entry.kind, entry.path().to_owned(), entry.version_id.clone()));
                                }
                                Some(_) => {
                                    view.walk_subtree(key, |_, entry| {
                                        out.push((entry.entry_id.clone(), entry.file_id.clone(), entry.kind, entry.path().to_owned(), entry.version_id.clone()));
                                        Ok(true)
                                    })?;
                                }
                            }
                            Ok(out)
                        };
                        if selected(&current_view)? != selected(&base_view)? {
                            return Err(Error::new(storage_contract::ErrorCode::NamespaceConflict, "namespace changed after the workspace began"));
                        }
                    }
                }
                let commit_id = new_id("commit");
                let mut written_files = std::collections::BTreeSet::new();
                let mut written_paths = Vec::new();
                for file in w.files.values() {
                    let existing = current_view.resolve_key(file.path.key())?;
                    // An insensitive lookup is not a rename. Keep the existing
                    // display path; explicit namespace operations own renames.
                    let location = existing.as_ref().map(|entry| entry.location()).unwrap_or(&file.path);
                    let path = location.as_str();
                    written_paths.push(path.to_owned());
                    let file_id = existing.as_ref().map(|entry| entry.file_id.clone()).unwrap_or_else(|| new_id("file"));
                    super::directory::prepare_file_write(engine,state,&w.branch_id,&file_id,location.key(),&request.principal,request.operation_id,published,&mut scratch.namespace)?;
                    let (file_context, pending_source) = {
                        let draft = crate::namespace::DraftSource { fresh: &scratch.namespace, base: published };
                        let file_context = directory::branch_context_at_key(state,&w.branch_id,location.key(),&draft)?;
                        let pending_source = match file_context.as_ref().and_then(|c|state.branches.get(&c.branch_id)).and_then(|b|b.pending_file_versions.get(&file_id)).cloned() {
                            Some(source) => Some(source),
                            None => directory::unwritten_source_at(state,&w.branch_id,location.key(),&draft)?,
                        };
                        (file_context, pending_source)
                    };
                    // A first write is a new branch-owned version even when its
                    // bytes equal the source or are genuinely empty.
                    if pending_source.is_none() && existing.as_ref().is_some_and(|entry| {
                        state.versions.get(&entry.version_id).is_some_and(|version|
                            version.content_hash == file.content_hash && version.logical_size == file.size)
                    }) {
                        continue;
                    }
                    editor.ensure_parents(existing.as_ref().map(|entry| entry.location()).unwrap_or(&file.path))?;
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
                        state, &mut scratch.payloads, crate::business::VersionBuild { history: &branch.history_tree_id, file_id: &file_id, path: location, source: &source, primary: &primary, copies, source_ids: source_ids.clone(), checkpoint: pending_source.is_some() },
                    )?;
                    if version.logical_size != file.size || version.content_hash != file.content_hash {
                        return Err(Error::new(storage_contract::ErrorCode::ChecksumMismatch, "workspace source changed after sealing"));
                    }
                    if let Some(node)=file_context.as_ref().and_then(|c|c.node_id.as_ref()) {
                        file_paths::source_node(state,&file_id,source_ids.first().map(String::as_str).unwrap_or(""),node)?;
                        state.file_node_parents.insert(version.version_id.clone(),vec![node.clone()]);
                    }
                    if repo.main_branch_id == w.branch_id
                        && let Some(context) = state.file_contexts.get_mut(&directory::context_key_from_key(&repo.repository_id, location.key()))
                    {
                        context.node_id = None;
                    }
                    let location = location.clone();
                    let entry = Entry { entry_id: existing.as_ref().map(|entry| entry.entry_id.clone()).unwrap_or_else(|| file.item_id.clone()), path: location, kind: NamespaceEntryKind::File, file_id: file_id.clone(), version_id: version.version_id, size: version.logical_size };
                    editor.upsert_file(&entry)?;
                    state.file_locations.entry(file_id.clone()).or_insert_with(||path.to_owned());
                    state.file_names.entry(file_id.clone()).or_insert_with(||path.rsplit('/').next().unwrap_or("").into());
                    written_files.insert(file_id);
                }
                let renamed_directories=directory::prepare_renamed_directories(state,&repo.repository_id,&current.namespace,&w.namespace_operations,published,&mut scratch.namespace)?;
                // Mirror the flat-list workspace replay: an explicit mkdir for a
                // parent that uploads in this same workspace materialized is
                // accepted once; every other operation keeps apply's semantics.
                let deleted = {
                    let base_view = crate::namespace::NamespaceView::new(&base.namespace, published);
                    let mut explicit_directories = std::collections::BTreeSet::new();
                    let mut deleted = Vec::new();
                    for op in &w.namespace_operations {
                        let key = op.key();
                        if op.kind() == NamespaceOperationKind::Mkdir
                            && base_view.resolve_key(key)?.is_none()
                            && editor.entry_kind(key)? == Some(NamespaceEntryKind::Directory)
                            && explicit_directories.insert(key)
                        {
                            continue;
                        }
                        deleted.extend(editor.apply(op)?);
                        if op.kind() == NamespaceOperationKind::Mkdir {
                            explicit_directories.insert(op.key());
                        }
                    }
                    deleted
                };
                let namespace = editor.seal(&mut scratch.namespace)?;
                if namespace.entries > engine.config.max_entries as u64 {
                    return Err(Error::new(storage_contract::ErrorCode::ResourceLimit, "directory entry budget exceeded"));
                }
                super::directory::namespace_locations(state,&repo.repository_id,&current.namespace,&namespace,&w.namespace_operations,&request.principal,request.operation_id,published,&mut scratch.namespace)?;
                // Deterministic packing makes an unchanged draft reseal to the
                // identical root, so no-change is one reference comparison.
                if namespace.root == current.namespace.root && deleted.is_empty() {
                    return Ok(json!({"commit_id":current.commit_id,"branch_id":branch.branch_id,"history_tree_id":branch.history_tree_id,"no_change":true}));
                }
                // Reservations survive only while their file is still present:
                // point lookups per pending key, never a full namespace scan.
                let (snapshot_pending, dead_branch_pending) = {
                    let draft = crate::namespace::DraftSource { fresh: &scratch.namespace, base: published };
                    let view = crate::namespace::NamespaceView::new(&namespace, &draft);
                    let live = |file: &String| -> Result<bool> {
                        Ok(state.file_locations.get(file)
                            .map(|location| view.resolve_key(&name_policy::path_key(location)))
                            .transpose()?
                            .flatten()
                            .is_some_and(|entry| entry.kind == NamespaceEntryKind::File && entry.file_id == *file)
                            || view.find_file(file)?.is_some())
                    };
                    let mut retained = std::collections::BTreeMap::new();
                    for (file, source) in current.pending_file_versions.iter() {
                        if !written_files.contains(file) && live(file)? {
                            retained.insert(file.clone(), source.clone());
                        }
                    }
                    let mut dead = Vec::new();
                    let branch_pending: Vec<String> = state
                        .branches
                        .get(&w.branch_id)
                        .map(|b| b.pending_file_versions.keys().cloned().collect())
                        .unwrap_or_default();
                    for file in branch_pending {
                        if written_files.contains(&file) || !(retained.contains_key(&file) || live(&file)?) {
                            dead.push(file);
                        }
                    }
                    (retained, dead)
                };
                state.commits.insert(commit_id.clone(), Commit {
                    namespace_deletions: namespace::deletion_record(deleted, now()?),
                    commit_id: commit_id.clone(), history_tree_id: branch.history_tree_id.clone(),
                    kind: "content".into(), parent_commit_ids: vec![current.commit_id.clone()],
                    tree_id: new_id("tree"), sequence: state.sequence, operation_id: request.operation_id,
                    author: request.principal.clone(), message: request.message.clone(), namespace: namespace.clone(), pending_file_versions: snapshot_pending, provenance: None,
                });
                let target = state.branches.get_mut(&w.branch_id).expect("branch checked");
                target.head_commit_id = commit_id.clone();
                target.generation = target.generation.checked_add(1).ok_or_else(|| Error::new(storage_contract::ErrorCode::ResourceLimit, "branch generation exhausted"))?;
                for file in dead_branch_pending { target.pending_file_versions.remove(&file); }
                if !written_files.is_empty(){target.file_node_id=None;}
                if branch.file_id.is_some() {
                    let selected_path = {
                        let draft = crate::namespace::DraftSource { fresh: &scratch.namespace, base: published };
                        let mut found = None;
                        for file in w.files.values() {
                            if directory::context_at_key(state,&repo.repository_id, file.path.key(), &draft)?
                                .is_some_and(|c|c.branch_id==w.branch_id&&c.inherited_from.is_none())
                            {
                                found = Some(file.path.clone());
                                break;
                            }
                        }
                        found
                    };
                    if let Some(path)=selected_path {
                        super::file::publish_selected_file_at(state,&w.branch_id,Some(&path),&request.principal,request.operation_id,published,&mut scratch.namespace)?;
                    }
                } else if repo.main_branch_id == w.branch_id {
                    super::file::synchronize_directory_edits(state, &commit_id, &written_paths, &request.principal, request.operation_id, published, &mut scratch.namespace)?;
                    let changed_paths = written_paths.into_iter()
                        .chain(w.namespace_operations.iter().flat_map(|n| std::iter::once(n.path().to_owned()).chain(n.destination().map(|path|path.as_str().to_owned()))))
                        .chain(renamed_directories)
                        .collect::<Vec<_>>();
                    super::directory::record_directory_edits(state, &repo.repository_id, &changed_paths, None, published, &mut scratch.namespace)?;
                }
                Ok(json!({"commit_id":commit_id,"branch_id":w.branch_id,"history_tree_id":branch.history_tree_id,"file_node_ids":file_paths::operation_nodes(state,request.operation_id)}))
            }),
        )?;
        Ok(WorkspaceMutation {
            mutation,
            workspace: w,
            request,
            wire,
        })
    }
}
