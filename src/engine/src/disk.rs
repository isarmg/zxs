use crate::*;
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::{
    AtomicFile, EntryName, PrivateDirectory, RelativePath,
    linux::{MountPolicy, OpenAt2Root, SingleLinkRequirement, open_private_regular},
};
use std::{
    fs::{self, File},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

mod fan_out;
mod mountinfo;
mod owned;
pub(crate) use fan_out::{FanOutTarget, fan_out_admitted, verified_fan_out_copy_at};
pub(crate) use owned::{DiskHandle, DiskResources, DiskRoot, ObjectInstallBatch};

pub(crate) fn fs_error(e: platform_io::fs_safety::Error) -> Error {
    e.into()
}
pub(crate) fn absolute_path(path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => out.push("/"),
            Component::Normal(name) => out.push(name),
            Component::CurDir => {}
            // Resolve no parent traversals before a symlink-safe walk: accepting
            // them would change their meaning if an ancestor were a symlink.
            _ => {
                return Err(Error::new(
                    storage_contract::ErrorCode::UnsafePath,
                    "parent traversal is not allowed in managed I/O",
                ));
            }
        }
    }
    Ok(out)
}

pub(crate) const MAX_METADATA: usize = 64 * 1024 * 1024;
// A 1 TiB file with the smallest supported 64 KiB checksum chunks needs
// 512 MiB plus its fixed checksum header. Keep this separate from JSON metadata.
pub(crate) const MAX_CHECKSUM_MANIFEST: usize = 512 * 1024 * 1024 + CHECKSUM_HEADER_BYTES;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct DiskSeal {
    pub pool_id: String,
    pub disk_id: String,
    pub slot_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub binding_generation: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub device: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub inode: u64,
    pub physical_device: String,
    pub filesystem_uuid: String,
    pub simulation: bool,
}
pub(crate) fn private_dir(path: &Path) -> Result<()> {
    let path = absolute_path(path)?;
    match PrivateDirectory::create(&path) {
        Ok(_) => (),
        Err(platform_io::fs_safety::Error::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            let parent = path
                .parent()
                .expect("invariant: missing child has a parent");
            private_dir(parent)?;
            PrivateDirectory::create(&path).map_err(fs_error)?;
        }
        Err(error) => return Err(fs_error(error)),
    }
    Ok(())
}
pub(crate) fn existing_private_dir(path: &Path) -> Result<PrivateDirectory> {
    PrivateDirectory::open_existing(absolute_path(path)?).map_err(fs_error)
}
pub(crate) fn sync_dir(path: &Path) -> Result<()> {
    OpenAt2Root::open(&absolute_path(path)?, MountPolicy::SameMount)
        .map_err(fs_error)?
        .directory()
        .sync_all()?;
    Ok(())
}
pub(crate) fn open_regular(path: &Path) -> Result<File> {
    open_optional_regular(path)?
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound).into())
}
/// Only an absent leaf below an opened, safe parent means no file. A missing
/// or linked parent and every other I/O failure must remain an error.
pub(crate) fn open_optional_regular(path: &Path) -> Result<Option<File>> {
    let path = absolute_path(path)?;
    let parent = path.parent().ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::UnsafePath,
            "missing file parent",
        )
    })?;
    let root = OpenAt2Root::open(parent, MountPolicy::SameMount).map_err(fs_error)?;
    let name =
        RelativePath::new(path.file_name().ok_or_else(|| {
            Error::new(storage_contract::ErrorCode::UnsafePath, "missing file name")
        })?)
        .map_err(fs_error)?;
    match root.open_file(&name, false) {
        Ok(file) => Ok(Some(file)),
        Err(platform_io::fs_safety::Error::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(None)
        }
        Err(error) => Err(fs_error(error)),
    }
}
pub(crate) fn open_existing_journal(parent: &File, name: &EntryName) -> Result<Option<File>> {
    match open_journal_file(parent, name, rustix::fs::OFlags::empty()) {
        Ok(file) => Ok(Some(file)),
        Err(Error::Filesystem(platform_io::fs_safety::Error::Io(error)))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}
pub(crate) fn create_journal(parent: &File, name: &EntryName) -> Result<File> {
    open_journal_file(
        parent,
        name,
        rustix::fs::OFlags::CREATE | rustix::fs::OFlags::EXCL,
    )
}
fn open_journal_file(parent: &File, name: &EntryName, flags: rustix::fs::OFlags) -> Result<File> {
    let file = File::from(
        rustix::fs::openat2(
            parent,
            name.as_os_str(),
            flags
                | rustix::fs::OFlags::RDWR
                | rustix::fs::OFlags::APPEND
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::NONBLOCK
                | rustix::fs::OFlags::CLOEXEC,
            if flags.contains(rustix::fs::OFlags::CREATE) {
                rustix::fs::Mode::from_raw_mode(0o600)
            } else {
                rustix::fs::Mode::empty()
            },
            rustix::fs::ResolveFlags::BENEATH
                | rustix::fs::ResolveFlags::NO_SYMLINKS
                | rustix::fs::ResolveFlags::NO_XDEV,
        )
        .map_err(|e| fs_error(platform_io::fs_safety::Error::Io(e.into())))?,
    );
    SingleLinkRequirement::check(&file).map_err(fs_error)?;
    Ok(file)
}
pub(crate) fn read_bytes(path: &Path, maximum: usize) -> Result<Vec<u8>> {
    read_file_bytes(open_regular(path)?, maximum)
}
/// Apply the same metadata bound to an already validated, held regular file.
pub(crate) fn read_file_bytes(mut file: File, maximum: usize) -> Result<Vec<u8>> {
    if file.metadata()?.len() > maximum as u64 {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "metadata input exceeds limit",
        ));
    }
    let mut output = Vec::new();
    (&mut file)
        .take(maximum as u64 + 1)
        .read_to_end(&mut output)?;
    if output.len() > maximum {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "metadata grew beyond limit",
        ));
    }
    Ok(output)
}

/// Account actual successful reads in bounded bursts; zero is a cancellation checkpoint.
pub(crate) fn read_observed_fill(
    reader: &mut impl Read,
    output: &mut [u8],
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<usize> {
    read_fill_with_limit(reader, output, 64 * 1024, observe)
}

/// Share the exact EOF/error behavior without imposing scrub pacing on normal readers.
pub(crate) fn read_fill_with_limit(
    reader: &mut impl Read,
    output: &mut [u8],
    maximum_read: usize,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<usize> {
    let mut filled = 0;
    while filled < output.len() {
        observe(0)?;
        let end = filled + (output.len() - filled).min(maximum_read);
        let count = match reader.read(&mut output[filled..end]) {
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        };
        if count == 0 {
            break;
        }
        filled += count;
        observe(count)?;
    }
    Ok(filled)
}

pub(crate) fn read_file_bytes_observed(
    mut file: File,
    maximum: usize,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<Vec<u8>> {
    if file.metadata()?.len() > maximum as u64 {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "metadata input exceeds limit",
        ));
    }
    let mut output = Vec::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let remaining = (maximum + 1 - output.len()).min(buffer.len());
        let count = read_observed_fill(&mut file, &mut buffer[..remaining], observe)?;
        output.extend_from_slice(&buffer[..count]);
        if output.len() > maximum {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "metadata grew beyond limit",
            ));
        }
        if count < remaining {
            break;
        }
    }
    Ok(output)
}

pub(crate) fn read_bytes_observed(
    path: &Path,
    maximum: usize,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<Vec<u8>> {
    observe(0)?;
    read_file_bytes_observed(open_regular(path)?, maximum, observe)
}

pub(crate) fn read_json<T: data_serialization::de::DeserializeOwned>(path: &Path) -> Result<T> {
    decode_strict(&read_bytes(path, MAX_METADATA)?).map_err(Into::into)
}

pub(crate) fn read_json_observed<T: data_serialization::de::DeserializeOwned>(
    path: &Path,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<T> {
    decode_strict(&read_bytes_observed(path, MAX_METADATA, observe)?).map_err(Into::into)
}
pub(crate) fn write_atomic(path: &Path, bytes: &[u8], replace: bool) -> Result<()> {
    let path = absolute_path(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| Error::new(storage_contract::ErrorCode::UnsafePath, "missing parent"))?;
    private_dir(parent)?;
    let directory = PrivateDirectory::open_existing(parent).map_err(fs_error)?;
    let name = EntryName::new(path.file_name().ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::UnsafePath,
            "missing object name",
        )
    })?)
    .map_err(fs_error)?;
    write_atomic_in(&directory, &name, bytes, replace)
}

fn write_atomic_in(
    directory: &PrivateDirectory,
    name: &EntryName,
    bytes: &[u8],
    replace: bool,
) -> Result<()> {
    if !replace {
        match open_private_regular(directory, name) {
            Ok(mut file) => {
                let mut existing = Vec::new();
                (&mut file)
                    .take(bytes.len() as u64 + 1)
                    .read_to_end(&mut existing)
                    .map_err(|error| fs_error(platform_io::fs_safety::Error::Io(error)))?;
                if existing.len() > bytes.len() {
                    return Err(
                        fs_error(platform_io::fs_safety::Error::BudgetExceeded).context(
                            storage_contract::ErrorCode::ObjectCollision,
                            "immutable object exceeds expected byte length",
                        ),
                    );
                }
                if existing != bytes {
                    return Err(Error::new(
                        storage_contract::ErrorCode::ObjectCollision,
                        "immutable object differs from existing bytes",
                    ));
                }
                // A prior create may have installed bytes before its parent sync failed.
                file.sync_all()?;
                return directory.sync().map_err(fs_error);
            }
            Err(platform_io::fs_safety::Error::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(fs_error(error)),
        }
    }
    if replace {
        AtomicFile::replace(directory, &name.as_relative(), bytes).map_err(fs_error)
    } else {
        AtomicFile::create(directory, name, bytes).map_err(fs_error)
    }
}
/// Every JSON object must fit the same bound used by its on-disk reader.
pub(crate) fn encode_metadata<T: Serialize + ?Sized>(value: &T, maximum: usize) -> Result<Vec<u8>> {
    let bytes = canonical_json(value)?;
    if bytes.len() > maximum {
        return Err(Error::new(
            storage_contract::ErrorCode::ResourceLimit,
            format!("serialized metadata exceeds {maximum} byte reader limit"),
        ));
    }
    Ok(bytes)
}
pub(crate) fn write_json<T: Serialize + ?Sized>(
    path: &Path,
    value: &T,
    replace: bool,
) -> Result<()> {
    write_atomic(path, &encode_metadata(value, MAX_METADATA)?, replace)
}
#[cfg(test)]
pub(crate) fn full_copy(
    source: &Path,
    destination: &Path,
    chunk_bytes: u32,
    max_bytes: u64,
) -> Result<(u64, ContentHash, ChecksumManifest)> {
    let source = open_regular(source)?;
    full_copy_to_path(source, destination, chunk_bytes, max_bytes, None)
        .map(|copy| (copy.size, copy.hash, copy.checksums))
        .map_err(CopyFailure::into_error)
}

/// Source commitments verified against this exact version. This is evidence
/// about the input bytes, not evidence that any target file is durable.
#[derive(Debug)]
struct VerifiedSourceSummary<'a> {
    version: &'a FileVersion,
    checksum_bytes: Vec<u8>,
}

/// Owns an independently written and synced output until installation. Only
/// the copy paths combine source evidence with a successfully sealed file.
#[derive(Debug)]
pub(crate) struct VerifiedCopy<'a> {
    file: File,
    summary: Arc<VerifiedSourceSummary<'a>>,
}

impl<'a> VerifiedCopy<'a> {
    pub(crate) fn checksum_bytes(&self) -> &[u8] {
        &self.summary.checksum_bytes
    }

    pub(crate) fn into_parts(self) -> (&'a FileVersion, File) {
        (self.summary.version, self.file)
    }
}

/// The serial reader also serves callers that have no expected version.
/// Retain its existing digest output, and retain the canonical encoding only
/// when it was needed for source validation. Export-only verified callers
/// must not encode a second time merely to discard the resulting copy.
struct FullCopy<'a> {
    size: u64,
    hash: ContentHash,
    checksums: ChecksumManifest,
    file: File,
    summary: Option<Arc<VerifiedSourceSummary<'a>>>,
}

impl<'a> FullCopy<'a> {
    fn into_verified(self) -> VerifiedCopy<'a> {
        VerifiedCopy {
            file: self.file,
            summary: self
                .summary
                .expect("invariant: expected version was verified by the serial reader"),
        }
    }
}

#[derive(Debug)]
pub(crate) enum CopyFailure {
    Source(Error),
    Destination(Error),
}

impl CopyFailure {
    pub(crate) fn into_error(self) -> Error {
        match self {
            Self::Source(error) | Self::Destination(error) => error,
        }
    }
}

impl From<Error> for CopyFailure {
    fn from(error: Error) -> Self {
        Self::Destination(error)
    }
}

impl From<std::io::Error> for CopyFailure {
    fn from(error: std::io::Error) -> Self {
        Self::Destination(error.into())
    }
}

impl From<storage_contract::Error> for CopyFailure {
    fn from(error: storage_contract::Error) -> Self {
        Self::Destination(error.into())
    }
}

pub(crate) fn verified_full_copy<'a>(
    source: File,
    destination: &Path,
    version: &'a FileVersion,
    max_bytes: u64,
) -> std::result::Result<VerifiedCopy<'a>, CopyFailure> {
    full_copy_to_path(
        source,
        destination,
        version.checksum_chunk_bytes,
        max_bytes,
        Some(version),
    )
    .map(FullCopy::into_verified)
}

pub(crate) fn verified_full_copy_at<'a>(
    root: &DiskRoot,
    source: File,
    destination: &Path,
    version: &'a FileVersion,
    max_bytes: u64,
) -> std::result::Result<VerifiedCopy<'a>, CopyFailure> {
    let parent = destination
        .parent()
        .expect("invariant: payload destination has a parent");
    root.private_dir(parent)?;
    let directory = root.directory(parent)?;
    let name = EntryName::new(
        destination
            .file_name()
            .expect("invariant: payload destination has a name"),
    )
    .map_err(fs_error)?;
    full_copy_checked(
        source,
        &directory,
        &name,
        version.checksum_chunk_bytes,
        max_bytes,
        Some(version),
    )
    .map(FullCopy::into_verified)
}

fn full_copy_to_path<'a>(
    source: File,
    destination: &Path,
    chunk_bytes: u32,
    max_bytes: u64,
    expected: Option<&'a FileVersion>,
) -> std::result::Result<FullCopy<'a>, CopyFailure> {
    let destination = absolute_path(destination)?;
    let parent = destination.parent().ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::UnsafePath,
            "missing destination parent",
        )
    })?;
    let directory = match OpenAt2Root::open(parent, MountPolicy::SameMount) {
        Ok(directory) => directory,
        Err(platform_io::fs_safety::Error::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            private_dir(parent)?;
            OpenAt2Root::open(parent, MountPolicy::SameMount).map_err(fs_error)?
        }
        Err(error) => return Err(fs_error(error).into()),
    };
    let destination_name = EntryName::new(destination.file_name().ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::UnsafePath,
            "missing destination name",
        )
    })?)
    .map_err(fs_error)?;
    full_copy_checked(
        source,
        directory.directory(),
        &destination_name,
        chunk_bytes,
        max_bytes,
        expected,
    )
}

pub(crate) fn full_copy_held(
    source: File,
    directory: &OpenAt2Root,
    destination_name: &EntryName,
    chunk_bytes: u32,
    max_bytes: u64,
) -> Result<(u64, ContentHash, ChecksumManifest, File)> {
    full_copy_checked(
        source,
        directory.directory(),
        destination_name,
        chunk_bytes,
        max_bytes,
        None,
    )
    .map(|copy| (copy.size, copy.hash, copy.checksums, copy.file))
    .map_err(CopyFailure::into_error)
}

fn full_copy_checked<'a>(
    source: File,
    directory: &File,
    destination_name: &EntryName,
    chunk_bytes: u32,
    max_bytes: u64,
    expected: Option<&'a FileVersion>,
) -> std::result::Result<FullCopy<'a>, CopyFailure> {
    let temporary = format!(".zxs-full-{}", new_id("req"));
    let mut target = File::from(
        rustix::fs::openat(
            directory,
            temporary.as_str(),
            rustix::fs::OFlags::CREATE
                | rustix::fs::OFlags::EXCL
                | rustix::fs::OFlags::WRONLY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::from_raw_mode(0o600),
        )
        .map_err(std::io::Error::from)?,
    );
    let result = (|| {
        let mut full = blake3::Hasher::new();
        let mut chunks = Vec::new();
        let mut size = 0u64;
        let mut buf = vec![0u8; chunk_bytes as usize];
        loop {
            let mut used = 0;
            while used < buf.len() {
                let n = std::os::unix::fs::FileExt::read_at(
                    &source,
                    &mut buf[used..],
                    size + used as u64,
                )
                .map_err(|error| CopyFailure::Source(error.into()))?;
                if n == 0 {
                    break;
                }
                used += n;
            }
            if used == 0 {
                break;
            }
            size = size.checked_add(used as u64).ok_or_else(|| {
                CopyFailure::Source(Error::new(
                    storage_contract::ErrorCode::FileTooLarge,
                    "file length overflow",
                ))
            })?;
            if size > max_bytes {
                return Err(CopyFailure::Source(Error::new(
                    storage_contract::ErrorCode::FileTooLarge,
                    "file exceeds configured maximum",
                )));
            }
            target.write_all(&buf[..used])?;
            full.update(&buf[..used]);
            chunks.push(*blake3::hash(&buf[..used]).as_bytes());
        }
        let hash = ContentHash::from_digest(full.finalize());
        let checksums = ChecksumManifest {
            chunk_bytes,
            logical_size: size,
            chunks,
        };
        // Bind the output to all published commitments before exposing its name.
        let summary = if let Some(expected) = expected {
            let changed = || {
                CopyFailure::Source(Error::new(
                    storage_contract::ErrorCode::SourceChanged,
                    "copied content differs from the published manifest",
                ))
            };
            if size != expected.logical_size || hash != expected.content_hash {
                return Err(changed());
            }
            let checksum_bytes = checksums.encode()?;
            if blake3::hash(&checksum_bytes).to_hex().as_str() != expected.checksum_manifest_hash {
                return Err(changed());
            }
            Some(Arc::new(VerifiedSourceSummary {
                version: expected,
                checksum_bytes,
            }))
        } else {
            None
        };
        target.set_permissions(fs::Permissions::from_mode(0o400))?;
        // Persist the complete bytes and final mode together before publishing
        // the name. The rename still needs its own parent-directory barrier.
        target.sync_all()?;
        rustix::fs::renameat_with(
            directory,
            temporary.as_str(),
            directory,
            destination_name.as_os_str(),
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(std::io::Error::from)?;
        directory.sync_all()?;
        Ok(FullCopy {
            size,
            hash,
            checksums,
            file: target,
            summary,
        })
    })();
    match result {
        Ok(copy) => Ok(copy),
        Err(operation) => {
            match rustix::fs::unlinkat(directory, temporary.as_str(), rustix::fs::AtFlags::empty())
            {
                Ok(()) | Err(rustix::io::Errno::NOENT) => Err(operation),
                Err(cleanup) => Err(CopyFailure::Destination(Error::Cleanup {
                    operation: Box::new(operation.into_error()),
                    cleanup: cleanup.into(),
                })),
            }
        }
    }
}
pub(crate) fn verify_payload(directory: &Path, version: &FileVersion) -> Result<()> {
    verify_payload_held(directory, version).map(|_| ())
}

pub(crate) fn open_payload(
    directory: &Path,
    version: &FileVersion,
) -> Result<(File, ChecksumManifest)> {
    let directory = absolute_path(directory)?;
    let history = directory.parent().and_then(Path::parent).ok_or_else(|| {
        Error::new(
            ErrorCode::UnsafePath,
            "version metadata has no history root",
        )
    })?;
    let root = DiskRoot::open(history)?;
    let (file, _, manifest) = open_payload_at(&root, &directory, version)?;
    Ok((file, manifest))
}

pub(crate) fn open_payload_at(
    root: &DiskRoot,
    directory: &Path,
    version: &FileVersion,
) -> Result<(File, Vec<u8>, ChecksumManifest)> {
    let checks = root.read_bytes(
        &directory.join("checksums.bin"),
        checksum_manifest_size(version)?,
    )?;
    let manifest = decode_checksum_manifest(&checks, version)?;
    let path =
        crate::container_storage::content_path_at(root, directory, version, &mut |_| Ok(()))?;
    let file = root.open_regular(&path)?;
    if file.metadata()?.len() != version.logical_size {
        return Err(Error::new(
            ErrorCode::ChecksumMismatch,
            "payload length differs",
        ));
    }
    Ok((file, checks, manifest))
}

pub(crate) fn read_checksum_manifest_at(
    root: &DiskRoot,
    directory: &Path,
    version: &FileVersion,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<ChecksumManifest> {
    let checks = root.read_bytes_observed(
        &directory.join("checksums.bin"),
        checksum_manifest_size(version)?,
        observe,
    )?;
    decode_checksum_manifest(&checks, version)
}

/// Bound a persisted checksum read by the version's canonical encoding, not
/// the format-wide maximum. Corrupt tiny replicas must not consume a large
/// file's budget concurrently before their digest is rejected.
pub(crate) fn checksum_manifest_size(version: &FileVersion) -> Result<usize> {
    if !SUPPORTED_CHECKSUM_CHUNK_BYTES.contains(&version.checksum_chunk_bytes) {
        return Err(Error::new(
            ErrorCode::ChecksumFormatInvalid,
            "unsupported checksum chunk size",
        ));
    }
    version
        .logical_size
        .div_ceil(u64::from(version.checksum_chunk_bytes))
        .checked_mul(32)
        .and_then(|bytes| bytes.checked_add(CHECKSUM_HEADER_BYTES as u64))
        .filter(|bytes| *bytes <= MAX_CHECKSUM_MANIFEST as u64)
        .map(|bytes| bytes as usize)
        .ok_or_else(|| {
            Error::new(
                ErrorCode::ResourceLimit,
                "version checksum manifest exceeds the supported format limit",
            )
        })
}

fn decode_checksum_manifest(checks: &[u8], version: &FileVersion) -> Result<ChecksumManifest> {
    if blake3::hash(checks).to_hex().as_str() != version.checksum_manifest_hash {
        return Err(Error::new(
            storage_contract::ErrorCode::ChecksumMismatch,
            "checksum manifest digest differs",
        ));
    }
    let manifest = ChecksumManifest::decode(checks)?;
    if manifest.logical_size != version.logical_size
        || manifest.chunk_bytes != version.checksum_chunk_bytes
    {
        return Err(Error::new(
            storage_contract::ErrorCode::ChecksumMismatch,
            "checksum header differs from version",
        ));
    }
    Ok(manifest)
}

pub(crate) fn open_payload_file_observed_at(
    root: &DiskRoot,
    directory: &Path,
    version: &FileVersion,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<File> {
    let file = root.open_regular(&crate::container_storage::content_path_at(
        root, directory, version, observe,
    )?)?;
    if file.metadata()?.len() != version.logical_size {
        return Err(Error::new(
            ErrorCode::ChecksumMismatch,
            "payload length differs",
        ));
    }
    Ok(file)
}

pub(crate) fn verify_payload_held(directory: &Path, version: &FileVersion) -> Result<File> {
    let (file, manifest) = open_payload(directory, version)?;
    verify_open_payload(file, manifest, version, usize::MAX, &mut |_| Ok(()))
}

/// Checkpoints never stand in for byte verification; scrub uses them to stop
/// between bounded chunks while ordinary readers retain the same full proof.
#[cfg(test)]
pub(crate) fn verify_payload_held_with(
    directory: &Path,
    version: &FileVersion,
    mut checkpoint: impl FnMut() -> Result<()>,
) -> Result<File> {
    checkpoint()?;
    let (file, manifest) = open_payload(directory, version)?;
    verify_open_payload_observed(file, manifest, version, &mut |_| checkpoint())
}

pub(crate) fn verify_open_payload_observed(
    file: File,
    manifest: ChecksumManifest,
    version: &FileVersion,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<File> {
    verify_open_payload(file, manifest, version, 64 * 1024, observe)
}

fn verify_open_payload(
    mut file: File,
    manifest: ChecksumManifest,
    version: &FileVersion,
    maximum_read: usize,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<File> {
    let mut full = blake3::Hasher::new();
    let mut buffer = vec![0u8; manifest.chunk_bytes as usize];
    let mut remaining = manifest.logical_size;
    for expected in manifest.chunks {
        let size = remaining.min(buffer.len() as u64) as usize;
        if read_fill_with_limit(&mut file, &mut buffer[..size], maximum_read, observe)? != size {
            return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into());
        }
        if blake3::hash(&buffer[..size]).as_bytes() != &expected {
            return Err(Error::new(
                ErrorCode::ChecksumMismatch,
                "payload chunk differs",
            ));
        }
        full.update(&buffer[..size]);
        remaining -= size as u64;
    }
    if read_fill_with_limit(&mut file, &mut [0u8; 1], maximum_read, observe)? != 0
        || file.metadata()?.len() != version.logical_size
    {
        return Err(Error::new(
            ErrorCode::ChecksumMismatch,
            "payload length changed during verification",
        ));
    }
    if ContentHash::from_digest(full.finalize()) != version.content_hash {
        return Err(Error::new(
            ErrorCode::ChecksumMismatch,
            "complete payload hash differs",
        ));
    }
    observe(0)?;
    Ok(file)
}
/// Byte verification held only until this preparation installs/syncs that inode.
/// The separate decision proof still reads the installed bytes before ACCEPT.
pub(crate) struct VerifiedExisting<'a> {
    version: &'a FileVersion,
    directory: PathBuf,
    path: PathBuf,
    file: File,
}
impl<'a> VerifiedExisting<'a> {
    #[cfg(test)]
    pub(crate) fn read(directory: &Path, version: &'a FileVersion) -> Result<Self> {
        let directory = absolute_path(directory)?;
        let history = directory.parent().and_then(Path::parent).ok_or_else(|| {
            Error::new(
                ErrorCode::UnsafePath,
                "version metadata has no history root",
            )
        })?;
        Self::read_at(&DiskRoot::open(history)?, &directory, version)
    }

    pub(crate) fn read_at(
        root: &DiskRoot,
        directory: &Path,
        version: &'a FileVersion,
    ) -> Result<Self> {
        let path =
            crate::container_storage::content_path_at(root, directory, version, &mut |_| Ok(()))?;
        let (file, _, manifest) = open_payload_at(root, directory, version)?;
        let file = verify_open_payload(file, manifest, version, usize::MAX, &mut |_| Ok(()))?;
        Ok(Self {
            version,
            directory: directory.to_path_buf(),
            path,
            file,
        })
    }
    pub(crate) fn version(&self) -> &'a FileVersion {
        self.version
    }
    pub(crate) fn assert_binding(&self, directory: &Path, version: &FileVersion) {
        assert!(
            std::ptr::eq(self.version, version) && self.directory == directory,
            "invariant: existing proof belongs to its admitted version and directory"
        );
    }
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
    pub(crate) fn sync_at(&self, root: &DiskRoot, directory: &Path, path: &Path) -> Result<()> {
        use platform_io::fs_safety::linux::FileIdentity;
        assert_eq!(
            directory, self.directory,
            "invariant: install consumes its admitted directory"
        );
        if path != self.path
            || FileIdentity::from_fd(root.open_regular(path)?)?
                != FileIdentity::from_fd(&self.file)?
        {
            return Err(Error::new(
                ErrorCode::PreparationStale,
                "existing payload identity changed before synchronization",
            ));
        }
        self.file.sync_all()?;
        Ok(())
    }
}

pub(crate) fn version_dir(disk: &DiskConfig, history: &str, version: &str) -> PathBuf {
    disk.root
        .join("histories")
        .join(history)
        .join("versions")
        .join(version)
}

pub(crate) fn identity(config: &ValidatedConfig, disk: &DiskConfig) -> Result<DiskSeal> {
    let m = fs::symlink_metadata(&disk.root)?;
    if !m.is_dir() || m.file_type().is_symlink() {
        return Err(Error::new(
            storage_contract::ErrorCode::DiskIdentityMismatch,
            "disk root is not a physical directory",
        ));
    }
    seal_for_root(config, disk, &m, || production_device(&disk.root))
}

fn identity_at(config: &ValidatedConfig, disk: &DiskConfig, root: &File) -> Result<DiskSeal> {
    seal_for_root(config, disk, &root.metadata()?, || {
        production_device_at(root)
    })
}

fn seal_for_root(
    config: &ValidatedConfig,
    disk: &DiskConfig,
    metadata: &fs::Metadata,
    production: impl FnOnce() -> Result<(String, String)>,
) -> Result<DiskSeal> {
    let (physical_device, filesystem_uuid) = if config.mode == Mode::Development {
        (
            format!("development:{}", disk.slot_id),
            format!("development:{}", disk.disk_id),
        )
    } else {
        production()?
    };
    Ok(DiskSeal {
        pool_id: config.pool_id.clone(),
        disk_id: disk.disk_id.clone(),
        slot_id: disk.slot_id.clone(),
        binding_generation: disk.binding_generation,
        device: metadata.dev(),
        inode: metadata.ino(),
        physical_device,
        filesystem_uuid,
        simulation: config.mode == Mode::Development,
    })
}
fn production_device(root: &Path) -> Result<(String, String)> {
    let canonical = root.canonicalize()?;
    let mountinfo = fs::read_to_string("/proc/self/mountinfo")?;
    let selected = mountinfo::resolve(&mountinfo, &canonical)?;
    production_device_on_mount(selected)
}

fn mount_identity(root: &File) -> Result<u64> {
    let identity = rustix::fs::statx(
        root,
        "",
        rustix::fs::AtFlags::EMPTY_PATH,
        rustix::fs::StatxFlags::MNT_ID,
    )
    .map_err(std::io::Error::from)?;
    if identity.stx_mask & rustix::fs::StatxFlags::MNT_ID.bits() == 0 {
        return Err(Error::new(
            storage_contract::ErrorCode::DiskIdentityMismatch,
            "kernel did not report the held disk root mount identity",
        ));
    }
    Ok(identity.stx_mnt_id)
}

fn production_device_at(root: &File) -> Result<(String, String)> {
    let mount_id = mount_identity(root)?;
    let mountinfo = fs::read_to_string("/proc/self/mountinfo")?;
    production_device_on_mount(mountinfo::resolve_id(&mountinfo, mount_id)?)
}

fn production_device_on_mount(selected: mountinfo::Mount) -> Result<(String, String)> {
    if selected.kind != "btrfs" {
        return Err(Error::new(
            storage_contract::ErrorCode::FilesystemRequired,
            "production requires an independent Btrfs filesystem for every disk",
        ));
    }
    let device = selected.source.canonicalize()?;
    let name = device.file_name().ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::DiskIdentityMismatch,
            "missing block identity",
        )
    })?;
    let class = Path::new("/sys/class/block").join(name);
    let hardware = class.canonicalize()?;
    let mut physical = if class.join("partition").try_exists()? {
        hardware
            .parent()
            .ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::DiskIdentityMismatch,
                    "partition parent missing",
                )
            })?
            .to_path_buf()
    } else {
        hardware.clone()
    };
    // Device-mapper, loop, md and other virtual block devices cannot establish
    // independent physical media from this kernel view. Refuse them instead of
    // counting two mappings of one drive as independent replicas.
    if physical.starts_with("/sys/devices/virtual")
        || fs::read_dir(class.join("slaves"))?
            .next()
            .transpose()?
            .is_some()
    {
        return Err(Error::new(
            storage_contract::ErrorCode::UnsupportedDeviceTopology,
            "production requires directly identifiable independent drives; virtual or aggregated block devices are not supported",
        ));
    }
    // Multiple namespaces of one NVMe controller still share a physical drive.
    if let Some(name) = physical.file_name().and_then(|n| n.to_str())
        && name.starts_with("nvme")
        && name.contains('n')
    {
        let suffix = &name[4..];
        if suffix
            .split_once('n')
            .is_some_and(|(controller, namespace)| {
                !controller.is_empty()
                    && !namespace.is_empty()
                    && controller.bytes().all(|b| b.is_ascii_digit())
                    && namespace.bytes().all(|b| b.is_ascii_digit())
            })
        {
            physical = physical
                .parent()
                .ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::DiskIdentityMismatch,
                        "NVMe controller identity is missing",
                    )
                })?
                .to_path_buf();
        }
    }
    let mut fsid = None;
    for entry in fs::read_dir("/sys/fs/btrfs")? {
        let entry = entry?;
        let devices = entry.path().join("devices");
        if !devices.is_dir() {
            continue;
        }
        let entries = fs::read_dir(&devices)?.collect::<std::io::Result<Vec<_>>>()?;
        let mut belongs = false;
        for device in &entries {
            if device.path().canonicalize()? == hardware {
                belongs = true;
                break;
            }
        }
        if belongs {
            if entries.len() != 1 {
                return Err(Error::new(
                    storage_contract::ErrorCode::MultiDeviceFilesystem,
                    "Btrfs must contain exactly one physical device",
                ));
            }
            fsid = Some(entry.file_name().to_string_lossy().into_owned());
            break;
        }
    }
    Ok((
        physical.to_string_lossy().into_owned(),
        fsid.ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::DiskIdentityMismatch,
                "cannot verify Btrfs device count and UUID",
            )
        })?,
    ))
}
/// An unmounted root is unavailable only when absent or a safe, empty directory.
/// A missing seal in a nonempty root is corruption, not permission to initialize.
pub(crate) fn disk_unavailable(disk: &DiskConfig) -> Result<bool> {
    match fs::symlink_metadata(&disk.root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error.into()),
        Ok(_) => (),
    }
    existing_private_dir(&disk.root)?;
    match fs::read_dir(&disk.root)?.next() {
        None => Ok(true),
        Some(Ok(_)) => Ok(false),
        Some(Err(error)) => Err(error.into()),
    }
}

pub(crate) fn validate_seal(config: &ValidatedConfig, disk: &DiskConfig) -> Result<DiskSeal> {
    validate_seal_observed(config, disk, &mut |_| Ok(()))
}

pub(crate) fn validate_seal_observed(
    config: &ValidatedConfig,
    disk: &DiskConfig,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<DiskSeal> {
    let stored: DiskSeal = read_json_observed(&disk.root.join("disk.json"), observe)?;
    let live = identity(config, disk)?;
    validate_stored_seal(&stored, &live)?;
    Ok(live)
}

pub(crate) fn validate_stored_seal(stored: &DiskSeal, live: &DiskSeal) -> Result<()> {
    if stored.pool_id != live.pool_id
        || stored.disk_id != live.disk_id
        || stored.slot_id != live.slot_id
        || stored.binding_generation != live.binding_generation
        || stored.inode != live.inode
        || stored.physical_device != live.physical_device
        || stored.filesystem_uuid != live.filesystem_uuid
        || stored.simulation != live.simulation
    {
        return Err(Error::new(
            storage_contract::ErrorCode::DiskIdentityMismatch,
            format!("disk {} identity changed", live.slot_id),
        ));
    }
    Ok(())
}

pub fn offline_check(root: &Path) -> Result<CheckReport> {
    offline_check_in_context(root, None)
}

pub(crate) fn offline_check_in_context(
    root: &Path,
    metadata: Option<&crate::engine::provenance::MetadataReadContext<'_>>,
) -> Result<CheckReport> {
    let loaded = read_json(&root.join("disk.json"));
    let _seal: DiskSeal = match metadata {
        Some(metadata) => metadata.seal(loaded)?,
        None => loaded?,
    };
    let mut report = CheckReport {
        checked_versions: 0,
        healthy_replicas: 0,
        tombstoned_versions: 0,
        issues: Vec::new(),
    };
    let histories = root.join("histories");
    let histories = match fs::read_dir(&histories) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(report),
        Err(error) => return Err(error.into()),
    };
    for h in histories {
        let h = h?;
        if !h.file_type()?.is_dir() {
            continue;
        }
        let versions = h.path().join("versions");
        let versions = match fs::read_dir(&versions) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        for v in versions {
            let v = v?;
            if !v.file_type()?.is_dir() {
                continue;
            }
            report.checked_versions += 1;
            let check: Result<bool> = (|| {
                let directory = v.path();
                let version: FileVersion = read_json(&directory.join("version.json"))?;
                if let Some(file) = open_optional_regular(&directory.join("purged.json"))? {
                    let tombstone = decode_strict(&read_file_bytes(file, MAX_METADATA)?)?;
                    crate::journal::verify_tombstone_content_in_context(
                        &directory, &version, &tombstone, metadata,
                    )?;
                    return Ok(true);
                }
                validate_id(&version.version_id, "ver")?;
                name_policy::validate_name(&version.creation_name)?;
                crate::journal::verify_local_publication_in_context(
                    &directory, &version, metadata,
                )?;
                verify_payload(&directory, &version)?;
                Ok(false)
            })();
            match check {
                Ok(true) => report.tombstoned_versions += 1,
                Ok(false) => report.healthy_replicas += 1,
                Err(e) => report
                    .issues
                    .push(format!("{}: {}", v.file_name().to_string_lossy(), e)),
            }
        }
    }
    Ok(report)
}
pub fn offline_export(root: &Path, version_id: &str, destination: &Path) -> Result<()> {
    validate_id(version_id, "ver")?;
    let _seal: DiskSeal = read_json(&root.join("disk.json"))?;
    for h in fs::read_dir(root.join("histories"))? {
        let h = h?;
        if !h.file_type()?.is_dir() {
            continue;
        }
        let directory = h.path().join("versions").join(version_id);
        match OpenAt2Root::open(&directory, MountPolicy::SameMount) {
            Ok(_) => (),
            Err(platform_io::fs_safety::Error::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound =>
            {
                continue;
            }
            Err(error) => return Err(fs_error(error)),
        }
        let version: FileVersion = read_json(&directory.join("version.json"))?;
        name_policy::validate_name(&version.creation_name)?;
        crate::journal::verify_local_publication(&directory, &version)?;
        let source = verify_payload_held(&directory, &version)?;
        return verified_full_copy(source, destination, &version, version.logical_size)
            .map(|_| ())
            .map_err(CopyFailure::into_error)
            .map_err(|error| match error {
                Error::Contract(storage_contract::Error::Io { ref source, .. })
                    if source.kind() == std::io::ErrorKind::AlreadyExists =>
                {
                    error.context(
                        storage_contract::ErrorCode::DestinationExists,
                        "export does not overwrite existing files",
                    )
                }
                error => error,
            });
    }
    Err(Error::new(
        storage_contract::ErrorCode::VersionNotFound,
        "version is not present on this disk",
    ))
}

#[cfg(test)]
#[path = "../../../test/engine/unit/disk/export_tests.rs"]
mod export_tests;

#[cfg(test)]
#[path = "../../../test/engine/unit/disk/admission_tests.rs"]
mod admission_tests;
