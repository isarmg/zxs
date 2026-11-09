//! Closed image inventory and independently copied immutable payloads.
use super::*;
use crate::history_package::{install_preparation_bytes, preparation_path_exists};
use std::io::Read;

fn inventory(root: &DiskRoot, image: &Image<'_>, complete: bool, stop: &AtomicBool) -> Result<()> {
    let path = root.path().join(image.root_name);
    if !preparation_path_exists(root, &path)? {
        return if complete {
            Err(invalid("target entity image is missing"))
        } else {
            Ok(())
        };
    }
    let held = root.private_child(&path)?;
    let mut retained_staging = 0usize;
    let mut pending = vec![PathBuf::new()];
    let mut seen = BTreeSet::new();
    while let Some(parent) = pending.pop() {
        for entry in held.entries(&held.path().join(&parent))? {
            preparation_checkpoint(stop)?;
            let name = entry.map_err(fs_error)?;
            let child = parent.join(name.as_path());
            if image.root_name == super::serving::PUBLISHED_ROOT
                && child != Path::new(MANIFEST)
                && !image.directories.contains(&child)
                && !image.files.contains_key(&child)
                && platform_io::fs_safety::AtomicFile::is_temporary_name(&name)
            {
                retained_staging += 1;
                if retained_staging > 1024 {
                    return Err(budget());
                }
                // Interrupted private writes are not entities or completion
                // evidence. Keep them, bounded, without promoting or removing.
                held.open_read_regular(&held.path().join(&parent).join(name.as_path()))?;
                continue;
            }
            if !seen.insert(child.clone()) || seen.len() > MAX_ENTRIES + 1 {
                return Err(budget());
            }
            let path = held.path().join(&child);
            if image.directories.contains(&child) {
                held.private_child(&path)?;
                pending.push(child);
            } else if child == Path::new(MANIFEST) || image.files.contains_key(&child) {
                held.open_regular(&path)?;
            } else {
                return Err(Error::new(
                    ErrorCode::ObjectCollision,
                    "unknown target image entry retained; it cannot be adopted or removed",
                ));
            }
        }
    }
    if complete && seen.len() != image.directories.len() + image.files.len() + 1 {
        return Err(invalid(
            "target image omits part of its complete entity plan",
        ));
    }
    Ok(())
}

fn verify_file(root: &DiskRoot, path: &Path, data: &Data<'_>, stop: &AtomicBool) -> Result<()> {
    match data {
        Data::Bytes(bytes) => {
            if root.read_bytes(path, bytes.len())? != *bytes {
                return Err(invalid(
                    "installed image metadata differs from its original frozen unit",
                ));
            }
        }
        Data::Body { version, .. } => {
            let mut file = root.open_regular(path)?;
            platform_io::payload::verify_full_observed(
                &mut file,
                expectation(version),
                &mut || preparation_checkpoint(stop).map_err(std::io::Error::other),
            )?;
        }
    }
    Ok(())
}
fn expectation(version: &FileVersion) -> platform_io::payload::Expectation {
    platform_io::payload::Expectation {
        bytes: version.logical_size,
        blake3: Some(version.content_hash.digest()),
    }
}

pub(super) fn require_capacity(
    root: &DiskRoot,
    image: &Image<'_>,
    stop: &AtomicBool,
) -> Result<()> {
    inventory(root, image, false, stop)?;
    let base = root.path().join(image.root_name);
    let mut missing = 16u64 * 1024 * 1024;
    // Include a bounded conservative directory allocation allowance. It is a
    // preflight, not a promise against concurrent consumers or filesystem ENOSPC.
    missing = missing
        .checked_add(
            (image.directories.len() as u64)
                .checked_mul(4096)
                .ok_or_else(budget)?,
        )
        .ok_or_else(budget)?;
    for (path, data) in &image.files {
        preparation_checkpoint(stop)?;
        let path = base.join(path);
        if preparation_path_exists(root, &path)? {
            verify_file(root, &path, data, stop)?;
        } else {
            missing = missing
                .checked_add(data.descriptor().bytes)
                .ok_or_else(budget)?;
        }
    }
    let manifest = base.join(MANIFEST);
    if preparation_path_exists(root, &manifest)? {
        if root.read_bytes(&manifest, MAX_MANIFEST)? != image.manifest {
            return Err(invalid(
                "target image belongs to a different source fence or entity plan",
            ));
        }
    } else {
        missing = missing
            .checked_add(image.manifest.len() as u64)
            .ok_or_else(budget)?;
    }
    if root.available()? < missing {
        return Err(Error::new(
            ErrorCode::NoSpace,
            "target cannot retain a complete independent entity image beside original prepared bytes",
        ));
    }
    Ok(())
}
struct CopyInput<'a, R> {
    inner: R,
    stop: &'a AtomicBool,
}
impl<R: Read> Read for CopyInput<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        preparation_checkpoint(self.stop).map_err(std::io::Error::other)?;
        self.inner.read(bytes)
    }
}

pub(super) fn install(root: &DiskRoot, image: &Image<'_>, stop: &AtomicBool) -> Result<()> {
    require_capacity(root, image, stop)?;
    let base = root.path().join(image.root_name);
    root.private_dir(&base)?;
    install_preparation_bytes(root, &base.join(MANIFEST), &image.manifest)?;
    for directory in &image.directories {
        preparation_checkpoint(stop)?;
        root.private_dir(&base.join(directory))?;
    }
    for (relative, data) in &image.files {
        preparation_checkpoint(stop)?;
        let path = base.join(relative);
        match data {
            Data::Bytes(bytes) => install_preparation_bytes(root, &path, bytes)?,
            Data::Body { version, source } => {
                let mut reader = CopyInput {
                    inner: root.open_read_regular(source)?,
                    stop,
                };
                root.copy_verified_bytes_synced(&path, &mut reader, expectation(version))?;
            }
        }
    }
    verify(root, image, stop)
}
pub(super) fn verify(root: &DiskRoot, image: &Image<'_>, stop: &AtomicBool) -> Result<()> {
    inventory(root, image, true, stop)?;
    let base = root.path().join(image.root_name);
    if root.read_bytes(&base.join(MANIFEST), MAX_MANIFEST)? != image.manifest {
        return Err(invalid(
            "target image manifest differs from the complete original plan",
        ));
    }
    for (relative, data) in &image.files {
        preparation_checkpoint(stop)?;
        verify_file(root, &base.join(relative), data, stop)?;
    }
    Ok(())
}
