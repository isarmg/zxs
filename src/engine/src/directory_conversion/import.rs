use super::{
    name_budget::{ImportNames, NameBudget},
    source::{Kind, Limits, Source},
    *,
};
use crate::{
    BranchState, CommitRequest, DiskRegistrationIdentity, NamespaceEntryKind, NamespaceOperation,
    OperationId, OperationResult, RegistrationMode, Workspace,
};
use name_policy::PortablePath;
use std::fs::File;

#[derive(Clone, Debug)]
pub struct OrdinaryDirectoryPreview {
    pub digest: DirectoryConversionDigest,
    pub source: PathBuf,
    pub files: usize,
    pub directories: usize,
    pub content_bytes: u64,
    pub temporary_bytes: u64,
}
/// A read-only source inventory can survive the separately confirmed admission
/// of a new empty primary. It grants no authority to initialize or merge a disk.
pub struct PreparedOrdinaryDirectory {
    preview: OrdinaryDirectoryPreview,
    source: Source,
}
impl PreparedOrdinaryDirectory {
    /// Bounded preview before a target system exists: at most 100,000 entries,
    /// 1 TiB total and 1 TiB per file. Binding also enforces the actual host limits.
    pub fn inspect(source: File, source_path: &Path) -> Result<Self> {
        Self::inspect_with_limits(
            source,
            source_path,
            Limits {
                entries: 100_000,
                bytes: 1 << 40,
                file_bytes: 1 << 40,
            },
        )
    }
    /// Read-only preview against the caller's verified fresh target FD.
    /// This does not admit or initialize that target, and does not change names.
    pub fn inspect_for_target(source: File, source_path: &Path, target: &File) -> Result<Self> {
        let maximum = rustix::fs::fstatvfs(target).map_err(std::io::Error::from)?.f_namemax;
        Self::inspect_source(
            source,
            source_path,
            Limits {
                entries: 100_000,
                bytes: 1 << 40,
                file_bytes: 1 << 40,
            },
            Some(NameBudget::from_maximum(maximum)?),
        )
    }
    fn inspect_with_limits(source: File, source_path: &Path, limits: Limits) -> Result<Self> {
        Self::inspect_source(source, source_path, limits, None)
    }
    fn inspect_source(
        source: File,
        source_path: &Path,
        limits: Limits,
        budget: Option<NameBudget>,
    ) -> Result<Self> {
        let source_path = crate::disk::absolute_path(source_path)?;
        if source_path.to_str().is_none() {
            return Err(invalid("conversion root must be valid UTF-8"));
        }
        let source = Source::hold(source, &source_path, limits, budget)?;
        let mut digest = blake3::Hasher::new();
        digest.update(b"zxs:ordinary-directory\0");
        digest_field(&mut digest, source_path.as_os_str().as_encoded_bytes());
        source.update_digest(&mut digest);
        let preview = OrdinaryDirectoryPreview {
            digest: DirectoryConversionDigest(*digest.finalize().as_bytes()),
            source: source_path,
            files: source
                .items()
                .filter(|item| item.kind == Kind::File)
                .count(),
            directories: source
                .items()
                .filter(|item| item.kind == Kind::Directory)
                .count(),
            content_bytes: source.bytes(),
            temporary_bytes: source.bytes(),
        };
        Ok(Self { preview, source })
    }
    pub fn preview(&self) -> &OrdinaryDirectoryPreview {
        &self.preview
    }
    /// The caller must verify this slot is the exact new disk in its confirmed
    /// addition receipt. Source changes, non-primary or nonempty targets fail.
    pub fn prepare_new_disk_import(
        self,
        engine: &Engine,
        slot: &str,
    ) -> Result<PreparedDirectoryImport> {
        let (identity, target) = disk_target(engine, slot, None)?;
        engine.bind_import(self, target, identity, None)
    }
}

enum Target {
    Branch { id: String, head: String },
    NewRoot { slot: String },
}
pub struct PreparedDirectoryImport {
    preview: DirectoryConversionPreview,
    source: Source,
    target: Target,
    destination: Option<PortablePath>,
    registration: DiskRegistrationIdentity,
    sequence: u64,
}
impl Engine {
    pub fn prepare_ordinary_directory(
        &self,
        source: File,
        source_path: &Path,
    ) -> Result<PreparedOrdinaryDirectory> {
        let source_path = external_root(self, source_path)?;
        PreparedOrdinaryDirectory::inspect_with_limits(
            source,
            &source_path,
            Limits {
                entries: self.config.max_entries,
                bytes: self.config.max_workspace_bytes,
                file_bytes: self.config.max_file_bytes,
            },
        )
    }
    pub fn prepare_directory_import(
        &self,
        source: File,
        source_path: &Path,
        branch: &str,
        destination: &PortablePath,
    ) -> Result<PreparedDirectoryImport> {
        let repository = self.repository_for_branch(branch)?;
        if !repository.file_manager_root || repository.main_branch_id != branch {
            return Err(invalid(
                "ordinary directory import requires the primary disk-root namespace",
            ));
        }
        self.prepare_disk_directory_import(
            source,
            source_path,
            &repository.primary_slot_id,
            destination,
        )
    }
    pub fn prepare_disk_directory_import(
        &self,
        source: File,
        source_path: &Path,
        slot: &str,
        destination: &PortablePath,
    ) -> Result<PreparedDirectoryImport> {
        let source = self.prepare_ordinary_directory(source, source_path)?;
        let (identity, target) = disk_target(self, slot, Some(destination))?;
        self.bind_import(source, target, identity, Some(destination.clone()))
    }
    fn conversion_name_budget(&self, slot: &str) -> Result<NameBudget> {
        let disk = self.disk(slot)?;
        let maximum = self.disk_resources.get(disk)?.root.component_name_maximum()?;
        NameBudget::from_maximum(maximum)
    }
    fn bind_import(
        &self,
        source: PreparedOrdinaryDirectory,
        target: Target,
        registration: DiskRegistrationIdentity,
        destination: Option<PortablePath>,
    ) -> Result<PreparedDirectoryImport> {
        self.require_write()?;
        external_root(self, &source.preview.source)?;
        source.source.verify_named(&source.preview.source)?;
        source.source.verify_unchanged()?;
        let (target_name, head, existing) = match &target {
            Target::Branch { id, head } => (id.as_str(), head.as_str(), self.list_tree_ref(id)?),
            Target::NewRoot { slot } => (slot.as_str(), "", &[][..]),
        };
        let mut added = source.source.count();
        if let Some(destination) = &destination {
            added = added
                .checked_add(1)
                .ok_or_else(|| invalid("import entry count overflow"))?;
            let mut ancestor = destination.parent();
            while let Some(path) = ancestor {
                if existing
                    .iter()
                    .all(|entry| entry.lookup_key() != path.key())
                {
                    added = added
                        .checked_add(1)
                        .ok_or_else(|| invalid("import entry count overflow"))?;
                }
                ancestor = path.parent();
            }
        }
        if existing
            .len()
            .checked_add(added)
            .is_none_or(|count| count > self.config.max_entries)
            || source.source.bytes() > self.config.max_workspace_bytes
            || source
                .source
                .items()
                .any(|item| item.kind == Kind::File && item.size() > self.config.max_file_bytes)
        {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "ordinary import exceeds host entry or byte limits",
            ));
        }
        let names = ImportNames::plan(
            &source.source,
            destination.as_ref(),
            self.conversion_name_budget(&registration.slot_id)?,
        )?;
        for item in names.source.items() {
            names.target_path(&item.path)?;
        }
        let mut digest = blake3::Hasher::new();
        digest.update(b"zxs:directory-import\0");
        for field in [
            self.config.pool_id.as_bytes(),
            registration.disk_id.as_bytes(),
            target_name.as_bytes(),
            destination
                .as_ref()
                .map_or("", PortablePath::as_str)
                .as_bytes(),
            head.as_bytes(),
        ] {
            digest_field(&mut digest, field);
        }
        digest.update(&registration.binding_generation.to_le_bytes());
        digest.update(&self.state.sequence.to_le_bytes());
        digest.update(&source.preview.digest.0);
        let preview = DirectoryConversionPreview {
            digest: DirectoryConversionDigest(*digest.finalize().as_bytes()),
            source: source.preview.source,
            destination: format!(
                "{}:/{}",
                registration.slot_id,
                destination.as_ref().map_or("", PortablePath::as_str)
            ),
            files: source.preview.files,
            directories: added - source.preview.files,
            content_bytes: source.preview.content_bytes,
            temporary_bytes: source.preview.temporary_bytes,
        };
        Ok(PreparedDirectoryImport {
            preview,
            source: source.source,
            target,
            destination,
            registration,
            sequence: self.state.sequence,
        })
    }
}
impl PreparedDirectoryImport {
    pub fn preview(&self) -> &DirectoryConversionPreview {
        &self.preview
    }
    pub fn execute(
        self,
        engine: &mut Engine,
        principal: &str,
        operation: OperationId,
        confirmation: DirectoryConversionConfirmation,
    ) -> std::result::Result<OperationResult, DirectoryImportFailure> {
        self.execute_with_progress(engine, principal, operation, confirmation, |_| Ok(()))
    }
    /// The observer must durably retain the supplied references before returning.
    /// It runs before root creation, before and after workspace creation, then
    /// before publishing the exact workspace request. The first workspace ID is
    /// a durable intent, not evidence that creation succeeded. An observer error
    /// stops the operation without cleanup or automatic retries under a new ID.
    pub fn execute_with_progress(
        self,
        engine: &mut Engine,
        principal: &str,
        operation: OperationId,
        confirmation: DirectoryConversionConfirmation,
        mut observer: impl FnMut(&DirectoryImportProgress) -> Result<()>,
    ) -> std::result::Result<OperationResult, DirectoryImportFailure> {
        let mut progress = DirectoryImportProgress {
            operation,
            root_operation: None,
            workspace: None,
            request: None,
        };
        self.execute_confirmed(
            engine,
            principal,
            operation,
            confirmation,
            &mut progress,
            &mut observer,
        )
        .map_err(|source| DirectoryImportFailure {
            progress: Box::new(progress),
            source,
        })
    }
    fn execute_confirmed(
        &self,
        engine: &mut Engine,
        principal: &str,
        operation: OperationId,
        confirmation: DirectoryConversionConfirmation,
        progress: &mut DirectoryImportProgress,
        observer: &mut impl FnMut(&DirectoryImportProgress) -> Result<()>,
    ) -> Result<OperationResult> {
        confirmation.verify(&self.preview)?;
        engine.require_write()?;
        if engine.state.sequence != self.sequence
            || primary_identity(engine, &self.registration.slot_id)? != self.registration
        {
            return Err(Error::new(
                ErrorCode::PlanStale,
                "system or disk registration changed; review a new conversion plan",
            ));
        }
        super::stop::require_import_not_stopped(&engine.state, operation)?;
        if engine.state.operations.contains_key(&operation) {
            return Err(Error::new(
                ErrorCode::OperationIdConflict,
                "conversion operation was already used; query its existing result",
            ));
        }
        external_root(engine, &self.preview.source)?;
        self.source.verify_named(&self.preview.source)?;
        self.source.verify_unchanged()?;
        let names = ImportNames::plan(
            &self.source,
            self.destination.as_ref(),
            engine.conversion_name_budget(&self.registration.slot_id)?,
        )?;
        observer(progress)?;
        let branch = match &self.target {
            Target::Branch { id, head } => {
                if import_target(engine, id, self.destination.as_ref())? != head {
                    return Err(Error::new(
                        ErrorCode::PlanStale,
                        "destination changed; review a new conversion plan",
                    ));
                }
                id.clone()
            }
            Target::NewRoot { slot } => {
                let root_operation = OperationId::generate()?;
                progress.root_operation = Some(root_operation);
                observer(progress)?;
                engine
                    .create_disk_root(slot, principal, root_operation)?
                    .main_branch_id
            }
        };
        let workspace = engine.create_workspace_with_intent(&branch, principal, |workspace| {
            progress.workspace = Some(workspace.workspace_id.clone());
            observer(progress)
        })?;
        observer(progress)?;
        self.execute_workspace(
            engine, principal, operation, &workspace, &names, progress, observer,
        )
    }
    fn execute_workspace(
        &self,
        engine: &mut Engine,
        principal: &str,
        operation: OperationId,
        workspace: &Workspace,
        names: &ImportNames<'_>,
        progress: &mut DirectoryImportProgress,
        observer: &mut impl FnMut(&DirectoryImportProgress) -> Result<()>,
    ) -> Result<OperationResult> {
        if let Some(path) = &self.destination {
            engine.namespace_operation(
                &workspace.workspace_id,
                NamespaceOperation::Mkdir { path: path.clone() },
            )?;
        }
        for item in names
            .source
            .items()
            .filter(|item| item.kind == Kind::Directory)
        {
            engine.namespace_operation(
                &workspace.workspace_id,
                NamespaceOperation::Mkdir {
                    path: names.target_path(&item.path)?,
                },
            )?;
        }
        for item in names.source.items().filter(|item| item.kind == Kind::File) {
            let file = self.source.open_file(item)?;
            let witness = file
                .try_clone()
                .map_err(|error| io("retain source witness", item.path.as_str(), error))?;
            let prepared = engine
                .prepare_upload_file(
                    &workspace.workspace_id,
                    &crate::new_id("entry")?,
                    &names.target_path(&item.path)?,
                    file,
                    1,
                    None,
                )?
                .execute()?;
            self.source.verify_file(item, &witness)?;
            engine.finish_upload(prepared)?;
        }
        self.source.verify_unchanged()?;
        let request = CommitRequest {
            workspace_id: workspace.workspace_id.clone(),
            operation_id: operation,
            principal: principal.into(),
            expected_head: workspace.base_head.clone(),
            fencing_token: workspace.fencing_token,
            message: "Import ordinary directory contents".into(),
            request_digest: None,
        };
        progress.request = Some(request.clone());
        observer(progress)?;
        // Persisting the workflow intent may block; recheck after that boundary
        // so a callback delay cannot publish a source that visibly changed.
        self.source.verify_named(&self.preview.source)?;
        self.source.verify_unchanged()?;
        engine.commit_workspace(request)
    }
}
pub(super) fn target_path(
    path: &PortablePath,
    destination: Option<&PortablePath>,
) -> Result<PortablePath> {
    match destination {
        Some(destination) => Ok(path.rebase(0, destination)?),
        None => Ok(path.clone()),
    }
}
pub(super) fn primary_identity(engine: &Engine, slot: &str) -> Result<DiskRegistrationIdentity> {
    let registered = engine
        .registered_disks()
        .find(|disk| disk.slot_id.eq_ignore_ascii_case(slot))
        .ok_or_else(|| {
            Error::new(
                ErrorCode::DiskNotFound,
                "import target disk is not currently registered",
            )
        })?;
    if registered.registration_mode != RegistrationMode::Enabled
        || registered.level != 1
        || registered.upstream_slot_id.is_some()
    {
        return Err(invalid(
            "ordinary data can only be converted to an enabled primary with no upstream",
        ));
    }
    engine.disk(registered.slot_id)?;
    if let Some(namespaces) = engine.admitted_current_namespaces()? {
        namespaces.require_write_slot(registered.slot_id)?;
    }
    Ok(registered.identity())
}
fn disk_target(
    engine: &Engine,
    slot: &str,
    destination: Option<&PortablePath>,
) -> Result<(DiskRegistrationIdentity, Target)> {
    let identity = primary_identity(engine, slot)?;
    let target = match engine
        .state
        .repositories
        .values()
        .find(|repo| repo.file_manager_root && repo.primary_slot_id == identity.slot_id)
    {
        Some(repository) => Target::Branch {
            id: repository.main_branch_id.clone(),
            head: import_target(engine, &repository.main_branch_id, destination)?.to_owned(),
        },
        None => Target::NewRoot {
            slot: identity.slot_id.clone(),
        },
    };
    Ok((identity, target))
}
fn import_target<'a>(
    engine: &'a Engine,
    branch: &str,
    target: Option<&PortablePath>,
) -> Result<&'a str> {
    let branch_record = engine
        .state
        .branches
        .get(branch)
        .ok_or_else(|| Error::new(ErrorCode::BranchNotFound, "import branch is absent"))?;
    if branch_record.file_id.is_some() || branch_record.state != BranchState::Active {
        return Err(invalid("import requires an active directory branch"));
    }
    let entries = engine.list_tree_ref(branch)?;
    let Some(target) = target else {
        if !entries.is_empty() {
            return Err(Error::new(
                ErrorCode::NameCollision,
                "new primary conversion requires an empty disk-root namespace",
            ));
        }
        return Ok(&branch_record.head_commit_id);
    };
    let prefix = format!("{}/", target.key());
    if entries
        .iter()
        .any(|entry| entry.lookup_key() == target.key() || entry.lookup_key().starts_with(&prefix))
    {
        return Err(Error::new(
            ErrorCode::NameCollision,
            "conversion never overwrites an existing destination subtree",
        ));
    }
    let mut parent = target.parent();
    while let Some(path) = parent {
        if engine
            .namespace_tree(branch)?
            .get_key(path.key())
            .is_some_and(|entry| entry.kind != NamespaceEntryKind::Directory)
        {
            return Err(Error::new(
                ErrorCode::ParentNotDirectory,
                "conversion destination parent is a file",
            ));
        }
        parent = path.parent();
    }
    Ok(&branch_record.head_commit_id)
}

/// Recovery references observed during one import, never an authorization token.
#[derive(Clone, Debug)]
pub struct DirectoryImportProgress {
    operation: OperationId,
    root_operation: Option<OperationId>,
    workspace: Option<String>,
    request: Option<CommitRequest>,
}
impl DirectoryImportProgress {
    /// Persist this exact native request before publication; completion must be
    /// checked through `Engine::native_workspace_commit_completed` after restart.
    pub fn request(&self) -> Option<&CommitRequest> {
        self.request.as_ref()
    }
    pub fn workspace_id(&self) -> Option<&str> {
        self.workspace.as_deref()
    }
    pub fn operation_id(&self) -> OperationId {
        self.operation
    }
    pub fn root_operation_id(&self) -> Option<OperationId> {
        self.root_operation
    }
}
/// Exact recovery references survive failures and uncertain publication results.
#[derive(error_definition::Error)]
#[error("directory import did not finish; source and staged data are retained: {source}")]
pub struct DirectoryImportFailure {
    progress: Box<DirectoryImportProgress>,
    #[source]
    source: Error,
}
impl std::fmt::Debug for DirectoryImportFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirectoryImportFailure")
            .field("operation", &self.progress.operation)
            .field("root_operation", &self.progress.root_operation)
            .field("workspace", &self.progress.workspace)
            .field("source", &self.source)
            .finish()
    }
}
impl DirectoryImportFailure {
    pub fn request(&self) -> Option<&CommitRequest> {
        self.progress.request()
    }
    pub fn cause(&self) -> &Error {
        &self.source
    }
    pub fn workspace_id(&self) -> Option<&str> {
        self.progress.workspace.as_deref()
    }
    pub fn operation_id(&self) -> OperationId {
        self.progress.operation
    }
    pub fn root_operation_id(&self) -> Option<OperationId> {
        self.progress.root_operation
    }
}
