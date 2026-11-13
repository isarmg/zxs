//! Descriptor creation, immutable reads and handle admission.
use super::*;

pub(super) struct OpenOptions {
    writable: bool,
    create: bool,
    truncate: bool,
    exclusive: bool,
    write_through: bool,
}
impl OpenOptions {
    pub(super) fn parse(
        writable: bool,
        create: bool,
        truncate: bool,
        exclusive: bool,
        write_through: bool,
    ) -> Result<Self, ApiError> {
        if !writable && truncate {
            return Err(ApiError::invalid("read handle cannot truncate"));
        }
        Ok(Self {
            writable,
            create,
            truncate,
            exclusive,
            write_through,
        })
    }
}

impl Session {
    pub(super) fn open(
        &mut self,
        path: String,
        writable: bool,
        create: bool,
        truncate: bool,
        exclusive: bool,
        write_through: bool,
    ) -> Result<(OpenResult, OwnedFd), ApiError> {
        let options = OpenOptions::parse(writable, create, truncate, exclusive, write_through)?;
        let path = if path.is_empty() {
            SmbRelativePath::Root
        } else {
            SmbRelativePath::Entry(
                name_policy::LookupPath::parse(&path).map_err(|source| contract(source.into()))?,
            )
        };
        self.open_typed(path, options)
    }
    pub(super) fn open_typed(
        &mut self,
        path: SmbRelativePath,
        options: OpenOptions,
    ) -> Result<(OpenResult, OwnedFd), ApiError> {
        let OpenOptions {
            writable,
            create,
            truncate,
            exclusive,
            write_through,
        } = options;
        let lookup = match path {
            SmbRelativePath::Root => {
                self.authorize_binding()?;
                None
            }
            SmbRelativePath::Entry(path) => Some(path),
        };
        if self.handles.len() >= MAX_HANDLES {
            return Err(ApiError::limit("too many SMB handles"));
        }
        let existing = lookup
            .as_ref()
            .map(|path| self.find(path))
            .transpose()?
            .flatten();
        if existing.is_some() && create && exclusive {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::NameConflict,
                "entry already exists",
            ));
        }
        if existing.is_none() && !create && lookup.is_some() {
            return Err(ApiError::new(
                ::http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::NotFound,
                "entry does not exist",
            ));
        }
        // The share root has no creation path. Existing entries retain their
        // display spelling; missing create targets are parsed before admission.
        let path = match (&existing, &lookup) {
            (Some(entry), _) => Some(entry.path.clone()),
            (None, None) => None,
            (None, Some(path)) => Some(
                name_policy::PortablePath::try_from(path.clone())
                    .map_err(|source| contract(source.into()))?,
            ),
        };
        let handle_id = self.next_handle;
        self.next_handle += 1;
        // An unwritten branch reads its parent's immutable version. A private
        // write copy starts from those visible bytes, while merely opening and
        // closing the handle leaves the branch without an own version.
        let mut version = existing
            .as_ref()
            .filter(|entry| !entry.version_id.is_empty())
            .map(|entry| entry.version_id.clone());
        let mut entry_id = existing
            .as_ref()
            .map(|entry| entry.entry_id.clone())
            .unwrap_or_else(|| self.branch.clone());
        if path.is_none()
            || existing
                .as_ref()
                .is_some_and(|entry| entry.kind == storage_contract::NamespaceEntryKind::Directory)
        {
            if writable || create || truncate {
                return Err(ApiError::new(
                    ::http::StatusCode::CONFLICT,
                    storage_contract::ErrorCode::IsDirectory,
                    "directory data is not writable",
                ));
            }
            let canonical_path = path.as_ref().map_or("", |path| path.as_str()).to_owned();
            // Freeze a directory's snapshot when its descriptor is opened,
            // not on the first read. Capture and identity validation share the
            // engine authorization lock and allocate no namespace copy.
            let snapshot = self.shared.backend.read_directory(
                &self.identity,
                crate::ReadDirectory {
                    branch: &self.branch,
                    entry_id: &entry_id,
                    path: path.as_ref(),
                    selection: crate::DirectorySelection::Capture,
                },
            )?;
            let snapshot = snapshot.snapshot;
            let file =
                File::open(self.shared.spool.join("empty-directory")).map_err(ApiError::io)?;
            let fd = OwnedFd::from(file.try_clone().map_err(ApiError::io)?);
            self.handles.insert(
                handle_id,
                Handle::Directory(DirectoryHandle {
                    file,
                    entry_id: entry_id.clone(),
                    path: path.clone(),
                    cursor: Some(DirectoryCursor {
                        snapshot,
                        after: None,
                    }),
                }),
            );
            let result = OpenResult {
                entry_id,
                kind: storage_contract::NamespaceEntryKind::Directory,
                path: canonical_path,
                handle: handle_id,
                size: 0,
                max_file_bytes: self.max_file_bytes,
                version_id: None,
                workspace_id: None,
                mtime: smb_protocol::protocol::FileMtime::NotFile {},
            };
            return Ok((result, fd));
        }
        let path = path.expect("invariant: a file handle has an admitted entry path");
        let mut mtime = match version.as_deref() {
            Some(version) => self.version_mtime(version)?,
            None => smb_protocol::protocol::FileMtime::Unpublished {},
        };
        // O_RDONLY | O_CREAT creates a missing empty file without granting
        // data-write access. Only a missing target needs a private workspace;
        // an existing read-only open stays pinned to its immutable version.
        let (handle, fd, size, workspace_id) = if writable || (create && existing.is_none()) {
            let operation_id = OperationId::generate().map_err(ApiError::contract_input)?;
            let paths = path_reservation::PathReservation::acquire(
                &self.shared,
                [(self.branch.as_str(), path.key())],
            )?;
            let mut pending_workspace = None;
            let mut pending_source = None;
            let mut pending_reservation = None;
            let result = (|| {
                let workspace = self.shared.workspace(&self.identity, &self.branch)?;
                pending_workspace = Some(workspace.workspace_id.clone());
                // Bind the observed file to the workspace's immutable baseline.
                // An HTTP writer can commit between find() and workspace creation,
                // even while this session owns the SMB-only path reservation.
                let baseline = self.shared.backend.lookup_at(
                    &self.identity,
                    &self.branch,
                    &workspace.base_head,
                    lookup.as_ref().expect("file handles have a lookup path"),
                )?;
                let same_entry = match (&existing, &baseline) {
                    (Some(existing), Some(baseline)) => {
                        existing.entry_id == baseline.entry_id
                            && existing.version_id == baseline.version_id
                            && existing.path == baseline.path
                            && existing.kind == baseline.kind
                    }
                    (None, None) => true,
                    (Some(_), None) | (None, Some(_)) => false,
                };
                if !same_entry {
                    return Err(ApiError::conflict(
                        storage_contract::ErrorCode::FileVersionConflict,
                        "the file changed while its write handle was opening; reopen it",
                    ));
                }
                use platform_io::fs_safety::{EntryName, PayloadOpenMode, PrivateDirectory};
                let directory = PrivateDirectory::open_trusted(&self.shared.spool)
                    .map_err(crate::upload_fs_error)?;
                let name = EntryName::new(format!(
                    "{}-{}",
                    workspace.workspace_id,
                    new_id("upload").map_err(ApiError::contract_input)?
                ))
                .map_err(crate::upload_fs_error)?;
                let mut file = directory
                    .open_payload(&name, PayloadOpenMode::CreateNew)
                    .map_err(crate::upload_fs_error)?;
                let source = directory.path().join(name.as_path());
                pending_source = Some(source.clone());
                let context = existing.as_ref().map(|entry| self.container_context(entry))
                    .transpose()?.flatten();
                let current = context.map(|context| {
                    let current = self.shared.backend.current_container_content(
                        &self.identity, context.clone(),
                    )?;
                    Ok::<_, ApiError>((context, current))
                }).transpose()?;
                let (container, content) = match current {
                    Some((context, current)) => {
                        if current.edit.draft().is_some() {
                            version = None;
                            mtime = smb_protocol::protocol::FileMtime::Unpublished {};
                        }
                        let container = ContainerWriter {
                            context, base: current.base, edit: current.edit,
                            revision: None, persisted: false,
                        };
                        let content = if truncate { None }
                            else { Some(current.content.fully_verified()?) };
                        (Some(container), content)
                    }
                    None => (None, version.as_deref().filter(|_| !truncate)
                        .map(|version| self.shared.backend.content(&self.identity, version))
                        .transpose()?),
                };
                let initial_size = content.as_ref().map_or(0, |content| content.size);
                pending_reservation = Some(self.shared.budget.reserve(
                    &self.identity.principal,
                    &source,
                    initial_size,
                    self.max_file_bytes,
                )?);
                let reservation = pending_reservation.as_mut().unwrap();
                if let Some(mut content) = content {
                    let mut copied = 0u64;
                    let mut buffer = [0u8; 65536];
                    loop {
                        let length = content.file.read(&mut buffer).map_err(ApiError::io)?;
                        if length == 0 {
                            break;
                        }
                        copied = copied
                            .checked_add(length as u64)
                            .ok_or_else(|| ApiError::limit("SMB byte count overflow"))?;
                        reservation.before_write(copied, length as u64)?;
                        file.write_all(&buffer[..length]).map_err(ApiError::io)?;
                    }
                }
                let size = file.metadata().map_err(ApiError::io)?.len();
                if size > self.max_file_bytes {
                    return Err(ApiError::limit("file exceeds SMB byte limit"));
                }
                if existing.is_none() {
                    // Allocate the future entry identity in this unpublished
                    // workspace so the SMB file id remains stable at first save.
                    entry_id = new_id("entry").map_err(ApiError::contract_input)?;
                    let staged = self.shared.backend.upload(
                        &self.identity,
                        &workspace.workspace_id,
                        &entry_id,
                        &path,
                        &crate::UploadSource::from_file(
                            file.try_clone().map_err(ApiError::io)?,
                            self.max_file_bytes,
                        )?,
                    )?;
                    entry_id = staged.item_id;
                }
                let fd = OwnedFd::from(file.try_clone().map_err(ApiError::io)?);
                let workspace_id = workspace.workspace_id.clone();
                let mut writer = Writer {
                    recovery_record: recovery_record::ActiveRecord::default(),
                    reservation: pending_reservation.take().unwrap(),
                    file,
                    directory,
                    source,
                    path: path.clone(),
                    item_id: entry_id.clone(),
                    last_head: workspace.base_head.clone(),
                    workspace,
                    dirty: existing.is_none() || truncate,
                    committed: false,
                    operation_id,
                    write_through,
                    container,
                };
                if let Err(error) = writer.file.sync_all().map_err(ApiError::io).and_then(|()| {
                    recovery_record(&mut writer, &self.identity.principal, &self.branch)
                }) {
                    publication::cleanup_writer(&writer);
                    return Err(error);
                }
                Ok((Box::new(writer), fd, size, Some(workspace_id)))
            })();
            if result.is_err() {
                if let Some(id) = pending_workspace
                    && let Err(error) = self.shared.backend.abort_workspace(&self.identity, &id)
                {
                    tracing::warn!(error = ?error, "SMB preparation abort failed; workspace retained for recovery");
                }
                if let Some(path) = pending_source {
                    for path in [path.with_extension("work.json"), path] {
                        if let Err(error) = fs::remove_file(path)
                            && error.kind() != io::ErrorKind::NotFound
                        {
                            tracing::warn!(error = ?error, "SMB preparation files retained after cleanup failure");
                        }
                    }
                }
            }
            let (writer, fd, size, workspace_id) = result?;
            (Handle::Write(writer, paths), fd, size, workspace_id)
        } else {
            let version_id = version.clone().ok_or_else(|| {
                ApiError::new(
                    ::http::StatusCode::NOT_FOUND,
                    storage_contract::ErrorCode::NotFound,
                    "version missing",
                )
            })?;
            let context = self.container_context(existing.as_ref()
                .expect("invariant: existing read handle has its entry"))?;
            let (content, authority) = if let Some(context) = context {
                let current = self.shared.backend.current_container_content(
                    &self.identity, context.clone(),
                )?;
                if current.base.version.to_string() != version_id
                    || current.base.entry.to_string() != entry_id
                {
                    return Err(ApiError::conflict(storage_contract::ErrorCode::FileVersionConflict,
                        "current file changed while its read handle was opening; reopen it"));
                }
                if current.edit.draft().is_some() {
                    version = None;
                    mtime = smb_protocol::protocol::FileMtime::Unpublished {};
                    (current.content, ReadAuthority::Draft(context))
                } else {
                    (current.content, ReadAuthority::Version(version_id.to_owned()))
                }
            } else {
                (self.shared.backend.range_content(&self.identity, crate::ReadContent {
                    version: &version_id, context: None,
                })?, ReadAuthority::Version(version_id.to_owned()))
            };
            let manifest = content.checksum_manifest.as_ref().ok_or_else(|| {
                ApiError::invalid(
                    "SMB immutable content requires an authenticated checksum manifest",
                )
            })?;
            crate::verified_chunk::validate_manifest(manifest, content.size)?;
            let size = content.size;
            // The negotiated VFS obtains bytes only through verified Read RPCs.
            // A raw read on this existing placeholder fails with EISDIR rather
            // than exposing the daemon's still-unverified payload descriptor.
            let fd = OwnedFd::from(
                File::open(self.shared.spool.join("empty-directory")).map_err(ApiError::io)?,
            );
            (Handle::Read(content, authority), fd, size, None)
        };
        self.handles.insert(handle_id, handle);
        let result = OpenResult {
            entry_id,
            kind: storage_contract::NamespaceEntryKind::File,
            path: path.into_string(),
            handle: handle_id,
            size,
            max_file_bytes: self.max_file_bytes,
            version_id: version,
            workspace_id,
            mtime,
        };
        Ok((result, fd))
    }
    pub(super) fn authorize_read(&self, handle: u64) -> Result<&Handle, ApiError> {
        let opened = self.handles.get(&handle).ok_or_else(missing_handle)?;
        opened.authorize_read(&self.shared, &self.identity, &self.branch)?;
        Ok(opened)
    }

    pub(super) fn read(
        &mut self,
        handle: u64,
        offset: u64,
        length: u32,
    ) -> Result<bytes::Bytes, ApiError> {
        if length as usize > smb_protocol::protocol::MAX_READ_BYTES {
            return Err(ApiError::limit("SMB read exceeds bounded reply size"));
        }
        let opened = self.handles.get_mut(&handle).ok_or_else(missing_handle)?;
        opened.authorize_read(&self.shared, &self.identity, &self.branch)?;
        let content = match opened {
            Handle::Read(content, _) => content,
            Handle::Directory(_) => {
                return Err(ApiError::new(
                    ::http::StatusCode::CONFLICT,
                    storage_contract::ErrorCode::IsDirectory,
                    "directory data is not readable",
                ));
            }
            Handle::Write(_, _) => return Err(ApiError::unsupported()),
        };
        // Version authorization concerns metadata; it does not admit disk I/O.
        // Check even a cached/empty read so an idle handle cannot resurrect a
        // cancelled source after the gate has closed its payload descriptor.
        if let Err(error) = content.check_running() {
            self.read_cache = None;
            return Err(error);
        }
        if length == 0 || offset >= content.size {
            return Ok(bytes::Bytes::new());
        }
        let cached = self
            .read_cache
            .as_ref()
            .filter(|(owner, _)| *owner == handle)
            .and_then(|(_, chunk)| chunk.range(offset, length as usize));
        let bytes = match cached {
            Some(bytes) => bytes,
            None => {
                // Evict first: at most one complete checksum chunk per session.
                self.read_cache = None;
                let manifest = content
                    .checksum_manifest
                    .as_ref()
                    .expect("invariant: immutable Open retained its authenticated manifest");
                let chunk = crate::verified_chunk::read(
                    &mut content.file,
                    &mut content.fallback_files,
                    manifest,
                    offset,
                )
                .map_err(|source| {
                    ApiError::new(
                        ::http::StatusCode::SERVICE_UNAVAILABLE,
                        storage_contract::ErrorCode::ContentUnavailable,
                        "no replica has verified bytes for the requested range",
                    )
                    .with_source(source)
                })?;
                let bytes = chunk
                    .range(offset, length as usize)
                    .expect("invariant: verified chunk covers the requested offset");
                self.read_cache = Some((handle, chunk));
                bytes
            }
        };
        Ok(bytes)
    }
}

pub(super) fn missing_handle() -> ApiError {
    ApiError::new(
        ::http::StatusCode::NOT_FOUND,
        storage_contract::ErrorCode::NotFound,
        "SMB handle missing",
    )
}

impl Handle {
    // Borrowing the selected handle keeps its identity and kind stable for this
    // synchronous operation; live backend authority is still checked each time.
    pub(super) fn authorize_read(
        &self,
        shared: &Shared,
        identity: &Identity,
        branch: &str,
    ) -> Result<(), ApiError> {
        match self {
            Self::Read(_, ReadAuthority::Version(version)) => {
                shared.backend.authorize_version(identity, version)
            }
            Self::Read(_, ReadAuthority::Draft(context)) => {
                shared.backend.read_container_edit(identity, context.clone()).map(|_| ())
            }
            Self::Write(..) | Self::Directory(_) => {
                shared.backend.authorize_current(identity, branch)
            }
        }
    }
}
