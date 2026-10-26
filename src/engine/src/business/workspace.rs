//! Full upload generations, working files and atomic content publication.
use super::*;
use crate::{
    disk::*,
    engine::{MutationPlan, Prepared},
};
use json_codec::json;
use platform_io::fs_safety::{
    EntryName, RelativePath,
    linux::{MountPolicy, OpenAt2Root},
};
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::Path,
};

fn upload_fingerprint_file(input: std::fs::File, maximum: u64) -> Result<(u64, String)> {
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
    Ok((size, digest.finalize().to_hex().to_string()))
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
    path: String,
    source: std::fs::File,
    expected_content: Option<(u64, String)>,
    generation: u64,
    base_version_id: Option<String>,
    directory: std::path::PathBuf,
    directory_handle: OpenAt2Root,
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
    hash: String,
    scratch: Option<std::path::PathBuf>,
    verified: Option<VerifiedUploadFile>,
}

#[derive(PartialEq, Eq)]
struct UploadFileStamp {
    device: u64,
    inode: u64,
    size: u64,
    links: u64,
    mode: u32,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl UploadFileStamp {
    fn of(file: &std::fs::File) -> Result<Self> {
        let metadata = file.metadata()?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.len(),
            links: metadata.nlink(),
            mode: metadata.mode(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }
}
struct VerifiedUploadFile {
    file: std::fs::File,
    stamp: UploadFileStamp,
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

    fn verify_retained(&self, size: u64, hash: &str) -> Result<VerifiedUploadFile> {
        let file = self.retained_file()?;
        let stamp = UploadFileStamp::of(&file)?;
        let actual = upload_fingerprint_file(file.try_clone()?, self.max_file_bytes)?;
        if actual.0 != size || actual.1 != hash || UploadFileStamp::of(&file)? != stamp {
            return Err(Error::new(
                storage_contract::ErrorCode::ChecksumMismatch,
                "retained upload generation no longer matches its receipt",
            ));
        }
        Ok(VerifiedUploadFile { file, stamp })
    }

    fn verify_content(&self, size: u64, hash: &str) -> Result<()> {
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
                hash,
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
                hash: incoming.1,
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
            hash: String::new(),
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
        prepared.hash = hash;
        prepared.verified = Some(VerifiedUploadFile {
            stamp: UploadFileStamp::of(&file)?,
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

fn validate_directory_selection(state: &State, workspace: &Workspace, path: &str) -> Result<()> {
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
    let path = name_policy::path_key(path);
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
                && (path == entry.lookup_key || namespace::is_descendant(&path, &entry.lookup_key))
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
    path: &str,
) -> Result<()> {
    let selected =
        directory::branch_context_at(state, &workspace.branch_id, path).ok_or_else(|| {
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
    let repo = state
        .repositories
        .values()
        .find(|r| r.main_branch_id == workspace.branch_id);
    let captured = repo.and_then(|r| {
        workspace
            .selected_file_contexts
            .get(&directory::context_key(&r.repository_id, path))
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
    pub(crate) path: &'a str,
    pub(crate) source: &'a Path,
    pub(crate) primary: &'a str,
    pub(crate) copies: u32,
    pub(crate) source_ids: Vec<String>,
    pub(crate) checkpoint: bool,
}

impl Engine {
    /// Check a captured destination before SMB reopens a successfully saved
    /// workspace. This also accepts committed workspaces as immutable receipts.
    pub fn validate_workspace_file_selection(&self, workspace: &str, path: &str) -> Result<()> {
        let workspace = self.workspace(workspace)?;
        validate_directory_selection(&self.state, &workspace, path)?;
        let mut entry = self
            .commit_namespace(&workspace.base_head)
            .and_then(|tree| tree.get(path));
        // First-created files are absent from the original base, but completion
        // records their published destination. The current entry supplies only
        // its identity; the saved context still decides whether it is writable.
        if entry.is_none()
            && self.state.repositories.values().any(|repo| {
                repo.main_branch_id == workspace.branch_id
                    && workspace
                        .selected_file_contexts
                        .contains_key(&directory::context_key(&repo.repository_id, path))
            })
        {
            entry = Some(
                self.state
                    .branches
                    .get(&workspace.branch_id)
                    .and_then(|branch| self.commit_namespace(&branch.head_commit_id))
                    .and_then(|tree| tree.get(path))
                    .ok_or_else(|| {
                        Error::new(
                            storage_contract::ErrorCode::FileNotFound,
                            "published workspace file is absent",
                        )
                    })?,
            );
        }
        if let Some(entry) = entry {
            validate_file_context(&self.state, &workspace, &entry.file_id, path)?;
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
        let w = Workspace {
            workspace_id: new_id("ws"),
            purpose: WorkspacePurpose::Normal,
            branch_id: branch_id.into(),
            principal: principal.into(),
            base_head: branch.head_commit_id.clone(),
            fencing_token: self.next_slot,
            files: BTreeMap::new(),
            selected_file_contexts: self
                .state
                .repositories
                .values()
                .find(|r| r.main_branch_id == branch_id)
                .map(|r| {
                    head.entries
                        .iter()
                        .filter(|e| e.kind == NamespaceEntryKind::File)
                        .filter_map(|e| {
                            self.file_context_for_current_entry(&r.repository_id, e)
                                .map(|c| {
                                    (
                                        directory::context_key(&r.repository_id, &e.path),
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
            selected_file_branches: head
                .entries
                .iter()
                .filter(|entry| entry.kind == NamespaceEntryKind::File)
                .filter_map(|entry| {
                    super::file::selected_file_branch(&self.state, branch_id, &entry.file_id)
                        .map(|selected| (entry.file_id.clone(), selected.branch_id.clone()))
                })
                .collect(),
            namespace_operations: Vec::new(),
            created_at: now(),
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
        if now().saturating_sub(w.created_at) > self.config.workspace_ttl_seconds {
            return Err(Error::new(
                storage_contract::ErrorCode::FencingTokenExpired,
                "workspace lease expired; pending bytes are preserved",
            ));
        }
        self.repository_for_branch(&w.branch_id)?;
        Ok(w)
    }
    pub fn stage_file(&mut self, workspace: &str, path: &str, source: &Path) -> Result<StagedFile> {
        let item = self
            .workspace(workspace)?
            .files
            .values()
            .find(|f| name_policy::path_key(&f.path) == name_policy::path_key(path))
            .map(|f| f.item_id.clone())
            .unwrap_or_else(|| new_id("entry"));
        self.stage_file_as(workspace, &item, path, source)
    }
    pub fn stage_file_as(
        &mut self,
        workspace: &str,
        item: &str,
        path: &str,
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
        self.stage_upload(workspace, item, path, source, generation)
    }
    /// Persist an explicitly numbered HTTP upload generation. Retrying the same
    /// generation may only replay the same complete bytes; older files remain
    /// available until the workspace is explicitly cleaned up.
    fn validate_upload_target(
        &self,
        workspace: &str,
        item: &str,
        path: &str,
        generation: u64,
    ) -> Result<(Workspace, Option<Entry>)> {
        if generation == 0 {
            return Err(Error::new(
                storage_contract::ErrorCode::InvalidRequest,
                "upload generation starts at one",
            ));
        }
        validate_id(item, "entry")?;
        name_policy::validate_path(path)?;
        let w = self.writable_workspace(workspace)?;
        if w.purpose == WorkspacePurpose::Normal {
            validate_directory_selection(&self.state, &w, path)?;
        }
        let key = name_policy::path_key(path);
        // Reject invalid targets before publishing an immutable generation.
        // Otherwise an IS_DIRECTORY failure leaves {item}-{generation}.full
        // behind and a subsequent valid retry collides with those orphan bytes.
        let base = self.commit_namespace(&w.base_head).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "workspace base is absent",
            )
        })?;
        let base_entry = base.get(path).cloned();
        if w.purpose == WorkspacePurpose::Normal
            && let Some(entry) = base_entry
                .as_ref()
                .filter(|e| e.kind == NamespaceEntryKind::File)
        {
            validate_file_context(&self.state, &w, &entry.file_id, path)?;
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
            let current = self
                .state
                .commits
                .get(&w.base_head)
                .and_then(|c| c.entries.iter().find(|e| e.file_id == file_id));
            if current.is_none_or(|entry| entry.lookup_key != key) {
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
                && other
                    .files
                    .values()
                    .any(|f| name_policy::path_key(&f.path) == key)
        }) {
            return Err(Error::new(
                storage_contract::ErrorCode::FileBusy,
                "another workspace owns this file",
            ));
        }
        if w.files
            .values()
            .any(|f| f.item_id != item && name_policy::path_key(&f.path) == key)
        {
            return Err(Error::new(
                storage_contract::ErrorCode::NameCollision,
                "workspace contains an equivalent file name",
            ));
        }
        let previous = w.files.get(item).cloned();
        if previous
            .as_ref()
            .is_some_and(|f| name_policy::path_key(&f.path) != key)
        {
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
            path,
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
        path: &str,
        source: std::fs::File,
        generation: u64,
        expected_content: Option<(u64, String)>,
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
        let directory_handle =
            OpenAt2Root::open(&directory, MountPolicy::SameMount).map_err(preparation_fs_error)?;
        let (retained, generations) = retained_upload_usage(&directory_handle)?;
        let target = directory.join(format!("{item}-{generation}.full"));
        let existing = target.try_exists()?;
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
            path: path.into(),
            source,
            expected_content,
            generation,
            base_version_id: base_entry.map(|entry| entry.version_id),
            directory,
            directory_handle,
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
        let current_directory = OpenAt2Root::open(&plan.directory, MountPolicy::SameMount)
            .map_err(preparation_fs_error)?;
        if current_directory.identity() != plan.directory_handle.identity() {
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
            if UploadFileStamp::of(&verified.file)? != verified.stamp
                || UploadFileStamp::of(&current)? != verified.stamp
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
            content_hash: prepared.hash.clone(),
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
        name_policy::validate_path(path)?;
        if w.purpose == WorkspacePurpose::Normal {
            validate_directory_selection(&self.state, &w, path)?;
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
            && let Some(entry) = base.get(path)
        {
            validate_file_context(&self.state, &w, &entry.file_id, path)?;
            let current = self
                .state
                .branches
                .get(&w.branch_id)
                .and_then(|branch| self.commit_namespace(&branch.head_commit_id))
                .and_then(|tree| tree.get_key(&entry.lookup_key));
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
            let source = open_regular(&previous.source_path)?;
            std::io::copy(
                &mut source.take(previous.size.saturating_add(1)),
                &mut temporary,
            )?;
            Some((previous.size, previous.content_hash.clone()))
        } else if let Some(entry) = base.get(path) {
            let source = self.open_content(&entry.version_id)?;
            std::io::copy(
                &mut (&source.file).take(source.size.saturating_add(1)),
                &mut temporary,
            )?;
            Some((source.size, source.content_hash.clone()))
        } else {
            None
        };
        if let Some((size, hash)) = expected {
            let digest = blake3::Hash::from_hex(&hash).map_err(|source| {
                Error::from(source).context(
                    storage_contract::ErrorCode::ChecksumMismatch,
                    "invalid staged content digest",
                )
            })?;
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
        self.stage_file_as(workspace, item, path, temporary.path())
    }
    pub fn namespace_operation(
        &mut self,
        workspace: &str,
        operation: NamespaceOperation,
    ) -> Result<Workspace> {
        if operation.kind == NamespaceOperationKind::Mkdir {
            name_policy::validate_path(&operation.path)?;
        } else {
            name_policy::validate_lookup_path(&operation.path)?;
        }
        if let Some(to) = &operation.destination {
            name_policy::validate_path(to)?;
        }
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
        self.persist_workspace(&w)?;
        self.workspaces.insert(id.into(), w);
        Ok(())
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
        if self.state.operations.contains_key(&request.operation_id) {
            let workspace = self.workspace(&request.workspace_id)?;
            let digest = native_commit_digest(&workspace, &request)?;
            let workspace = self.validate_workspace_commit_replay(&request, &digest)?;
            self.require_saved_workspace_request(&workspace, &request)?;
            return Ok(WorkspaceMutation {
                mutation: MutationPlan::Existing(self.operation(&request.operation_id)?),
                workspace,
                request,
                wire,
            });
        }
        let w = self.workspace(&request.workspace_id)?;
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
        if now().saturating_sub(w.created_at) > self.config.workspace_ttl_seconds {
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
        let mutation = self.plan_mutation_digest(
            "workspace.commit", &request.principal, &request.operation_id, input,
            wire.as_ref().map(|wire| wire.digest.clone()),
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
                let base = state.commits.get(&w.base_head)
                    .ok_or_else(|| Error::new(storage_contract::ErrorCode::StateInvalid, "workspace base missing"))?;
                let repo = engine.repository_for_branch(&w.branch_id)?;

                // One explicit mutation-local index serves both conflict checks
                // and staged replacements, reusing admitted source keys.
                let mut draft = namespace::NamespaceDraft::new(current.entries.clone());
                for file in w.files.values() {
                    validate_directory_selection(state, &w, &file.path)?;
                    let existing = draft.get(&file.path);
                    if existing.map(|entry| &entry.version_id) != file.base_version_id.as_ref() {
                        return Err(Error::new(storage_contract::ErrorCode::FileVersionConflict, "file changed after this workspace began"));
                    }
                    if let Some(entry) = existing {
                        validate_file_context(state, &w, &entry.file_id,&file.path)?;
                    }
                }
                {
                    for operation in &w.namespace_operations {
                        validate_directory_selection(state, &w, &operation.path)?;
                        if let Some(destination) = &operation.destination { validate_directory_selection(state, &w, destination)?; }
                        let key = name_policy::path_key(&operation.path);
                        let prefix = format!("{key}/");
                        // A same-name replacement is not the entry this workspace
                        // opened. In particular, every directory has an empty
                        // version_id, so path/content alone cannot fence it.
                        let selected = |entries: &[Entry]| entries.iter()
                            .filter(|entry| entry.lookup_key == key || entry.lookup_key.starts_with(&prefix))
                            .map(|entry| (entry.entry_id.clone(), entry.file_id.clone(), entry.kind, entry.path.clone(), entry.version_id.clone())).collect::<Vec<_>>();
                        if selected(&current.entries) != selected(&base.entries) {
                            return Err(Error::new(storage_contract::ErrorCode::NamespaceConflict, "namespace changed after the workspace began"));
                        }
                    }
                }
                let commit_id = new_id("commit");
                let mut written_files = Vec::new();
                let mut written_paths = Vec::new();
                for file in w.files.values() {
                    let existing = draft.get(&file.path).cloned();
                    // An insensitive lookup is not a rename. Keep the existing
                    // display path; explicit namespace operations own renames.
                    let path = existing.as_ref().map(|entry| entry.path.as_str()).unwrap_or(&file.path);
                    written_paths.push(path.to_owned());
                    let file_id = existing.as_ref().map(|entry| entry.file_id.clone()).unwrap_or_else(|| new_id("file"));
                    super::directory::prepare_file_write(engine,state,&w.branch_id,&file_id,path,&request.principal,&request.operation_id)?;
                    let file_context=directory::branch_context_at(state,&w.branch_id,path);
                    let pending_source = file_context.as_ref().and_then(|c|state.branches.get(&c.branch_id)).and_then(|b|b.pending_file_versions.get(&file_id)).cloned()
                        .or_else(||directory::unwritten_source_at(state,&w.branch_id,path).map(str::to_owned));
                    // A first write is a new branch-owned version even when its
                    // bytes equal the source or are genuinely empty.
                    if pending_source.is_none() && existing.as_ref().is_some_and(|entry| {
                        state.versions.get(&entry.version_id).is_some_and(|version|
                            version.content_hash == file.content_hash && version.logical_size == file.size)
                    }) {
                        continue;
                    }
                    draft.ensure_parents(path)?;
                    let source_ids:Vec<String> = pending_source.as_ref().cloned()
                        .or_else(|| existing.as_ref().map(|entry| entry.version_id.clone()))
                        .into_iter().collect();
                    let (primary, copies) = if let Some(placement) = wire.as_ref().and_then(|wire| wire.placements.get(&file.item_id)).cloned() {
                        placement
                    } else {
                        engine.new_file_placement_for_file(&repo.repository_id,Some(&file_id), path)?
                    };
                    let version = engine.build_version(
                        state, pending, crate::business::VersionBuild { history: &branch.history_tree_id, file_id: &file_id, path, source: &file.source_path, primary: &primary, copies, source_ids: source_ids.clone(), checkpoint: pending_source.is_some() },
                    )?;
                    if version.logical_size != file.size || version.content_hash != file.content_hash {
                        return Err(Error::new(storage_contract::ErrorCode::ChecksumMismatch, "workspace source changed after sealing"));
                    }
                    if let Some(node)=file_context.as_ref().and_then(|c|c.node_id.as_ref()) {
                        file_paths::source_node(state,&file_id,source_ids.first().map(String::as_str).unwrap_or(""),node)?;
                        state.file_node_parents.insert(version.version_id.clone(),vec![node.clone()]);
                    }
                    if let Some(repo)=state.repositories.values().find(|r|r.main_branch_id==w.branch_id)&& let Some(context)=state.file_contexts.get_mut(&directory::context_key(&repo.repository_id,path)){context.node_id=None;}
                    let entry = Entry {
                        entry_id: existing.as_ref().map(|entry| entry.entry_id.clone()).unwrap_or_else(|| file.item_id.clone()),
                        path: path.into(), name: path.rsplit('/').next().unwrap_or("").into(),
                        lookup_key: name_policy::path_key(path), kind: NamespaceEntryKind::File,
                        file_id: file_id.clone(), version_id: version.version_id, size: version.logical_size,
                    };
                    draft.replace(entry);
                    state.file_locations.entry(file_id.clone()).or_insert_with(||path.to_owned());
                    state.file_names.entry(file_id.clone()).or_insert_with(||path.rsplit('/').next().unwrap_or("").into());
                    written_files.push(file_id);
                }
                let mut entries = draft.into_entries();
                let renamed_directories=directory::prepare_renamed_directories(state,&repo.repository_id,&current.entries,&w.namespace_operations)?;
                let deleted = apply_workspace_namespace(&mut entries, &current.entries, &w.namespace_operations)?;
                super::directory::namespace_locations(state,&repo.repository_id,&current.entries,&entries,&w.namespace_operations,&request.principal,&request.operation_id)?;
                validate_entries(&mut entries, engine.config.max_entries)?;
                if entries == current.entries && deleted.is_empty() {
                    return Ok(json!({"commit_id":current.commit_id,"branch_id":branch.branch_id,"history_tree_id":branch.history_tree_id,"no_change":true}));
                }
                let live_files = entries.iter().filter(|entry| entry.kind == NamespaceEntryKind::File)
                    .map(|entry| entry.file_id.clone()).collect::<std::collections::BTreeSet<_>>();
                let mut snapshot_pending = current.pending_file_versions.clone();
                snapshot_pending.retain(|file,_| live_files.contains(file) && !written_files.contains(file));
                state.commits.insert(commit_id.clone(), Commit {
                    namespace_deletions: namespace::deletion_record(deleted, now()),
                    commit_id: commit_id.clone(), history_tree_id: branch.history_tree_id.clone(),
                    kind: "content".into(), parent_commit_ids: vec![current.commit_id.clone()],
                    tree_id: new_id("tree"), sequence: state.sequence, operation_id: request.operation_id.clone(),
                    author: request.principal.clone(), message: request.message.clone(), entries, pending_file_versions: snapshot_pending, provenance: None,
                });
                let target = state.branches.get_mut(&w.branch_id).expect("branch checked");
                target.head_commit_id = commit_id.clone();
                target.generation = target.generation.checked_add(1).ok_or_else(|| Error::new(storage_contract::ErrorCode::ResourceLimit, "branch generation exhausted"))?;
                target.pending_file_versions.retain(|file, _| live_files.contains(file) && !written_files.contains(file));
                if !written_files.is_empty(){target.file_node_id=None;}
                if branch.file_id.is_some() {
                    if let Some(path)=w.files.values().find(|file|directory::context_at(state,&repo.repository_id,&file.path).is_some_and(|c|c.branch_id==w.branch_id&&c.inherited_from.is_none())).map(|f|f.path.clone()) {
                        super::file::publish_selected_file_at(state,&w.branch_id,Some(&path),&request.principal,&request.operation_id)?;
                    }
                } else if repo.main_branch_id == w.branch_id {
                    super::file::synchronize_directory_edits(state, &commit_id, &written_paths, &request.principal, &request.operation_id)?;
                    let changed_paths = written_paths.into_iter()
                        .chain(w.namespace_operations.iter().flat_map(|n| std::iter::once(n.path.clone()).chain(n.destination.clone())))
                        .chain(renamed_directories)
                        .collect::<Vec<_>>();
                    super::directory::record_directory_edits(state, &repo.repository_id, &changed_paths, None)?;
                }
                Ok(json!({"commit_id":commit_id,"branch_id":w.branch_id,"history_tree_id":branch.history_tree_id,"file_node_ids":file_paths::operation_nodes(state,&request.operation_id)}))
            },
        )?;
        Ok(WorkspaceMutation {
            mutation,
            workspace: w,
            request,
            wire,
        })
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
        write_json(
            &self
                .workspace_data_directory(w)?
                .join("commit-request.json"),
            &request,
            true,
        )?;
        if let Some(wire) = wire {
            for directory in [
                self.root.join("workspaces").join(&w.workspace_id),
                self.workspace_data_directory(w)?,
            ] {
                write_json(&directory.join("wire-request.json"), &wire.body, true)?;
            }
        }
        Ok(())
    }
    /// Keep ordinary completion and restart reconstruction on one definition.
    pub(crate) fn refresh_workspace_contexts(state: &State, completed: &mut Workspace) {
        if let Some(repo) = state
            .repositories
            .values()
            .find(|repo| repo.main_branch_id == completed.branch_id)
        {
            for file in completed.files.values() {
                if let Some(context) = directory::context_at(state, &repo.repository_id, &file.path)
                {
                    completed.selected_file_contexts.insert(
                        directory::context_key(&repo.repository_id, &file.path),
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
        self.persist_workspace(&completed)?;
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
        self.build_version_checked(state, pending, input, None)
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
        name_policy::validate_path(path)?;
        let mut file = open_regular(source)?;
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
        let content_hash = digest.to_hex().to_string();
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
            creation_name: path.rsplit('/').next().unwrap_or("").into(),
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
            crate::protection::apply_backup_paths(state, &repository, file_id, path, &route)?;
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
            source: source.into(),
        });
        Ok(version)
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/business/workspace_published_selection_tests.rs"]
mod published_selection_tests;
#[cfg(test)]
#[path = "../../../../test/engine/unit/business/workspace_selection_tests.rs"]
mod selection_tests;
