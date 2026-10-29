//! Hash-prefix object layout. Unsupported flat objects fail closed; readers
//! never migrate, delete, reuse, or accept them as current-format evidence.
use super::*;
use std::collections::BTreeSet;
#[cfg(test)]
use std::fs;

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

/// Parsed object coordinates, never an arbitrary disk-relative write path.
/// The fixed kind/shard directories are not renamed by the object protocol.
pub(crate) struct ObjectAddress<'a> {
    kind: &'a str,
    hash: &'a str,
}

impl<'a> ObjectAddress<'a> {
    pub(crate) fn parse(kind: &'a str, hash: &'a str) -> Result<Self> {
        validate(kind, hash)?;
        Ok(Self { kind, hash })
    }

    pub(crate) fn path(&self, root: &Path) -> PathBuf {
        sharded_path(root, self.kind, self.hash)
    }

    pub(crate) fn existing_paths(&self, root: &DiskRoot, d: &DiskConfig) -> Result<Vec<PathBuf>> {
        existing_paths(root, d, self.kind, self.hash)
    }
}
fn flat_path(d: &DiskConfig, kind: &str, hash: &str) -> PathBuf {
    d.root
        .join("metadata/objects")
        .join(kind)
        .join(format!("{hash}.json"))
}
pub(crate) fn object_path(d: &DiskConfig, kind: &str, hash: &str) -> PathBuf {
    // All I/O entry points validate before using this pure path constructor.
    sharded_path(&d.root, kind, hash)
}
fn sharded_path(root: &Path, kind: &str, hash: &str) -> PathBuf {
    root.join("metadata/objects")
        .join(kind)
        .join(hash.get(..2).unwrap_or(hash))
        .join(format!("{hash}.json"))
}
fn directory_exists(root: &DiskRoot, path: &Path) -> Result<bool> {
    match root.optional_metadata(path)? {
        Some(_) => {
            root.directory(path)?;
            Ok(true)
        }
        None => Ok(false),
    }
}
fn kind_directory(root: &DiskRoot, d: &DiskConfig, kind: &str) -> Result<Option<PathBuf>> {
    let mut path = d.root.clone();
    for component in ["metadata", "objects", kind] {
        path.push(component);
        if !directory_exists(root, &path)? {
            return Ok(None);
        }
    }
    Ok(Some(path))
}
fn add_existing(root: &DiskRoot, path: PathBuf, out: &mut Vec<PathBuf>) -> Result<()> {
    if root.open_optional_regular(&path)?.is_some() {
        out.push(path);
    }
    Ok(())
}
#[cfg(test)]
pub(crate) fn existing_object_paths(
    d: &DiskConfig,
    kind: &str,
    hash: &str,
) -> Result<Vec<PathBuf>> {
    match open_optional_root(d)? {
        Some(root) => existing_object_paths_at(&root, d, kind, hash),
        None => {
            validate(kind, hash)?;
            Ok(Vec::new())
        }
    }
}

#[cfg(test)]
fn open_optional_root(d: &DiskConfig) -> Result<Option<DiskRoot>> {
    match fs::symlink_metadata(&d.root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
        Ok(_) => DiskRoot::open(&d.root).map(Some),
    }
}

pub(crate) fn existing_object_paths_at(
    root: &DiskRoot,
    d: &DiskConfig,
    kind: &str,
    hash: &str,
) -> Result<Vec<PathBuf>> {
    ObjectAddress::parse(kind, hash)?.existing_paths(root, d)
}

fn existing_paths(
    root: &DiskRoot,
    d: &DiskConfig,
    kind: &str,
    hash: &str,
) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    if let Some(directory) = kind_directory(root, d, kind)? {
        // Reject old-format evidence rather than silently creating a second
        // representation or treating an unsupported object as absent.
        if root.optional_metadata(&flat_path(d, kind, hash))?.is_some() {
            return Err(Error::new(
                storage_contract::ErrorCode::FormatUnsupported,
                "flat metadata object layout is unsupported",
            ));
        }
        if directory_exists(root, &directory.join(&hash[..2]))? {
            add_existing(root, object_path(d, kind, hash), &mut paths)?;
        }
    }
    Ok(paths)
}
#[cfg(test)]
pub(super) fn read_object_bytes_bounded(
    d: &DiskConfig,
    kind: &str,
    hash: &str,
    maximum: usize,
) -> Result<Vec<u8>> {
    let root = open_optional_root(d)?.ok_or_else(missing_object)?;
    read_object_bytes_bounded_at(&root, d, kind, hash, maximum)
}

pub(super) fn read_object_bytes_bounded_at(
    root: &DiskRoot,
    d: &DiskConfig,
    kind: &str,
    hash: &str,
    maximum: usize,
) -> Result<Vec<u8>> {
    read_object_bytes_with(root, d, kind, hash, |path| {
        root.read_bytes(path, maximum.min(MAX_METADATA))
    })
}

pub(super) fn read_object_bytes_observed_at(
    root: &DiskRoot,
    d: &DiskConfig,
    kind: &str,
    hash: &str,
    maximum: usize,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<Vec<u8>> {
    read_object_bytes_with(root, d, kind, hash, |path| {
        root.read_bytes_observed(path, maximum.min(MAX_METADATA), observe)
    })
}

fn read_object_bytes_with(
    root: &DiskRoot,
    d: &DiskConfig,
    kind: &str,
    hash: &str,
    mut read: impl FnMut(&Path) -> Result<Vec<u8>>,
) -> Result<Vec<u8>> {
    let paths = existing_object_paths_at(root, d, kind, hash)?;
    let path = paths.first().ok_or_else(missing_object)?;
    read(path)
}
fn missing_object() -> Error {
    Error::new(
        storage_contract::ErrorCode::ObjectCorrupt,
        "immutable object is missing",
    )
}
#[cfg(any(test, feature = "test-support"))]
pub(super) fn write_object_bytes(
    d: &DiskConfig,
    kind: &str,
    hash: &str,
    bytes: &[u8],
) -> Result<()> {
    private_dir(&d.root)?;
    write_object_bytes_at(&DiskRoot::open(&d.root)?, d, kind, hash, bytes)
}

pub(super) fn write_object_bytes_at(
    root: &DiskRoot,
    d: &DiskConfig,
    kind: &str,
    hash: &str,
    bytes: &[u8],
) -> Result<()> {
    let address = ObjectAddress::parse(kind, hash)?;
    address.existing_paths(root, d)?;
    root.write_immutable_object(&address, bytes, false)
}

#[cfg(test)]
pub(super) fn write_object_bytes_with(
    d: &DiskConfig,
    kind: &str,
    hash: &str,
    bytes: &[u8],
    publish: impl FnOnce(&Path, &[u8]) -> Result<()>,
) -> Result<()> {
    private_dir(&d.root)?;
    write_object_bytes_with_at(&DiskRoot::open(&d.root)?, d, kind, hash, bytes, publish)
}

#[cfg(test)]
fn write_object_bytes_with_at(
    root: &DiskRoot,
    d: &DiskConfig,
    kind: &str,
    hash: &str,
    bytes: &[u8],
    publish: impl FnOnce(&Path, &[u8]) -> Result<()>,
) -> Result<()> {
    let address = ObjectAddress::parse(kind, hash)?;
    address.existing_paths(root, d)?;
    // The immutable writer validates existing bytes and completes durability.
    publish(&address.path(&d.root), bytes)
}
/// Inventory both layouts, rejecting unexpected entries rather than claiming
/// absence when unreadable evidence could contain a publication.
#[cfg(test)]
pub(crate) fn list_object_hashes(d: &DiskConfig, kind: &str) -> Result<BTreeSet<String>> {
    match open_optional_root(d)? {
        Some(root) => list_object_hashes_at(&root, d, kind),
        None => {
            validate(kind, &"0".repeat(64))?;
            Ok(BTreeSet::new())
        }
    }
}

pub(crate) fn list_object_hashes_at(
    root: &DiskRoot,
    d: &DiskConfig,
    kind: &str,
) -> Result<BTreeSet<String>> {
    validate(kind, &"0".repeat(64))?;
    let mut hashes = BTreeSet::new();
    let Some(directory) = kind_directory(root, d, kind)? else {
        return Ok(hashes);
    };
    for entry in root.entries(&directory)? {
        let entry = entry?;
        let name = entry.as_os_str().to_str().ok_or_else(|| {
            Error::PathEncoding(entry.as_os_str().to_owned())
                .context(ErrorCode::ObjectCorrupt, "invalid object name")
        })?;
        if name.len() == 2
            && name
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            let shard = directory.join(entry.as_path());
            root.directory(&shard)?;
            for child in root.entries(&shard)? {
                let child = child?;
                let hash = inventory_hash(root, &shard.join(child.as_path()))?;
                if !hash.starts_with(name) {
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
fn inventory_hash(root: &DiskRoot, path: &Path) -> Result<String> {
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
    root.open_regular(path)?;
    Ok(hash.to_owned())
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/journal/object_storage/tests.rs"]
mod tests;
