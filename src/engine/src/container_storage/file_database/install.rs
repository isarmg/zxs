//! Source-bound file-container installation. The common installer owns all
//! publication, retention and crash recovery; this adapter only proves scope.
use crate::container_storage::file_database::{
    CapturedMaterial, MAX_BYTES, PreparedFileMaterial, invalid, validate_owner, verify_scope,
};
use crate::{
    Result,
    database_install::{InstallAdmission, PreparedInstall, Scope},
    disk::DiskRoot,
    state_projection::StateProjection,
};
use platform_io::fs_safety::linux::FileIdentity;
use std::{path::Path, sync::Arc};

struct CapturedTarget<'a> {
    root: &'a DiskRoot,
    scoped: bool,
    history: &'a Path,
    container: &'a Path,
    held: DiskRoot,
    inode: FileIdentity,
}

impl<'a> CapturedTarget<'a> {
    fn capture(
        root: &'a DiskRoot,
        history: (&'a Path, &str),
        container: &'a Path,
        file: &str,
        view: StateProjection<'_>,
    ) -> Result<Self> {
        let held = capture_entity(root, history, container, file, view)?;
        let inode = held.directory_identity(held.path())?;
        Ok(Self {
            root,
            scoped: false,
            history: history.0,
            container,
            held,
            inode,
        })
    }

    fn capture_container(
        root: &'a DiskRoot,
        history: (&'a Path, &str),
        container: &'a DiskRoot,
        file: &str,
        view: StateProjection<'_>,
    ) -> Result<Self> {
        validate_owner(view, file, history.1)?;
        let record = view.state().file_containers.get(&file.parse::<storage_contract::FileId>()?)
            .ok_or(crate::engine::fresh_layout::FreshLayoutError::ContainerShape)?;
        let inode = container.directory_identity(container.path())?;
        if !record.replicas.values().any(|replica| {
            replica.entity.device() == inode.device && replica.entity.inode() == inode.inode
        }) {
            return Err(crate::engine::fresh_layout::FreshLayoutError::ContainerIdentity.into());
        }
        let held = container.private_child_matching(container.path(), inode)?;
        verify_entity_contents(&held, file, view, false, Some(&record.namespace.name))?;
        Ok(Self {
            root, scoped: true, history: history.0, container: container.path(), held, inode,
        })
    }

    fn prepare(
        self,
        database: &PreparedFileMaterial,
        admission: InstallAdmission,
    ) -> Result<PreparedFile> {
        let Self {
            root,
            scoped,
            history,
            container,
            held,
            inode,
        } = self;
        let file = &database.file;
        let history_id = &database.history;
        let install = if scoped {
            PreparedInstall::capture_held(
                root, history, &held, (Scope::file(history_id, file)?, admission),
                Arc::clone(&database.bytes), MAX_BYTES,
                |bytes| verify_scope(bytes, file, history_id),
            )?
        } else {
            PreparedInstall::capture(
                root, history, (container, inode),
                (Scope::file(history_id, file)?, admission), Arc::clone(&database.bytes),
                MAX_BYTES, |bytes| verify_scope(bytes, file, history_id),
            )?
        };
        // The original descriptor remains alive through matching capture.
        Ok(PreparedFile { install, inode })
    }
}

/// One retained entity and its complete logical FILE material. Capturing keeps
/// physical admission ahead of row admission; preparation consumes the rows
/// without consulting State or replacing the held entity binding. Neither
/// phase authorizes publication or installation.
pub(in crate::container_storage) struct CapturedFile<'a> {
    target: CapturedTarget<'a>,
    material: CapturedMaterial,
}

impl<'a> CapturedFile<'a> {
    pub(in crate::container_storage) fn capture(
        root: &'a DiskRoot,
        history: (&'a Path, &str),
        container: &'a Path,
        file: &str,
        view: StateProjection<'_>,
    ) -> Result<Self> {
        let target = CapturedTarget::capture(root, history, container, file, view)?;
        let material = CapturedMaterial::from_projection(view, file, history.1)?;
        Ok(Self { target, material })
    }

    pub(in crate::container_storage) fn prepare(
        self,
        admission: InstallAdmission,
    ) -> Result<PreparedFile> {
        self.target.prepare(&self.material.encode()?, admission)
    }
}

/// One image for targets of this exact borrowed logical view. Construction is
/// lazy at the first real target, after its physical capture. Each later target
/// repeats physical and old-FILE admission; only pure row capture and encoding
/// are shared. No target observation or installation authority is reused.
pub(in crate::container_storage) struct PreparedFileCopies<'a> {
    view: StateProjection<'a>,
    database: PreparedFileMaterial,
}

impl<'a> PreparedFileCopies<'a> {
    pub(in crate::container_storage) fn capture(
        root: &DiskRoot,
        history: (&Path, &str),
        container: &Path,
        file: &str,
        view: StateProjection<'a>,
        admission: InstallAdmission,
    ) -> Result<(Self, PreparedFile)> {
        let CapturedFile { target, material } =
            CapturedFile::capture(root, history, container, file, view)?;
        let database = material.encode()?;
        let prepared = target.prepare(&database, admission)?;
        Ok((Self { view, database }, prepared))
    }

    pub(in crate::container_storage) fn capture_container(
        root: &DiskRoot,
        history: (&Path, &str),
        container: &DiskRoot,
        file: &str,
        view: StateProjection<'a>,
        admission: InstallAdmission,
    ) -> Result<(Self, PreparedFile)> {
        let target = CapturedTarget::capture_container(root, history, container, file, view)?;
        let database = CapturedMaterial::from_projection(view, file, history.1)?.encode()?;
        let prepared = target.prepare(&database, admission)?;
        Ok((Self { view, database }, prepared))
    }

    pub(in crate::container_storage) fn prepare_container(
        &self,
        root: &DiskRoot,
        history: (&Path, &str),
        container: &DiskRoot,
        admission: InstallAdmission,
    ) -> Result<PreparedFile> {
        CapturedTarget::capture_container(
            root, history, container, &self.database.file, self.view,
        )?.prepare(&self.database, admission)
    }

    pub(in crate::container_storage) fn prepare(
        &self,
        root: &DiskRoot,
        history: (&Path, &str),
        container: &Path,
        admission: InstallAdmission,
    ) -> Result<PreparedFile> {
        let target =
            CapturedTarget::capture(root, history, container, &self.database.file, self.view)?;
        // Locations include the predecessor as well as the next State. A later
        // target must not silently replace its different history with this one.
        validate_owner(self.view, &self.database.file, history.1)?;
        target.prepare(&self.database, admission)
    }
}

pub(in crate::container_storage) struct PreparedFile {
    install: PreparedInstall,
    inode: FileIdentity,
}
impl PreparedFile {
    pub(in crate::container_storage) fn require_entity(
        &self,
        expected: storage_contract::file_container::ContainerEntity,
    ) -> Result<()> {
        if self.inode.device != expected.device() || self.inode.inode != expected.inode() {
            return Err(crate::engine::fresh_layout::FreshLayoutError::ContainerIdentity.into());
        }
        Ok(())
    }

    pub(in crate::container_storage) fn reservation(&self) -> (u64, usize) {
        self.install.reservation()
    }
    pub(in crate::container_storage) fn byte_len(&self) -> usize {
        self.install.byte_len()
    }
    pub(in crate::container_storage) fn check_entity(
        &self,
        root: &DiskRoot,
        path: &Path,
    ) -> Result<()> {
        if root.directory_identity(path)? != self.inode {
            return Err(invalid("file database container identity changed"));
        }
        Ok(())
    }
    pub(in crate::container_storage) fn stage_unpublished(
        &mut self,
        root: &DiskRoot,
        path: &Path,
        observe: &mut impl FnMut(&'static str) -> Result<()>,
    ) -> Result<()> {
        self.check_entity(root, path)?;
        self.install.stage_unpublished(root, observe)?;
        self.check_entity(root, path)
    }
    pub(in crate::container_storage) fn verify_unpublished(
        &self,
        root: &DiskRoot,
        path: &Path,
    ) -> Result<()> {
        self.check_entity(root, path)?;
        self.install.verify_unpublished(root)
    }
    pub(in crate::container_storage) fn unpublished_identity(
        &self,
        root: &DiskRoot,
        path: &Path,
    ) -> Result<String> {
        self.check_entity(root, path)?;
        let identity = self.install.unpublished_identity(root)?;
        self.check_entity(root, path)?;
        Ok(identity)
    }
    pub(in crate::container_storage) fn apply_container(
        self,
        management: &DiskRoot,
        container: &DiskRoot,
        observe: &mut impl FnMut(&'static str) -> Result<()>,
    ) -> Result<()> {
        let inode = self.inode;
        if container.directory_identity(container.path())? != inode {
            return Err(crate::engine::fresh_layout::FreshLayoutError::ContainerIdentity.into());
        }
        // PreparedInstall owns the exact FILE entity FD. Only its retention
        // hierarchy uses the independently held management root supplied here.
        self.install.apply(management, observe)?;
        if container.directory_identity(container.path())? != inode {
            return Err(crate::engine::fresh_layout::FreshLayoutError::ContainerIdentity.into());
        }
        Ok(())
    }

    pub(in crate::container_storage) fn apply(
        self,
        root: &DiskRoot,
        path: &Path,
        observe: &mut impl FnMut(&'static str) -> Result<()>,
    ) -> Result<()> {
        self.check_entity(root, path)?;
        let inode = self.inode;
        self.install.apply(root, observe)?;
        if root.directory_identity(path)? != inode {
            return Err(invalid(
                "file database container name changed during installation",
            ));
        }
        Ok(())
    }
}

fn capture_entity(
    root: &DiskRoot,
    (history, history_id): (&Path, &str),
    current: &Path,
    file: &str,
    view: StateProjection<'_>,
) -> Result<DiskRoot> {
    use super::super::{
        layout_locator, path_exists_at,
    };
    let locator = history
        .join(".zxs-layout/files")
        .join(format!("{file}.json"));
    let indexed = path_exists_at(root, history, &locator)?;
    let held = if indexed {
        let anchor = layout_locator::HistoryAnchor::new(root, history, history_id)?;
        let intent = layout_locator::load_file(&anchor, file, &mut |_| Ok(()))?;
        root.private_child_matching(current, intent.inode().identity())?
    } else {
        // A legacy current-format location has no locator inode. Prove its
        // owner on the very descriptor retained by the new installer.
        if !current.starts_with(history.join("file-containers").join(file)) {
            return Err(invalid(
                "unregistered file container is outside its stable owner",
            ));
        }
        root.private_child(current)?
    };
    verify_entity_contents(&held, file, view, indexed, None)?;
    Ok(held)
}

/// A source-specific caller already pinned the exact current occurrence. Keep
/// ordinary owner/BODY/unsupported checks in the same FILE admission kernel.
pub(in crate::container_storage) fn verify_held_container(
    held: &DiskRoot, file: &str, view: StateProjection<'_>,
) -> Result<()> {
    let record = view.state().file_containers.get(&file.parse::<storage_contract::FileId>()?)
        .ok_or(crate::engine::fresh_layout::FreshLayoutError::ContainerShape)?;
    validate_owner(view, file, &record.history_tree_id.to_string())?;
    verify_entity_contents(held, file, view, false, Some(&record.namespace.name))
}

fn verify_entity_contents(
    held: &DiskRoot,
    file: &str,
    view: StateProjection<'_>,
    indexed: bool,
    name: Option<&name_policy::LookupName>,
) -> Result<()> {
    use super::super::{OWNER_MARKER, marker_matches_at, reject_unsupported_container, verify_body};
    reject_unsupported_container(&held, held.path())?;
    let versions = held.path().join("versions");
    held.private_child(&versions)?;
    if !marker_matches_at(
        &held,
        &held.path().join(OWNER_MARKER),
        file,
        &mut |_| Ok(()),
    )? {
        if indexed {
            return Err(invalid("indexed file container lost its owner marker"));
        }
        // The first payload is installed before the publication decision; its
        // owner marker is only published with names after Finalize. An unmarked
        // empty/unrelated directory cannot stand in for that prepared payload.
        let mut proved = false;
        for id in view.owners().versions(file) {
            let version = &view.state().versions[id];
            // A decided leaf rename can precede the outer container/owner marker.
            // Its inode-bound intent locates the body; only a complete hash proves it.
            let body = match crate::container_storage::version_rename::interrupted_path(
                &held, held.path(), version,
            )? {
                Some(body) => body,
                None => match name {
                    Some(name) => crate::container_storage::ordinary_body_path(
                        held.path(), version, name,
                    )?,
                    None => crate::container_storage::installed_version_path(held.path(), version)?,
                },
            };
            if held.optional_metadata(&body)?.is_some() {
                verify_body(&held, &body, version)?;
                proved = true;
                break;
            }
        }
        if !proved {
            return Err(invalid(
                "unmarked file container has no retained verified body",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/container_storage/file_database_copy_tests.rs"]
mod tests;
