//! Source-bound file-container installation. The common installer owns all
//! publication, retention and crash recovery; this adapter only proves scope.
use super::{MAX_BYTES, invalid, prepare, verify_scope};
use crate::{
    Result,
    database_install::{InstallAdmission, PreparedInstall, Scope},
    disk::DiskRoot,
    state_projection::StateProjection,
};
use platform_io::fs_safety::linux::FileIdentity;
use std::path::Path;

pub(in crate::container_storage) struct PreparedFile {
    install: PreparedInstall,
    inode: FileIdentity,
}
impl PreparedFile {
    pub(in crate::container_storage) fn capture(
        root: &DiskRoot,
        history: (&Path, &str),
        container: &Path,
        file: &str,
        view: StateProjection<'_>,
        admission: InstallAdmission,
    ) -> Result<Self> {
        let (history_path, history_id) = history;
        let held = capture_entity(root, history, container, file, view)?;
        let inode = held.directory_identity(held.path())?;
        let database = prepare(view, file, history_id)?;
        let install = PreparedInstall::capture(
            root,
            history_path,
            (container, inode),
            (Scope::file(history_id, file)?, admission),
            database.bytes,
            MAX_BYTES,
            |bytes| verify_scope(bytes, file, history_id),
        )?;
        // `held` remains alive through matching capture, preventing inode reuse.
        Ok(Self { install, inode })
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
        OWNER_MARKER, layout_locator, marker_matches_at, path_exists_at,
        reject_unsupported_container, verify_body,
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
            let body = versions.join(&version.storage_name);
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
    Ok(held)
}
