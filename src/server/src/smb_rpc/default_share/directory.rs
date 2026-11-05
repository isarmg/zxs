//! Virtual share/disk directories: bounded pages and current topology/binding admission.
use super::*;
use smb_protocol::protocol::{
    AuthorizedMetadata, DirectoryEntry, DirectoryPage, MAX_DIRECTORY_PAGE_BYTES,
    MAX_DIRECTORY_PAGE_ENTRIES,
};

// Open captures one bounded page. Limit retained pages independently of the
// lightweight handle count: <= 2 MiB encoded data plus bounded typed metadata.
const MAX_BUFFERED_DISK_PAGES: usize = 8;

pub(super) struct VirtualDirectory {
    scope: VirtualScope,
    pending: Option<DirectoryPage>,
    next_cursor: Option<String>,
    exhausted: bool,
    namespace: Option<SessionKey>,
    // Keep the placeholder descriptor alive until the remote handle closes.
    _file: File,
}

#[derive(Clone)]
enum VirtualScope {
    Root {
        generation: u64,
        after: usize,
    },
    Disk {
        slot: String,
        disk_id: String,
        binding_generation: u64,
    },
}

struct DiskPage {
    page: DirectoryPage,
    next_cursor: Option<String>,
    namespace: Option<SessionKey>,
}

impl DefaultShare {
    pub(super) fn open_directory(
        &mut self,
        location: Location,
        id: u64,
    ) -> Result<(OpenResult, Option<OwnedFd>, RoutedHandle), ApiError> {
        let (entry_id, path, scope, pending, next_cursor, exhausted, namespace) = match location {
            Location::Root => {
                let topology = self.shared.backend.topology_snapshot(&self.identity)?;
                (
                    "smb-root".into(),
                    String::new(),
                    VirtualScope::Root {
                        generation: topology.generation,
                        after: 0,
                    },
                    None,
                    None,
                    false,
                    None,
                )
            }
            Location::Disk(slot) => {
                if self
                    .handles
                    .values()
                    .filter(|handle| {
                        matches!(handle,
                    RoutedHandle::Directory(directory) if directory.pending.is_some())
                    })
                    .count()
                    >= MAX_BUFFERED_DISK_PAGES
                {
                    return Err(ApiError::limit(
                        "too many unread SMB disk directory pages; read or close an existing directory",
                    ));
                }
                let DiskPage {
                    page,
                    next_cursor,
                    namespace,
                } = self.disk_page(&slot.slot_id, None)?;
                let exhausted = next_cursor.is_none();
                (
                    slot.disk_id.clone(),
                    slot.directory_name()?.as_str().to_owned(),
                    VirtualScope::Disk {
                        slot: slot.slot_id,
                        disk_id: slot.disk_id,
                        binding_generation: slot.binding_generation,
                    },
                    Some(page),
                    next_cursor,
                    exhausted,
                    namespace,
                )
            }
            Location::Path { .. } => {
                unreachable!("invariant: only virtual directories reach this path")
            }
        };
        let file = File::open(self.shared.spool.join("empty-directory")).map_err(ApiError::io)?;
        let descriptor = OwnedFd::from(file.try_clone().map_err(ApiError::io)?);
        let opened = OpenResult {
            entry_id,
            kind: storage_contract::NamespaceEntryKind::Directory,
            path,
            handle: id,
            size: 0,
            max_file_bytes: self.max_file_bytes,
            version_id: None,
            workspace_id: None,
            mtime: FileMtime::NotFile {},
        };
        Ok((
            opened,
            Some(descriptor),
            RoutedHandle::Directory(VirtualDirectory {
                scope,
                pending,
                next_cursor,
                exhausted,
                namespace,
                _file: file,
            }),
        ))
    }

    fn disk_page(&mut self, slot: &str, cursor: Option<String>) -> Result<DiskPage, ApiError> {
        let mut query = BTreeMap::from([("limit".into(), MAX_DIRECTORY_PAGE_ENTRIES.to_string())]);
        if let Some(cursor) = cursor {
            query.insert("cursor".into(), cursor);
        }
        let snapshot = self
            .shared
            .backend
            .listing(&self.identity, ReadListing::parse(slot.into(), query)?)?;
        let namespace = snapshot
            .context
            .map(|context| {
                let repository = self
                    .shared
                    .backend
                    .repository_snapshot(&self.identity, &context.repository_id)?;
                self.pin_binding(repository)
            })
            .transpose()?;
        let projection_identity = namespace
            .as_ref()
            .map(|key| self.bound_identity(key))
            .unwrap_or_else(|| self.identity.clone());
        let mut entries = Vec::with_capacity(snapshot.entries.len());
        for entry in snapshot.entries {
            let (version_id, mtime) = match entry.version {
                ListingVersion::Directory(version) => (version, FileMtime::NotFile {}),
                ListingVersion::File { content, .. } => {
                    let version = content
                        .source_version_id
                        .or(content.version_id)
                        .ok_or_else(|| {
                            ApiError::new(
                                ::http::StatusCode::CONFLICT,
                                storage_contract::ErrorCode::ContentUnavailable,
                                "the listed file has no published content",
                            )
                        })?;
                    let snapshot = self
                        .shared
                        .backend
                        .version_snapshot(&projection_identity, &version)?;
                    let mtime = crate::backend_read::version_mtime(&snapshot.version)?;
                    (version, mtime)
                }
            };
            entries.push(DirectoryEntry {
                entry: Entry {
                    entry_id: entry.entry_id,
                    path: name_policy::PortablePath::parse(&entry.path)
                        .map_err(ApiError::invalid_source)?,
                    kind: entry.kind,
                    file_id: entry.file_id,
                    version_id,
                    size: entry.size,
                },
                mtime,
            });
        }
        let page = DirectoryPage {
            more: snapshot.next_cursor.is_some(),
            entries,
        };
        // At a disk root, names are single components (<= 240 UTF-8 bytes).
        // Keep the wire byte contract explicit even if listing metadata grows.
        let encoded = json_codec::to_vec(&ResponsePayload::Ok { result: &page })
            .expect("invariant: directory page serializes");
        if encoded.len() > MAX_DIRECTORY_PAGE_BYTES {
            return Err(ApiError::limit("directory page exceeds byte budget"));
        }
        if let Some(key) = &namespace {
            self.shared
                .backend
                .authorize_repository(&projection_identity, &key.repository)?;
        }
        Ok(DiskPage {
            page,
            next_cursor: snapshot.next_cursor,
            namespace,
        })
    }

    pub(super) fn virtual_handle(
        &mut self,
        id: u64,
        request: Request<OperationId>,
    ) -> Result<(Reply, Option<OwnedFd>), ApiError> {
        let account = self.shared.account.clone();
        let binding = account.read()?;
        binding.require_generation(self.account_generation)?;
        if matches!(
            request,
            Request::Close { .. } | Request::Abort { .. } | Request::Detach { .. }
        ) {
            self.handles.remove(&id);
            return Ok(json_reply(json!({"closed":true})));
        }
        let topology = self.shared.backend.topology_snapshot(&self.identity)?;
        let RoutedHandle::Directory(directory) = self
            .handles
            .get(&id)
            .expect("invariant: virtual handle was resolved")
        else {
            unreachable!("invariant: repository handles dispatched separately")
        };
        if let VirtualScope::Disk {
            slot,
            disk_id,
            binding_generation,
        } = &directory.scope
            && !topology.slots.iter().any(|candidate| {
                candidate.level == 1
                    && candidate.slot_id == *slot
                    && candidate.disk_id == *disk_id
                    && candidate.binding_generation == *binding_generation
            })
        {
            return Err(changed());
        }
        if let Some(key) = &directory.namespace {
            self.shared
                .backend
                .authorize_repository(&self.bound_identity(key), &key.repository)?;
        }
        match request {
            Request::Flush { .. } => {
                directory._file.sync_all().map_err(ApiError::io)?;
                Ok(json_reply(json!({"saved":true,"changed":false})))
            }
            Request::AuthorizeRead { .. } => Ok(json_reply(AuthorizedMetadata {
                mtime: FileMtime::NotFile {},
            })),
            Request::ReadDirectory { restart, .. } => {
                let scope = directory.scope.clone();
                let page = match scope {
                    VirtualScope::Root { .. } => self.read_root_page(id, restart, topology)?,
                    VirtualScope::Disk { slot, .. } => self.read_disk_page(id, &slot, restart)?,
                };
                Ok(json_reply(page))
            }
            _ => Err(directory_only()),
        }
    }

    fn virtual_directory_mut(&mut self, id: u64) -> &mut VirtualDirectory {
        match self.handles.get_mut(&id) {
            Some(RoutedHandle::Directory(directory)) => directory,
            _ => {
                unreachable!("invariant: virtual handle remains owned during synchronous dispatch")
            }
        }
    }
    fn read_root_page(
        &mut self,
        id: u64,
        restart: bool,
        topology: crate::TopologySnapshot,
    ) -> Result<DirectoryPage, ApiError> {
        let VirtualScope::Root { generation, after } = self.virtual_directory_mut(id).scope else {
            unreachable!("invariant: only root directories use topology paging")
        };
        if !restart && generation != topology.generation {
            return Err(changed());
        }
        let after = if restart { 0 } else { after };
        let mut slots = topology
            .slots
            .into_iter()
            .filter(|slot| slot.level == 1)
            .collect::<Vec<_>>();
        slots.sort_by(|left, right| {
            left.name
                .as_ref()
                .map(|name| name.key())
                .cmp(&right.name.as_ref().map(|name| name.key()))
        });
        let more = slots.len() > after + MAX_DIRECTORY_PAGE_ENTRIES;
        let entries = slots
            .into_iter()
            .skip(after)
            .take(MAX_DIRECTORY_PAGE_ENTRIES)
            .map(|slot| {
                let path = slot.directory_name()?.clone();
                Ok(DirectoryEntry {
                    entry: Entry {
                        entry_id: slot.disk_id,
                        path,
                        kind: storage_contract::NamespaceEntryKind::Directory,
                        file_id: String::new(),
                        version_id: String::new(),
                        size: 0,
                    },
                    mtime: FileMtime::NotFile {},
                })
            })
            .collect::<Result<Vec<_>, ApiError>>()?;
        let directory = self.virtual_directory_mut(id);
        directory.scope = VirtualScope::Root {
            generation: topology.generation,
            after: after + entries.len(),
        };
        Ok(DirectoryPage { entries, more })
    }

    fn read_disk_page(
        &mut self,
        id: u64,
        slot: &str,
        restart: bool,
    ) -> Result<DirectoryPage, ApiError> {
        if !restart && let Some(page) = self.virtual_directory_mut(id).pending.take() {
            return Ok(page);
        }
        let directory = self.virtual_directory_mut(id);
        if !restart && directory.exhausted {
            return Ok(DirectoryPage {
                entries: vec![],
                more: false,
            });
        }
        let cursor = if restart {
            None
        } else {
            directory.next_cursor.clone()
        };
        let DiskPage {
            page,
            next_cursor,
            namespace,
        } = self.disk_page(slot, cursor)?;
        let directory = self.virtual_directory_mut(id);
        directory.pending = None;
        directory.exhausted = next_cursor.is_none();
        directory.next_cursor = next_cursor;
        directory.namespace = namespace;
        Ok(page)
    }
}
