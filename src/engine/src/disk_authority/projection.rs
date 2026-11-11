//! Hold the selected target through source verification and all projection I/O.
use crate::disk_authority::{
    Error, Result,
    cache::{self, CachedCopy},
    journal::DiskSelection,
    location,
    model::{MAX_BYTES, MAX_OBJECTS, MAX_PUBLICATIONS},
    storage::Disk,
};
use crate::persist::layout;
use platform_io::fs_safety::{AtomicFile, EntryName, PrivateDirectory};
use std::path::{Path, PathBuf};

pub(super) struct Destination {
    path: PathBuf,
    parent: PrivateDirectory,
    name: EntryName,
    existing: Option<PrivateDirectory>,
    location: location::Location,
    sources: Vec<location::Location>,
}
impl Destination {
    pub(super) fn open(path: &Path) -> Result<Self> {
        let path = crate::disk::absolute_path(path).map_err(Error::EngineBoundary)?;
        let parent_path = path.parent().ok_or(Error::Invalid(
            "projection needs an existing private parent",
        ))?;
        let name = EntryName::new(
            path.file_name()
                .ok_or(Error::Invalid("projection name is required"))?,
        )?;
        let parent_location = location::Location::open(parent_path)?;
        let parent = parent_location.private_directory(parent_path)?;
        let (existing, location) = match location::Location::open(&path) {
            Ok(location) => (Some(location.private_directory(&path)?), location),
            Err(Error::Filesystem(platform_io::fs_safety::Error::Io(error)))
                if error.kind() == std::io::ErrorKind::NotFound =>
            {
                (None, parent_location)
            }
            Err(error) => return Err(error),
        };
        parent.verify_identity()?;
        Ok(Self {
            path,
            parent,
            name,
            existing,
            location,
            sources: vec![],
        })
    }
    pub(super) fn admit(&mut self, disk: &Disk) -> Result<()> {
        if disk.path().starts_with(&self.path) || self.path.starts_with(disk.path()) {
            return Err(Error::Invalid(
                "projection directory overlaps a selected data disk",
            ));
        }
        let source = disk.location()?;
        if (self.existing.is_some() && self.location.overlaps(&source)?)
            || (self.existing.is_none()
                && self
                    .location
                    .descends_from(source.identity(), source.mount_root())?)
        {
            return Err(Error::Invalid("projection physically overlaps a data disk"));
        }
        self.sources.push(source);
        Ok(())
    }
    pub(super) fn open_for_write(self) -> Result<PrivateDirectory> {
        self.parent.verify_identity()?;
        let (directory, actual) = match self.existing {
            Some(directory) => (directory, self.location.refreshed()?),
            None => self
                .location
                .create_child_exclusive(&self.name, &self.path)?,
        };
        self.parent.verify_identity()?;
        directory.verify_identity()?;
        // mkdirat returning success does not reserve the name until open. Both
        // sides retain the reviewed handles, so a source moved into that name
        // cannot be mistaken for a newly created, unrelated cache directory.
        for source in self.sources {
            if actual.overlaps(&source.refreshed()?)? {
                return Err(Error::Invalid("projection physically overlaps a data disk"));
            }
        }
        // Creation barriers run only after the actual child has been admitted.
        directory.sync()?;
        self.parent.sync()?;
        Ok(directory)
    }
}

/// Only a dedicated projection directory outside every selected data root is
/// writable. An already-held parent/child owns writes, never a re-opened path.
pub fn rebuild_projection(path: &Path, selections: &[DiskSelection]) -> Result<Vec<CachedCopy>> {
    if selections.len() > MAX_PUBLICATIONS {
        return Err(Error::Budget);
    }
    let mut destination = Destination::open(path)?;
    let mut verified = Vec::with_capacity(selections.len());
    let mut retained_bytes = 0usize;
    let mut retained_rows = 0usize;
    for selection in selections {
        let disk = Disk::open(&selection.path, &selection.identity)?;
        destination.admit(&disk)?;
        let snapshot = disk.verify(selection.known_checkpoint)?;
        let bytes: usize = snapshot
            .bundle
            .objects
            .values()
            .chain(snapshot.bundle.bodies.values())
            .map(Vec::len)
            .sum();
        retained_bytes = retained_bytes.checked_add(bytes).ok_or(Error::Budget)?;
        retained_rows = retained_rows
            .checked_add(snapshot.rows.len())
            .ok_or(Error::Budget)?;
        if retained_bytes > MAX_BYTES || retained_rows > MAX_OBJECTS {
            return Err(Error::Budget);
        }
        verified.push(snapshot);
    }
    let directory = destination.open_for_write()?;
    let bytes = br#"{"format":"independent-disk-projection"}"#;
    match directory.read_bounded(&layout::PROJECTION_KIND, 256) {
        Ok(existing) if existing == bytes => {}
        Ok(_) => return Err(Error::Invalid("projection format differs")),
        Err(platform_io::fs_safety::Error::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            if std::fs::read_dir(directory.path())?
                .next()
                .transpose()?
                .is_some()
            {
                return Err(Error::Invalid("new projection directory must be empty"));
            }
            AtomicFile::create(&directory, &layout::PROJECTION_KIND, bytes)?;
        }
        Err(error) => return Err(error.into()),
    }
    let published = cache::prepare(directory, &verified)?.publish()?;
    published.read()
}
