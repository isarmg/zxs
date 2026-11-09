//! Bind ordinary existing entities, never the isolated backup-images paths.
use super::super::{
    OWNER_MARKER, container_at, layout_locator, path_exists_at, reject_unsupported_container,
};
use super::*;
use std::{os::unix::fs::MetadataExt, path::Path};

const MAX_LOCATOR: usize = 64 * 1024;
const MAX_SIBLINGS: usize = 65_536;

struct NamedFile {
    parent: DiskRoot,
    parent_inode: FileIdentity,
    leaf: String,
    held: HeldRegular,
    maximum: usize,
}
impl NamedFile {
    fn capture(root: &DiskRoot, path: &Path, maximum: usize) -> Result<Self> {
        let parent = root.private_child(
            path.parent()
                .expect("invariant: object metadata has a parent"),
        )?;
        let parent_inode = parent.directory_identity(parent.path())?;
        let leaf = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| invalid("object metadata name is not UTF-8"))?
            .to_owned();
        let held = HeldRegular::capture(&parent, &leaf, maximum)?
            .ok_or_else(|| invalid("original object owner or locator is missing"))?;
        Ok(Self {
            parent,
            parent_inode,
            leaf,
            held,
            maximum,
        })
    }
    fn check(&self, root: &DiskRoot) -> Result<()> {
        root.private_child_matching(self.parent.path(), self.parent_inode)?;
        let fresh = HeldRegular::capture(&self.parent, &self.leaf, self.maximum)?
            .ok_or_else(|| invalid("original object owner or locator disappeared"))?;
        if fresh.stamp() != self.held.stamp() {
            return Err(invalid("original object owner or locator was replaced"));
        }
        self.held.check_running()?;
        fresh.check_running()
    }
}

pub(super) struct Entity {
    pub(super) scope: backup_databases::Scope,
    pub(super) root: DiskRoot,
    pub(super) inode: FileIdentity,
    pub(super) history: PathBuf,
    history_root: DiskRoot,
    history_inode: FileIdentity,
    relative: String,
    locator: Option<NamedFile>,
    owner: Option<NamedFile>,
    versions: Option<(DiskRoot, FileIdentity)>,
}
#[derive(Serialize)]
pub(super) struct Descriptor<'a> {
    scope: &'a backup_databases::Scope,
    relative: &'a str,
    entity: Physical,
    history: Physical,
    owner: Option<&'a FileStamp>,
    locator: Option<&'a FileStamp>,
    versions: Option<Physical>,
    bytes: u64,
    digest: &'a str,
}
impl Entity {
    pub(super) fn capture(
        root: &DiskRoot,
        scope: backup_databases::Scope,
        stop: &AtomicBool,
    ) -> Result<Self> {
        preparation_checkpoint(stop)?;
        let history_id = match &scope {
            backup_databases::Scope::File { history, .. }
            | backup_databases::Scope::Directory { history, .. } => history,
        };
        let history = root.path().join("histories").join(history_id);
        let history_root = root.private_child(&history)?;
        let history_inode = history_root.directory_identity(&history)?;
        let (path, mut locator) = match &scope {
            backup_databases::Scope::File { file, .. } => {
                let path = container_at(root, &history, Some(history_id), file, &mut |_| preparation_checkpoint(stop))?
                    .ok_or_else(|| invalid("original routed FILE has no ordinary container; target copies cannot replace it"))?;
                let locator_path = history
                    .join(".zxs-layout/files")
                    .join(format!("{file}.json"));
                let locator = if path_exists_at(root, &history, &locator_path)? {
                    Some(NamedFile::capture(root, &locator_path, MAX_LOCATOR)?)
                } else {
                    None
                };
                (path, locator)
            }
            backup_databases::Scope::Directory { directory, .. } => {
                let id = layout_locator::DirectoryId::parse(directory)?;
                let anchor = layout_locator::HistoryAnchor::new(root, &history, history_id)?;
                let intent = layout_locator::load_directory(&anchor, &id, &mut |_| {
                    preparation_checkpoint(stop)
                })?;
                let path = layout_locator::locate_directory(&anchor, &intent, &mut |_| {
                    preparation_checkpoint(stop)
                })?;
                let locator = NamedFile::capture(
                    root,
                    &history
                        .join(".zxs-layout/directories")
                        .join(format!("{directory}.json")),
                    MAX_LOCATOR,
                )?;
                (path, Some(locator))
            }
        };
        if let Some(locator) = &mut locator {
            locator.held.release_bytes();
        }
        let entity = root.private_child(&path)?;
        let inode = entity.directory_identity(&path)?;
        let relative = path
            .strip_prefix(root.path())
            .map_err(|_| invalid("original object lies outside its held disk"))?
            .to_str()
            .ok_or_else(|| invalid("original object path is not UTF-8"))?
            .to_owned();
        layout_locator::HistoryRelative::parse(&relative)?;
        let (owner, versions) = match &scope {
            backup_databases::Scope::File { file, .. } => {
                reject_unsupported_container(&entity, entity.path())?;
                let mut owner = NamedFile::capture(root, &path.join(OWNER_MARKER), 256)?;
                let id: String = storage_contract::decode_strict(owner.held.bytes())?;
                if id != *file {
                    return Err(invalid(
                        "original ordinary container belongs to another FILE",
                    ));
                }
                owner.held.release_bytes();
                let versions = entity.private_child(&path.join("versions"))?;
                let version_inode = versions.directory_identity(versions.path())?;
                (Some(owner), Some((versions, version_inode)))
            }
            backup_databases::Scope::Directory { .. } => (None, None),
        };
        let result = Self {
            scope,
            root: entity,
            inode,
            history,
            history_root,
            history_inode,
            relative,
            locator,
            owner,
            versions,
        };
        result.check(root, stop)?;
        Ok(result)
    }

    pub(super) fn descriptor<'a>(&'a self, bytes: u64, digest: &'a str) -> Descriptor<'a> {
        Descriptor {
            scope: &self.scope,
            relative: &self.relative,
            entity: self.inode.into(),
            history: self.history_inode.into(),
            owner: self.owner.as_ref().map(|owner| owner.held.stamp()),
            locator: self.locator.as_ref().map(|locator| locator.held.stamp()),
            versions: self.versions.as_ref().map(|(_, inode)| (*inode).into()),
            bytes,
            digest,
        }
    }
    pub(super) fn leaf(&self) -> &'static str {
        match self.scope {
            backup_databases::Scope::File { .. } => file_database::NAME,
            backup_databases::Scope::Directory { .. } => directory_database::NAME,
        }
    }
    pub(super) fn maximum(&self) -> usize {
        match self.scope {
            backup_databases::Scope::File { .. } => file_database::MAX_BYTES,
            backup_databases::Scope::Directory { .. } => directory_database::MAX_BYTES,
        }
    }
    pub(super) fn install_scope(&self) -> Result<crate::database_install::Scope> {
        match &self.scope {
            backup_databases::Scope::File { history, file } => {
                crate::database_install::Scope::file(history, file)
            }
            backup_databases::Scope::Directory {
                history,
                repository,
                directory,
            } => crate::database_install::Scope::directory(history, repository, directory),
        }
    }
    pub(super) fn verify_scope(&self, bytes: &[u8]) -> Result<()> {
        match &self.scope {
            backup_databases::Scope::File { history, file } => {
                file_database::verify_scope(bytes, file, history)
            }
            backup_databases::Scope::Directory {
                repository,
                directory,
                ..
            } => directory_database::verify_scope(bytes, repository, directory),
        }
    }

    pub(super) fn check(&self, root: &DiskRoot, stop: &AtomicBool) -> Result<()> {
        preparation_checkpoint(stop)?;
        root.private_child_matching(&self.history, self.history_inode)?;
        self.history_root
            .directory_identity(self.history_root.path())?;
        root.private_child_matching(self.root.path(), self.inode)?;
        if let Some(locator) = &self.locator {
            locator.check(root)?;
        } else if let backup_databases::Scope::File { file, .. } = &self.scope {
            let locator = self
                .history
                .join(".zxs-layout/files")
                .join(format!("{file}.json"));
            if path_exists_at(root, &self.history, &locator)? {
                return Err(invalid("an unselected FILE locator appeared during Backup"));
            }
        }
        if let Some(owner) = &self.owner {
            owner.check(root)?;
        }
        if let Some((versions, inode)) = &self.versions {
            self.root.private_child_matching(versions.path(), *inode)?;
        }
        let actual = match &self.scope {
            backup_databases::Scope::File { history, file } => {
                container_at(root, &self.history, Some(history), file, &mut |_| {
                    preparation_checkpoint(stop)
                })?
                .ok_or_else(|| invalid("original ordinary FILE container disappeared"))?
            }
            backup_databases::Scope::Directory {
                history, directory, ..
            } => {
                let id = layout_locator::DirectoryId::parse(directory)?;
                let anchor = layout_locator::HistoryAnchor::new(root, &self.history, history)?;
                let intent = layout_locator::load_directory(&anchor, &id, &mut |_| {
                    preparation_checkpoint(stop)
                })?;
                layout_locator::locate_directory(&anchor, &intent, &mut |_| {
                    preparation_checkpoint(stop)
                })?
            }
        };
        if actual != self.root.path() {
            return Err(invalid(
                "original ordinary object address changed during Backup",
            ));
        }
        self.check_names(stop)
    }

    fn check_names(&self, stop: &AtomicBool) -> Result<()> {
        for (count, entry) in self.root.entries(self.root.path())?.enumerate() {
            preparation_checkpoint(stop)?;
            if count == MAX_SIBLINGS {
                return Err(budget());
            }
            let name = entry?;
            let name = name
                .as_os_str()
                .to_str()
                .ok_or_else(|| invalid("original object child name is not UTF-8"))?;
            // All SQLite sidecars and unknown database siblings are retained,
            // never interpreted as inactive staging or silently removed.
            if name.starts_with(".zxs-") && name.contains(".sqlite") && name != self.leaf() {
                return Err(invalid(
                    "original object has a SQLite sidecar or unknown database name; all bytes retained",
                ));
            }
            if matches!(self.scope, backup_databases::Scope::File { .. }) {
                match name {
                    "versions" | OWNER_MARKER => {}
                    "container.json" => {
                        let file = self.root.open_read_regular(&self.root.path().join(name))?;
                        if file.metadata()?.nlink() != 1 {
                            return Err(invalid(
                                "original container cache is not a single-link regular file",
                            ));
                        }
                    }
                    file_database::NAME => {}
                    _ => {
                        return Err(invalid(
                            "unknown original FILE container child retained without adoption",
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}
