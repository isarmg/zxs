//! Descriptor creation, immutable reads and handle admission.
use super::*;

impl Session {
    pub(super) fn open(
        &mut self,
        path: String,
        writable: bool,
        create: bool,
        truncate: bool,
        exclusive: bool,
        write_through: bool,
    ) -> Result<(Value, Option<OwnedFd>), ApiError> {
        let lookup = if path.is_empty() {
            self.authorize_binding()?;
            None
        } else {
            Some(name_policy::LookupPath::parse(&path).map_err(|source| contract(source.into()))?)
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
        if existing.is_none() && !create && !path.is_empty() {
            return Err(ApiError::new(
                ::http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::NotFound,
                "entry does not exist",
            ));
        }
        // The share root has no creation path. Existing entries retain their
        // display spelling; missing create targets are parsed before admission.
        let path = match &existing {
            Some(entry) => Some(entry.path.clone()),
            None if path.is_empty() => None,
            None => Some(
                name_policy::PortablePath::parse(&path)
                    .map_err(|source| contract(source.into()))?,
            ),
        };
        let handle_id = self.next_handle;
        self.next_handle += 1;
        // An unwritten branch reads its parent's immutable version. A private
        // write copy starts from those visible bytes, while merely opening and
        // closing the handle leaves the branch without an own version.
        let version = existing
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
            let commit_id = snapshot.commit_id;
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
                        commit_id,
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
            };
            return Ok((
                json_codec::to_value(result)
                    .expect("invariant: internal JSON serialization succeeds"),
                Some(fd),
            ));
        }
        let path = path.expect("invariant: a file handle has an admitted entry path");
        let (handle, fd, size, workspace_id) = if writable {
            self.reserve_paths(&[path.as_str()])?;
            let mut pending_workspace = None;
            let mut pending_source = None;
            let mut pending_reservation = None;
            let result = (|| {
                let workspace = self.workspace()?;
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
                let name =
                    EntryName::new(format!("{}-{}", workspace.workspace_id, new_id("upload")))
                        .map_err(crate::upload_fs_error)?;
                let mut file = directory
                    .open_payload(&name, PayloadOpenMode::CreateNew)
                    .map_err(crate::upload_fs_error)?;
                let source = directory.path().join(name.as_path());
                pending_source = Some(source.clone());
                let content = version
                    .as_deref()
                    .filter(|_| !truncate)
                    .map(|version| self.shared.backend.content(&self.identity, version))
                    .transpose()?;
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
                    entry_id = new_id("entry");
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
                    operation_id: OperationId::generate(),
                    write_through,
                };
                if let Err(error) = writer.file.sync_all().map_err(ApiError::io).and_then(|()| {
                    recovery_record(&mut writer, &self.identity.principal, &self.branch)
                }) {
                    publication::cleanup_writer(&writer);
                    return Err(error);
                }
                Ok((
                    Handle::Write(Box::new(writer)),
                    fd,
                    size,
                    Some(workspace_id),
                ))
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
                self.release_paths(&[path.key().to_owned()]);
            }
            result?
        } else {
            if create || truncate {
                return Err(ApiError::invalid("read handle cannot create or truncate"));
            }
            let version_id = version.as_deref().ok_or_else(|| {
                ApiError::new(
                    ::http::StatusCode::NOT_FOUND,
                    storage_contract::ErrorCode::NotFound,
                    "version missing",
                )
            })?;
            let content = self.shared.backend.range_content(
                &self.identity,
                crate::ReadContent {
                    version: version_id,
                    context: None,
                },
            )?;
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
            (
                Handle::Read(
                    content,
                    version
                        .clone()
                        .ok_or_else(|| ApiError::invalid("read version missing"))?,
                ),
                fd,
                size,
                None,
            )
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
        };
        Ok((
            json_codec::to_value(result).expect("invariant: internal JSON serialization succeeds"),
            Some(fd),
        ))
    }
    pub(super) fn authorize_read(&self, handle: u64) -> Result<Value, ApiError> {
        match self.handles.get(&handle) {
            Some(Handle::Read(_, version)) => {
                self.shared
                    .backend
                    .authorize_version(&self.identity, version)?;
            }
            Some(Handle::Write(_)) | Some(Handle::Directory(_)) => {
                self.shared
                    .backend
                    .authorize_current(&self.identity, &self.branch)?;
            }
            None => {
                return Err(ApiError::new(
                    ::http::StatusCode::NOT_FOUND,
                    storage_contract::ErrorCode::NotFound,
                    "SMB handle missing",
                ));
            }
        }
        Ok(json!({"authorized":true}))
    }

    pub(super) fn read(
        &mut self,
        handle: u64,
        offset: u64,
        length: u32,
    ) -> Result<Value, ApiError> {
        if length as usize > smb_protocol::protocol::MAX_READ_BYTES {
            return Err(ApiError::limit("SMB read exceeds bounded reply size"));
        }
        self.authorize_read(handle)?;
        let content = match self.handles.get_mut(&handle) {
            Some(Handle::Read(content, _)) => content,
            Some(Handle::Directory(_)) => {
                return Err(ApiError::new(
                    ::http::StatusCode::CONFLICT,
                    storage_contract::ErrorCode::IsDirectory,
                    "directory data is not readable",
                ));
            }
            Some(Handle::Write(_)) => return Err(ApiError::unsupported()),
            None => unreachable!("invariant: read authorization admitted this handle"),
        };
        if length == 0 || offset >= content.size {
            return Ok(json!({"bytes":[]}));
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
        Ok(json!({"bytes":bytes.as_ref()}))
    }
}
