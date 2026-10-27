//! Hash-prefix object layout. Unsupported flat objects fail closed; readers
//! never migrate, delete, reuse, or accept them as current-format evidence.
use super::*;
use std::{collections::BTreeSet, fs};

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn validate(kind: &str, hash: &str) -> Result<()> {
    if kind.is_empty()
        || !kind.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
        || !valid_hash(hash)
    {
        return Err(Error::new(
            storage_contract::ErrorCode::ObjectCorrupt,
            "invalid immutable object address",
        ));
    }
    Ok(())
}
fn flat_path(d: &DiskConfig, kind: &str, hash: &str) -> PathBuf {
    d.root
        .join("metadata/objects")
        .join(kind)
        .join(format!("{hash}.json"))
}
pub(crate) fn object_path(d: &DiskConfig, kind: &str, hash: &str) -> PathBuf {
    // All I/O entry points validate before using this pure path constructor.
    d.root
        .join("metadata/objects")
        .join(kind)
        .join(hash.get(..2).unwrap_or(hash))
        .join(format!("{hash}.json"))
}
fn directory_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            existing_private_dir(path)?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
fn kind_directory(d: &DiskConfig, kind: &str) -> Result<Option<PathBuf>> {
    let mut path = d.root.clone();
    if !directory_exists(&path)? {
        return Ok(None);
    }
    for component in ["metadata", "objects", kind] {
        path.push(component);
        if !directory_exists(&path)? {
            return Ok(None);
        }
    }
    Ok(Some(path))
}
fn add_existing(path: PathBuf, out: &mut Vec<PathBuf>) -> Result<()> {
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            open_regular(&path)?;
            out.push(path);
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    Ok(())
}
pub(crate) fn existing_object_paths(
    d: &DiskConfig,
    kind: &str,
    hash: &str,
) -> Result<Vec<PathBuf>> {
    validate(kind, hash)?;
    let mut paths = Vec::new();
    if let Some(directory) = kind_directory(d, kind)? {
        // Reject old-format evidence rather than silently creating a second
        // representation or treating an unsupported object as absent.
        match fs::symlink_metadata(flat_path(d, kind, hash)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
            Ok(_) => {
                return Err(Error::new(
                    storage_contract::ErrorCode::FormatUnsupported,
                    "flat metadata object layout is unsupported",
                ));
            }
        }
        if directory_exists(&directory.join(&hash[..2]))? {
            add_existing(object_path(d, kind, hash), &mut paths)?;
        }
    }
    Ok(paths)
}
pub(super) fn read_object_bytes_bounded(
    d: &DiskConfig,
    kind: &str,
    hash: &str,
    maximum: usize,
) -> Result<Vec<u8>> {
    read_object_bytes_with(d, kind, hash, |path| {
        read_bytes(path, maximum.min(MAX_METADATA))
    })
}

pub(super) fn read_object_bytes_observed(
    d: &DiskConfig,
    kind: &str,
    hash: &str,
    maximum: usize,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<Vec<u8>> {
    read_object_bytes_with(d, kind, hash, |path| {
        read_bytes_observed(path, maximum.min(MAX_METADATA), observe)
    })
}

fn read_object_bytes_with(
    d: &DiskConfig,
    kind: &str,
    hash: &str,
    mut read: impl FnMut(&Path) -> Result<Vec<u8>>,
) -> Result<Vec<u8>> {
    let paths = existing_object_paths(d, kind, hash)?;
    let path = paths.first().ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::ObjectCorrupt,
            "immutable object is missing",
        )
    })?;
    read(path)
}
pub(super) fn write_object_bytes(
    d: &DiskConfig,
    kind: &str,
    hash: &str,
    bytes: &[u8],
) -> Result<()> {
    write_object_bytes_with(d, kind, hash, bytes, |path, bytes| {
        write_atomic(path, bytes, false)
    })
}

pub(super) fn write_object_bytes_with(
    d: &DiskConfig,
    kind: &str,
    hash: &str,
    bytes: &[u8],
    publish: impl FnOnce(&Path, &[u8]) -> Result<()>,
) -> Result<()> {
    existing_object_paths(d, kind, hash)?;
    // The immutable writer validates existing bytes and completes durability.
    publish(&object_path(d, kind, hash), bytes)
}
/// Inventory both layouts, rejecting unexpected entries rather than claiming
/// absence when unreadable evidence could contain a publication.
pub(crate) fn list_object_hashes(d: &DiskConfig, kind: &str) -> Result<BTreeSet<String>> {
    validate(kind, &"0".repeat(64))?;
    let mut hashes = BTreeSet::new();
    let Some(directory) = kind_directory(d, kind)? else {
        return Ok(hashes);
    };
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        let name = entry.file_name().into_string().map_err(|name| {
            Error::PathEncoding(name).context(ErrorCode::ObjectCorrupt, "invalid object name")
        })?;
        if name.len() == 2
            && name
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            existing_private_dir(&entry.path())?;
            for child in fs::read_dir(entry.path())? {
                let child = child?;
                let hash = inventory_hash(&child.path())?;
                if !hash.starts_with(&name) {
                    return Err(Error::new(
                        storage_contract::ErrorCode::ObjectCorrupt,
                        "object is in the wrong shard",
                    ));
                }
                hashes.insert(hash);
            }
        } else {
            return Err(Error::new(
                storage_contract::ErrorCode::FormatUnsupported,
                "expected a metadata object hash-prefix directory",
            ));
        }
    }
    Ok(hashes)
}
fn inventory_hash(path: &Path) -> Result<String> {
    let hash = path
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_suffix(".json"))
        .filter(|hash| valid_hash(hash))
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::ObjectCorrupt,
                "invalid object filename",
            )
        })?;
    open_regular(path)?;
    Ok(hash.to_owned())
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/journal/object_storage/tests.rs"]
mod tests;
