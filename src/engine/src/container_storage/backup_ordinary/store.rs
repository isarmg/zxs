//! Exact ordinary namespaces, no-clobber copies and real normal-reader
//! readback. Completed identity records are compared, never repaired here.
use super::*;
use platform_io::fs_safety::linux::FileIdentity;
use std::{io::Read, os::unix::fs::MetadataExt};

fn scopes<'image>(image: &'image Image<'_>) -> impl Iterator<Item = PathBuf> + 'image {
    image.histories.iter().flat_map(|history| {
        SCOPES
            .into_iter()
            .map(move |name| PathBuf::from("histories").join(history).join(name))
    })
}
pub(super) fn require_absent(root: &DiskRoot, image: &Image<'_>, stop: &AtomicBool) -> Result<()> {
    for scope in scopes(image) {
        preparation_checkpoint(stop)?;
        if preparation_path_exists(root, &root.path().join(scope))? {
            return Err(invalid(
                "unselected ordinary Backup namespace is preserved without adoption",
            ));
        }
    }
    Ok(())
}

fn inventory(root: &DiskRoot, image: &Image<'_>, stop: &AtomicBool) -> Result<()> {
    let mut pending = Vec::new();
    for scope in scopes(image) {
        preparation_checkpoint(stop)?;
        if preparation_path_exists(root, &root.path().join(&scope))? {
            pending.push(scope);
        }
    }
    let mut count = 0usize;
    let mut temporary = 0usize;
    while let Some(path) = pending.pop() {
        preparation_checkpoint(stop)?;
        if count == MAX_ENTRIES {
            return Err(budget());
        }
        count += 1;
        if !image.directories.contains(&path) {
            return Err(invalid("unknown ordinary Backup directory retained"));
        }
        let held = root.private_child(&root.path().join(&path))?;
        for entry in held.entries(held.path())? {
            preparation_checkpoint(stop)?;
            let name = entry.map_err(fs_error)?;
            let child = path.join(name.as_path());
            if image.directories.contains(&child) {
                pending.push(child);
            } else if image.files.contains_key(&child) || image.locator_paths.contains(&child) {
                regular(root, &root.path().join(child))?;
                if count == MAX_ENTRIES {
                    return Err(budget());
                }
                count += 1;
            } else if platform_io::fs_safety::AtomicFile::is_temporary_name(&name) {
                temporary += 1;
                if temporary > 1024 {
                    return Err(budget());
                }
                regular(root, &root.path().join(child))?;
            } else {
                return Err(invalid(
                    "unknown ordinary Backup file retained without removal",
                ));
            }
        }
    }
    Ok(())
}
fn regular(root: &DiskRoot, path: &Path) -> Result<std::fs::Metadata> {
    let file = root.open_read_regular(path)?;
    let metadata = file.metadata()?;
    if metadata.nlink() != 1 {
        return Err(invalid("ordinary Backup file is not independently owned"));
    }
    Ok(metadata)
}
fn expectation(version: &FileVersion) -> platform_io::payload::Expectation {
    platform_io::payload::Expectation {
        bytes: version.logical_size,
        blake3: Some(version.content_hash.digest()),
    }
}
fn verify_file(
    root: &DiskRoot,
    path: &Path,
    data: &Data<'_>,
    stop: &AtomicBool,
) -> Result<FileIdentity> {
    let mut file = root.open_read_regular(path)?;
    let metadata = file.metadata()?;
    let identity = FileIdentity::from_metadata(&metadata);
    let stamp = crate::file_stamp::FileStamp::from_metadata(&metadata);
    if metadata.nlink() != 1 || metadata.len() != data.descriptor().bytes {
        return Err(invalid(
            "ordinary Backup file has a changed length or link count",
        ));
    }
    match data {
        Data::Bytes(bytes) => {
            let mut actual = vec![0; bytes.len()];
            file.read_exact(&mut actual)?;
            if actual != *bytes || file.read(&mut [0; 1])? != 0 {
                return Err(invalid(
                    "ordinary Backup metadata differs from the complete published State",
                ));
            }
        }
        Data::Body(version) => {
            let directory = root
                .path()
                .join("histories")
                .join(&version.history_tree_id)
                .join("versions")
                .join(&version.version_id);
            let checks = read_authenticated_checksums_at(root, &directory, version)?;
            let manifest = decode_authenticated_checksums(&checks, version)?;
            file = verify_open_payload_observed(file, manifest, version, &mut |_| {
                preparation_checkpoint(stop)
            })?;
        }
    }
    if crate::file_stamp::FileStamp::from_metadata(&file.metadata()?) != stamp
        || FileIdentity::from_metadata(&regular(root, path)?) != identity
    {
        return Err(invalid("ordinary Backup file was replaced during readback"));
    }
    file.check_running()?;
    Ok(identity)
}
fn directory_intent(
    root: &DiskRoot,
    image: &Image<'_>,
    directory: &Directory,
) -> Result<DirectoryIntent> {
    let history = root.path().join("histories").join(&directory.history);
    DirectoryIntent::new(
        &directory.history,
        directory.id.clone(),
        image.binding.clone(),
        EntityInode::capture(root, &history.join(directory.to.as_path()))?,
        directory.to.clone(),
        directory.to.clone(),
    )
}
fn file_intent(root: &DiskRoot, image: &Image<'_>, file: &FileHome) -> Result<FileIntent> {
    FileIntent::new(
        &file.history,
        &file.id,
        image.binding.clone(),
        EntityInode::capture(root, &root.path().join(&file.path))?,
        file.address.clone(),
        file.address.clone(),
    )
}
fn verify_locators(
    root: &DiskRoot,
    image: &Image<'_>,
    complete: bool,
    stop: &AtomicBool,
) -> Result<BTreeMap<PathBuf, FileIdentity>> {
    let mut bindings = BTreeMap::new();
    for directory in image.directory_homes.values() {
        preparation_checkpoint(stop)?;
        let path = root
            .path()
            .join("histories")
            .join(&directory.history)
            .join(".zxs-layout/directories")
            .join(format!("{}.json", directory.id.as_str()));
        if !complete && !preparation_path_exists(root, &path)? {
            continue;
        }
        let history = root.path().join("histories").join(&directory.history);
        let anchor = HistoryAnchor::new(root, &history, &directory.history)?;
        let stored = layout_locator::load_directory(&anchor, &directory.id, &mut |_| {
            preparation_checkpoint(stop)
        })?;
        let expected = directory_intent(root, image, directory)?;
        if stored != expected
            || layout_locator::locate_directory(&anchor, &stored, &mut |_| {
                preparation_checkpoint(stop)
            })? != history.join(directory.to.as_path())
        {
            return Err(invalid(
                "ordinary Backup directory locator changed its original owner, inode or address",
            ));
        }
        bindings.insert(
            path.strip_prefix(root.path())
                .expect("invariant: generated locator below root")
                .to_owned(),
            verify_file(root, &path, &Data::Bytes(expected.canonical_bytes()?), stop)?,
        );
    }
    for file in image.file_homes.values() {
        preparation_checkpoint(stop)?;
        let history = root.path().join("histories").join(&file.history);
        let path = history
            .join(".zxs-layout/files")
            .join(format!("{}.json", file.id));
        if !complete && !preparation_path_exists(root, &path)? {
            continue;
        }
        let anchor = HistoryAnchor::new(root, &history, &file.history)?;
        let stored =
            layout_locator::load_file(&anchor, &file.id, &mut |_| preparation_checkpoint(stop))?;
        let expected = file_intent(root, image, file)?;
        if stored != expected
            || layout_locator::locate_file(&anchor, &stored, &mut |_| preparation_checkpoint(stop))?
                != root.path().join(&file.path)
        {
            return Err(invalid(
                "ordinary Backup FILE locator changed its original owner, inode or address",
            ));
        }
        bindings.insert(
            path.strip_prefix(root.path())
                .expect("invariant: generated locator below root")
                .to_owned(),
            verify_file(root, &path, &Data::Bytes(expected.canonical_bytes()?), stop)?,
        );
    }
    Ok(bindings)
}

pub(super) fn require_capacity(
    root: &DiskRoot,
    image: &Image<'_>,
    reserve: u64,
    stop: &AtomicBool,
) -> Result<()> {
    inventory(root, image, stop)?;
    verify_locators(root, image, false, stop)?;
    let mut missing = 0u64;
    let mut largest = 0u64;
    for (path, data) in &image.files {
        preparation_checkpoint(stop)?;
        let path = root.path().join(path);
        if preparation_path_exists(root, &path)? {
            verify_file(root, &path, data, stop)?;
        } else {
            let bytes = data.descriptor().bytes;
            missing = missing.checked_add(bytes).ok_or_else(budget)?;
            largest = largest.max(bytes);
        }
    }
    let required = missing
        .checked_add(largest)
        .and_then(|n| n.checked_add(reserve))
        .and_then(|n| n.checked_add(MAX_MANIFEST as u64 + 1024 * 1024))
        .and_then(|n| {
            n.checked_add(
                (image.directories.len() + image.files.len() + image.locator_paths.len()) as u64
                    * 8192,
            )
        })
        .ok_or_else(budget)?;
    if root.available()? < required {
        return Err(Error::new(
            ErrorCode::NoSpace,
            "ordinary Backup copies need missing bytes, atomic staging, inode manifest and metadata reserve",
        ));
    }
    Ok(())
}
struct Input<'a, R> {
    inner: R,
    stop: &'a AtomicBool,
}
impl<R: Read> Read for Input<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        preparation_checkpoint(self.stop).map_err(std::io::Error::other)?;
        self.inner.read(bytes)
    }
}
pub(super) fn install(root: &DiskRoot, image: &Image<'_>, stop: &AtomicBool) -> Result<()> {
    inventory(root, image, stop)?;
    verify_locators(root, image, false, stop)?;
    for path in &image.directories {
        preparation_checkpoint(stop)?;
        root.private_dir(&root.path().join(path))?;
    }
    for (path, data) in &image.files {
        preparation_checkpoint(stop)?;
        let destination = root.path().join(path);
        if preparation_path_exists(root, &destination)? {
            verify_file(root, &destination, data, stop)?;
            continue;
        }
        match data {
            Data::Bytes(bytes) => install_preparation_bytes(root, &destination, bytes)?,
            Data::Body(version) => {
                // Copy, never move: every prior capsule/image remains usable by
                // the exact original recovery path throughout partial progress.
                let source = root
                    .path()
                    .join("histories")
                    .join(&version.history_tree_id)
                    .join("versions")
                    .join(&version.version_id)
                    .join("content")
                    .join(&version.creation_name);
                let mut input = Input {
                    inner: root.open_read_regular(&source)?,
                    stop,
                };
                root.copy_verified_bytes_synced(&destination, &mut input, expectation(version))?;
            }
        }
    }
    for directory in image.directory_homes.values() {
        preparation_checkpoint(stop)?;
        let history = root.path().join("histories").join(&directory.history);
        let anchor = HistoryAnchor::new(root, &history, &directory.history)?;
        layout_locator::store_directory(&anchor, &directory_intent(root, image, directory)?, None)?;
    }
    for file in image.file_homes.values() {
        preparation_checkpoint(stop)?;
        let history = root.path().join("histories").join(&file.history);
        let anchor = HistoryAnchor::new(root, &history, &file.history)?;
        layout_locator::store_file(&anchor, &file_intent(root, image, file)?, None)?;
    }
    // Existing files can follow an uncertain atomic publish. Finish their own
    // barriers even on retries, without deleting or replacing any original.
    for path in image.files.keys().chain(&image.locator_paths) {
        root.open_read_regular(&root.path().join(path))?
            .sync_all()?;
    }
    for path in image.directories.iter().rev() {
        preparation_checkpoint(stop)?;
        root.sync_dir(&root.path().join(path))?;
    }
    Ok(())
}

#[derive(Serialize)]
struct Binding {
    #[serialization(with = "storage_contract::u64_string")]
    device: u64,
    #[serialization(with = "storage_contract::u64_string")]
    inode: u64,
}
impl From<FileIdentity> for Binding {
    fn from(value: FileIdentity) -> Self {
        Self {
            device: value.device,
            inode: value.inode,
        }
    }
}
pub(super) fn verify(root: &DiskRoot, image: &Image<'_>, stop: &AtomicBool) -> Result<Vec<u8>> {
    inventory(root, image, stop)?;
    let mut bindings = BTreeMap::<PathBuf, Binding>::new();
    for path in &image.directories {
        preparation_checkpoint(stop)?;
        bindings.insert(
            path.clone(),
            root.directory_identity(&root.path().join(path))?.into(),
        );
    }
    for (path, identity) in verify_locators(root, image, true, stop)? {
        bindings.insert(path, identity.into());
    }
    for (path, data) in &image.files {
        preparation_checkpoint(stop)?;
        let absolute = root.path().join(path);
        let identity = verify_file(root, &absolute, data, stop)?;
        if let Data::Body(version) = data {
            let directory = root
                .path()
                .join("histories")
                .join(&version.history_tree_id)
                .join("versions")
                .join(&version.version_id);
            if super::super::content_path_at(root, &directory, version, &mut |_| {
                preparation_checkpoint(stop)
            })? != absolute
            {
                return Err(invalid(
                    "ordinary Backup reader selected a package or another container instead of its exact entity",
                ));
            }
            let (file, _, manifest) = open_payload_at(root, &directory, version)?;
            drop(verify_open_payload_observed(
                file,
                manifest,
                version,
                &mut |_| preparation_checkpoint(stop),
            )?);
        }
        if FileIdentity::from_metadata(&regular(root, &absolute)?) != identity {
            return Err(invalid(
                "ordinary Backup reader lost its verified body inode",
            ));
        }
        bindings.insert(path.clone(), identity.into());
    }
    #[derive(Serialize)]
    struct Completion {
        format: &'static str,
        recipe_hash: String,
        root: Binding,
        entries: BTreeMap<PathBuf, Binding>,
    }
    let bytes = canonical_json(&Completion {
        format: "zxs-backup-ordinary-serving-entities",
        recipe_hash: image.hash(),
        root: root.directory_identity(root.path())?.into(),
        entries: bindings,
    })?;
    if bytes.len() > MAX_MANIFEST {
        return Err(budget());
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/backup_ordinary_entities_tests.rs"]
mod tests;
