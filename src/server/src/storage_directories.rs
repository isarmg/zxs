//! Descriptor-bound, read-only discovery of existing storage directories.
//! Callers provide current authorization and server-owned excluded roots. No
//! method creates directories, writes test files, changes mode, or mounts disks.
use platform_io::fs_safety::{
    self, RelativePath,
    linux::{MountPolicy, OpenAt2Root},
};
use rustix::fs::{self as unix, Access, AtFlags, FileType, StatxFlags};
use std::{
    fs::File,
    io::{self, Read},
    path::{Component, Path, PathBuf},
};
use storage_contract::storage_directories::{
    CandidateSelection, DirectoryEntry, DirectoryIdentity, DirectoryListing, DirectoryProblem,
    InspectRequest, MountIdentity,
};

const MAX_MOUNTINFO: u64 = 4 * 1024 * 1024;
const MAX_MOUNTS: usize = 4096;
const MAX_ENTRIES: usize = 200;
const MAX_SCANNED: usize = 4096;

#[derive(Debug, error_definition::Error)]
pub enum Error {
    #[error("directory inspection I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("directory boundary failed: {0}")]
    Boundary(#[from] fs_safety::Error),
    #[error("expected an absolute, normalized UTF-8 directory path")]
    InvalidPath,
    #[error(
        "directory must be inside the configured discovery root; the root itself is not a storage candidate"
    )]
    OutsideDiscoveryRoot,
    #[error("mount inventory is invalid or exceeds its bound")]
    InvalidMounts,
    #[error("mounted directory identity changed; inspect it again")]
    Changed,
    #[error("directory overlaps a protected or already selected root: {0}")]
    Overlap(PathBuf),
    #[error("storage directory is not empty")]
    NotEmpty,
    #[error("this mounted filesystem is not a persistent storage filesystem: {0}")]
    UnsupportedFilesystem(String),
}

impl From<rustix::io::Errno> for Error {
    fn from(error: rustix::io::Errno) -> Self {
        Self::Io(io::Error::from(error))
    }
}

impl From<Error> for crate::ApiError {
    fn from(source: Error) -> Self {
        use storage_contract::ErrorCode;
        let (status, code) = match &source {
            Error::Changed => (http::StatusCode::CONFLICT, ErrorCode::DiskChanged),
            Error::NotEmpty => (http::StatusCode::CONFLICT, ErrorCode::DirectoryNotEmpty),
            Error::Overlap(_) => (http::StatusCode::CONFLICT, ErrorCode::ConfigConflict),
            Error::Boundary(fs_safety::Error::UnsafePermissions(_)) => {
                (http::StatusCode::FORBIDDEN, ErrorCode::PermissionDenied)
            }
            Error::Io(error) | Error::Boundary(fs_safety::Error::Io(error))
                if matches!(
                    error.kind(),
                    io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem
                ) =>
            {
                (http::StatusCode::FORBIDDEN, ErrorCode::PermissionDenied)
            }
            Error::Io(error) | Error::Boundary(fs_safety::Error::Io(error))
                if error.kind() == io::ErrorKind::NotFound =>
            {
                (http::StatusCode::NOT_FOUND, ErrorCode::NotFound)
            }
            Error::Io(_) | Error::InvalidMounts => (
                http::StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::DiskUnavailable,
            ),
            Error::Boundary(_) | Error::InvalidPath | Error::OutsideDiscoveryRoot => {
                (http::StatusCode::BAD_REQUEST, ErrorCode::UnsafePath)
            }
            Error::UnsupportedFilesystem(_) => {
                (http::StatusCode::BAD_REQUEST, ErrorCode::ConfigInvalid)
            }
        };
        Self::new(status, code, source.to_string()).with_source(source)
    }
}

type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Mount {
    id: u64,
    major: u32,
    minor: u32,
    root: PathBuf,
    path: PathBuf,
    filesystem: String,
}

fn absolute(value: &str) -> Result<PathBuf> {
    let path = Path::new(value);
    if value.len() > 4096
        || value.as_bytes().contains(&0)
        || !path.is_absolute()
        || value.split('/').any(|part| matches!(part, "." | ".."))
        || path
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
    {
        return Err(Error::InvalidPath);
    }
    let normalized: PathBuf = path.components().collect();
    if normalized.as_os_str() != path.as_os_str() {
        return Err(Error::InvalidPath);
    }
    Ok(normalized)
}

fn unescape(value: &str) -> Result<String> {
    let mut output = Vec::with_capacity(value.len());
    let mut bytes = value.as_bytes();
    while let Some((&head, rest)) = bytes.split_first() {
        if head != b'\\' {
            output.push(head);
            bytes = rest;
            continue;
        }
        let Some((escape, rest)) = bytes.split_at_checked(4) else {
            return Err(Error::InvalidMounts);
        };
        output.push(match escape {
            b"\\040" => b' ',
            b"\\011" => b'\t',
            b"\\012" => b'\n',
            b"\\134" => b'\\',
            _ => return Err(Error::InvalidMounts),
        });
        bytes = rest;
    }
    String::from_utf8(output).map_err(|_| Error::InvalidMounts)
}

fn parse_mounts(text: &str) -> Result<Vec<Mount>> {
    let mut result = Vec::new();
    let mut ids = std::collections::BTreeSet::new();
    for line in text.lines() {
        if result.len() == MAX_MOUNTS {
            return Err(Error::InvalidMounts);
        }
        let fields: Vec<_> = line.split(' ').collect();
        let split = fields
            .iter()
            .position(|field| *field == "-")
            .ok_or(Error::InvalidMounts)?;
        if split < 6 || fields.len() != split + 4 {
            return Err(Error::InvalidMounts);
        }
        let (major, minor) = fields[2].split_once(':').ok_or(Error::InvalidMounts)?;
        let mount = Mount {
            id: fields[0].parse().map_err(|_| Error::InvalidMounts)?,
            major: major.parse().map_err(|_| Error::InvalidMounts)?,
            minor: minor.parse().map_err(|_| Error::InvalidMounts)?,
            root: absolute(&unescape(fields[3])?).map_err(|_| Error::InvalidMounts)?,
            path: absolute(&unescape(fields[4])?).map_err(|_| Error::InvalidMounts)?,
            filesystem: fields[split + 1].into(),
        };
        if !ids.insert(mount.id) {
            return Err(Error::InvalidMounts);
        }
        result.push(mount);
    }
    if result.is_empty() {
        return Err(Error::InvalidMounts);
    }
    Ok(result)
}

fn mounts() -> Result<Vec<Mount>> {
    let mut text = String::new();
    File::open("/proc/self/mountinfo")?
        .take(MAX_MOUNTINFO + 1)
        .read_to_string(&mut text)?;
    if text.len() as u64 > MAX_MOUNTINFO {
        return Err(Error::InvalidMounts);
    }
    parse_mounts(&text)
}

fn identity(file: &File) -> Result<(DirectoryIdentity, u64, u32, u32)> {
    let stat = unix::statx(
        file,
        "",
        AtFlags::EMPTY_PATH | AtFlags::SYMLINK_NOFOLLOW,
        StatxFlags::BASIC_STATS | StatxFlags::MNT_ID,
    )?;
    if stat.stx_mask & (StatxFlags::INO | StatxFlags::CTIME | StatxFlags::MNT_ID).bits()
        != (StatxFlags::INO | StatxFlags::CTIME | StatxFlags::MNT_ID).bits()
    {
        return Err(Error::InvalidMounts);
    }
    let raw = unix::fstat(file)?;
    Ok((
        DirectoryIdentity {
            device: raw.st_dev,
            inode: stat.stx_ino,
            ctime_seconds: stat.stx_ctime.tv_sec.to_string(),
            ctime_nanoseconds: stat.stx_ctime.tv_nsec,
        },
        stat.stx_mnt_id,
        stat.stx_dev_major,
        stat.stx_dev_minor,
    ))
}

struct Observed {
    path: PathBuf,
    storage: OpenAt2Root,
    mount_root: OpenAt2Root,
    mount: Mount,
    root_identity: DirectoryIdentity,
    mount_identity: MountIdentity,
}

fn observe(path: &Path, inventory: &[Mount]) -> Result<Observed> {
    // Absolute open traverses all components without following any symlink.
    let opened = OpenAt2Root::open(path, MountPolicy::AllowNestedMounts)?;
    observe_opened(path, opened, inventory)
}

fn observe_opened(path: &Path, opened: OpenAt2Root, inventory: &[Mount]) -> Result<Observed> {
    let (root_identity, id, major, minor) = identity(opened.directory())?;
    let mount = inventory
        .iter()
        .find(|mount| mount.id == id)
        .ok_or(Error::Changed)?
        .clone();
    if mount.major != major || mount.minor != minor {
        return Err(Error::Changed);
    }
    let relative = path.strip_prefix(&mount.path).map_err(|_| Error::Changed)?;
    let mount_root = OpenAt2Root::open(&mount.path, MountPolicy::SameMount)?;
    let (mount_directory, mount_id, mount_major, mount_minor) = identity(mount_root.directory())?;
    if (mount_id, mount_major, mount_minor) != (id, major, minor) {
        return Err(Error::Changed);
    }
    // Resolve the actual root again, anchored to the held mount descriptor.
    let storage = if relative.as_os_str().is_empty() {
        OpenAt2Root::from_directory(mount_root.directory().try_clone()?, MountPolicy::SameMount)?
    } else {
        OpenAt2Root::from_directory(
            mount_root.open_directory(&RelativePath::new(relative)?)?,
            MountPolicy::SameMount,
        )?
    };
    if identity(storage.directory())?.0 != root_identity {
        return Err(Error::Changed);
    }
    let mount_identity = MountIdentity {
        mount_id: id,
        device_major: major,
        device_minor: minor,
        root_inode: mount_directory.inode,
        filesystem: mount.filesystem.clone(),
        filesystem_root: mount
            .root
            .to_str()
            .expect("validated UTF-8 mount path")
            .into(),
    };
    Ok(Observed {
        path: path.to_owned(),
        storage,
        mount_root,
        mount,
        root_identity,
        mount_identity,
    })
}

fn verify_current(observed: &Observed) -> Result<()> {
    let fresh = observe(&observed.path, &mounts()?)?;
    if observed.root_identity != fresh.root_identity
        || observed.mount_identity != fresh.mount_identity
        || observed.mount != fresh.mount
    {
        return Err(Error::Changed);
    }
    Ok(())
}

fn overlaps(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}
fn mount_relative_location(mount: &Mount, path: &Path) -> PathBuf {
    mount.root.join(
        path.strip_prefix(&mount.path)
            .expect("observed mount contains path"),
    )
}
fn filesystem_path(observed: &Observed) -> PathBuf {
    mount_relative_location(&observed.mount, &observed.path)
}
fn exclusion_location(path: &Path, inventory: &[Mount]) -> Result<(Observed, PathBuf)> {
    let path = absolute(path.to_str().ok_or(Error::InvalidPath)?)?;
    let mut existing = path.as_path();
    loop {
        match observe(existing, inventory) {
            Ok(observed) => {
                let location = filesystem_path(&observed).join(
                    path.strip_prefix(existing)
                        .expect("ancestor contains excluded path"),
                );
                return Ok((observed, location));
            }
            Err(Error::Io(error)) | Err(Error::Boundary(fs_safety::Error::Io(error)))
                if error.kind() == io::ErrorKind::NotFound =>
            {
                existing = existing.parent().ok_or(Error::InvalidPath)?;
            }
            Err(error) => return Err(error),
        }
    }
}
fn reject_overlaps(observed: &Observed, excluded: &[PathBuf], inventory: &[Mount]) -> Result<()> {
    for path in excluded {
        if overlaps(&observed.path, path) {
            return Err(Error::Overlap(path.clone()));
        }
        // Resolve the closest existing ancestor even for a planned system
        // directory. This also rejects aliases of not-yet-created exclusions.
        let (other, location) = exclusion_location(path, inventory)?;
        if (observed.mount.major, observed.mount.minor) == (other.mount.major, other.mount.minor)
            && overlaps(&filesystem_path(observed), &location)
        {
            return Err(Error::Overlap(path.clone()));
        }
        verify_current(&other)?;
    }
    Ok(())
}

fn reject_non_storage_filesystem(filesystem: &str) -> Result<()> {
    // Non-storage kernel/memory control filesystems remain browsable, but are
    // not offered as a disk. Unknown filesystems are not declared healthy.
    if matches!(
        filesystem,
        "proc"
            | "sysfs"
            | "devtmpfs"
            | "devpts"
            | "tmpfs"
            | "ramfs"
            | "cgroup"
            | "cgroup2"
            | "securityfs"
            | "debugfs"
            | "tracefs"
            | "pstore"
            | "configfs"
            | "fusectl"
            | "mqueue"
            | "hugetlbfs"
            | "bpf"
            | "autofs"
            | "rpc_pipefs"
    ) {
        return Err(Error::UnsupportedFilesystem(filesystem.into()));
    }
    Ok(())
}

/// Bounded, read-only navigation. Files are not returned or opened. Symlink
/// entries are reported as unavailable, never traversed. Nested mounts can be
/// entered by a new browse request, where their own mount identity is resolved.
pub fn browse(path: &str) -> Result<DirectoryListing> {
    let path = absolute(path)?;
    let observed = observe(&path, &mounts()?)?;
    browse_observed(observed, true)
}

fn browse_observed(observed: Observed, report_links: bool) -> Result<DirectoryListing> {
    let path = &observed.path;
    let browsing_root = OpenAt2Root::from_directory(
        observed.storage.directory().try_clone()?,
        MountPolicy::AllowNestedMounts,
    )?;
    let mut entries = Vec::new();
    let mut skipped_non_utf8 = 0;
    let mut scanned = observed.storage.entries()?;
    let mut truncated = false;
    for index in 0..=MAX_SCANNED {
        let Some(entry) = scanned.next() else {
            break;
        };
        let entry = entry?;
        if index == MAX_SCANNED || entries.len() == MAX_ENTRIES {
            truncated = true;
            break;
        }
        let Some(name) = entry.as_os_str().to_str() else {
            skipped_non_utf8 += 1;
            continue;
        };
        let stat = unix::statat(
            observed.storage.directory(),
            entry.as_os_str(),
            AtFlags::SYMLINK_NOFOLLOW,
        )?;
        let kind = FileType::from_raw_mode(stat.st_mode);
        if kind != FileType::Directory && !(report_links && kind == FileType::Symlink) {
            continue;
        }
        let child = path.join(name);
        let problem = if kind == FileType::Symlink {
            Some(DirectoryProblem::SymbolicLink)
        } else {
            // Anchor navigation probes to this exact listed directory, including
            // nested mounts. Never reopen a child through an absolute raceable path.
            match browsing_root.open_directory(&RelativePath::new(Path::new(name))?) {
                Ok(directory) => match unix::accessat(
                    &directory,
                    ".",
                    Access::READ_OK | Access::EXEC_OK,
                    AtFlags::EACCESS,
                ) {
                    Ok(()) => None,
                    Err(rustix::io::Errno::ACCESS) => Some(DirectoryProblem::PermissionDenied),
                    Err(_) => Some(DirectoryProblem::Unavailable),
                },
                Err(fs_safety::Error::Io(error))
                    if error.kind() == io::ErrorKind::PermissionDenied =>
                {
                    Some(DirectoryProblem::PermissionDenied)
                }
                Err(_) => Some(DirectoryProblem::Unavailable),
            }
        };
        entries.push(DirectoryEntry {
            name: name.into(),
            path: child.to_str().expect("UTF-8 parent and name").into(),
            can_browse: problem.is_none(),
            problem,
        });
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    verify_current(&observed)?;
    Ok(DirectoryListing {
        path: path.to_str().expect("validated UTF-8 path").into(),
        mount_path: observed
            .mount
            .path
            .to_str()
            .expect("validated UTF-8 mount")
            .into(),
        mount_identity: observed.mount_identity,
        identity: observed.root_identity,
        entries,
        truncated,
        skipped_non_utf8,
    })
}

fn selection(observed: &Observed, scope: &DiscoveryScope) -> CandidateSelection {
    let mut selection = CandidateSelection {
        selection_id: String::new(),
        discovery_root: scope
            .observed
            .path
            .to_str()
            .expect("validated UTF-8 discovery root")
            .into(),
        discovery_identity: scope.observed.root_identity.clone(),
        mount_path: observed
            .mount
            .path
            .to_str()
            .expect("validated UTF-8 mount")
            .into(),
        root_path: observed.path.to_str().expect("validated UTF-8 path").into(),
        mount_identity: observed.mount_identity.clone(),
        root_identity: observed.root_identity.clone(),
    };
    let bytes = json_codec::to_vec(&selection).expect("directory identity serializes");
    selection.selection_id = format!("directory_{}", blake3::hash(&bytes).to_hex());
    selection
}

fn inspect_held(
    request: &InspectRequest,
    excluded_roots: &[PathBuf],
    discovery_root: &str,
    strict_descendant: bool,
) -> Result<HeldSelection> {
    let root_path = absolute(&request.root_path)?;
    let requested_mount = absolute(&request.mount_path)?;
    let inventory = mounts()?;
    let scope = DiscoveryScope::open(discovery_root, &inventory)?;
    let opened = scope.open_descendant(&root_path, strict_descendant)?;
    let observed = observe_opened(&root_path, opened, &inventory)?;
    if observed.mount.path != requested_mount {
        return Err(Error::Changed);
    }
    reject_non_storage_filesystem(&observed.mount.filesystem)?;
    reject_overlaps(&observed, excluded_roots, &inventory)?;
    // Use the same actual descriptor, owner and mode policy as initialization.
    let _private = fs_safety::PrivateDirectory::from_directory(
        observed.storage.directory().try_clone()?,
        root_path,
    )?;
    unix::accessat(
        observed.storage.directory(),
        ".",
        Access::READ_OK | Access::WRITE_OK | Access::EXEC_OK,
        AtFlags::EACCESS,
    )?;
    if observed.storage.entries()?.next().transpose()?.is_some() {
        return Err(Error::NotEmpty);
    }
    verify_current(&observed)?;
    verify_current(&scope.observed)?;
    let selected = selection(&observed, &scope);
    Ok(HeldSelection {
        selected,
        directory: observed.storage.into_directory(),
        _mount: observed.mount_root.into_directory(),
        _discovery: scope.observed.storage.into_directory(),
    })
}

/// Inspect a currently empty, private directory without changing it. Permission
/// checks are an observation; quota, future I/O and physical redundancy are not proved.
pub fn inspect(request: &InspectRequest, excluded_roots: &[PathBuf]) -> Result<CandidateSelection> {
    Ok(inspect_held(request, excluded_roots, &request.mount_path, false)?.selected)
}

pub struct HeldSelection {
    selected: CandidateSelection,
    directory: File,
    _mount: File,
    _discovery: File,
}
impl HeldSelection {
    pub fn selection(&self) -> &CandidateSelection {
        &self.selected
    }
    pub fn into_storage_directory(self) -> File {
        self.directory
    }
}

/// Recheck the exact observation before activation, returning its still-held FD.
/// Callers must consume this FD, not reopen root_path, and recheck exclusions for
/// the entire selected set. This is not a cross-reboot configuration proof.
pub fn revalidate(
    selected: &CandidateSelection,
    excluded_roots: &[PathBuf],
) -> Result<HeldSelection> {
    let held = inspect_held(
        &InspectRequest {
            mount_path: selected.mount_path.clone(),
            root_path: selected.root_path.clone(),
        },
        excluded_roots,
        &selected.discovery_root,
        false,
    )?;
    if held.selected != *selected {
        return Err(Error::Changed);
    }
    Ok(held)
}

struct DiscoveryScope {
    observed: Observed,
    directory: OpenAt2Root,
}
impl DiscoveryScope {
    fn open(root: &str, inventory: &[Mount]) -> Result<Self> {
        let path = absolute(root)?;
        let observed = observe(&path, inventory)?;
        unix::accessat(
            observed.storage.directory(),
            ".",
            Access::READ_OK | Access::EXEC_OK,
            AtFlags::EACCESS,
        )?;
        // Opening a directory iterator checks real enumeration access without
        // collecting or scanning unrelated names. Root need not be private 0700.
        let _entries = observed.storage.entries()?;
        let directory = OpenAt2Root::from_directory(
            observed.storage.directory().try_clone()?,
            MountPolicy::AllowNestedMounts,
        )?;
        Ok(Self {
            observed,
            directory,
        })
    }
    fn open_descendant(&self, path: &Path, strict: bool) -> Result<OpenAt2Root> {
        let relative = path
            .strip_prefix(&self.observed.path)
            .map_err(|_| Error::OutsideDiscoveryRoot)?;
        let directory = if relative.as_os_str().is_empty() {
            if strict {
                return Err(Error::OutsideDiscoveryRoot);
            }
            self.directory.directory().try_clone()?
        } else {
            // Component membership plus openat2 BENEATH/NO_SYMLINKS is the
            // boundary. A text prefix alone would admit siblings and link races.
            self.directory
                .open_directory(&RelativePath::new(relative)?)?
        };
        Ok(OpenAt2Root::from_directory(
            directory,
            MountPolicy::AllowNestedMounts,
        )?)
    }
}

/// Validate an explicitly configured discovery root without creating it or
/// requiring storage-root ownership/mode. Exclusions are server-owned system roots.
pub fn validate_discovery_root(root: &str, excluded_roots: &[PathBuf]) -> Result<()> {
    let inventory = mounts()?;
    let scope = DiscoveryScope::open(root, &inventory)?;
    reject_overlaps(&scope.observed, excluded_roots, &inventory)?;
    verify_current(&scope.observed)
}

/// Public management navigation must use this scope, never the unrestricted
/// low-level browse helper. The root itself is only a listing container.
pub fn browse_within(discovery_root: &str, path: &str) -> Result<DirectoryListing> {
    let inventory = mounts()?;
    let scope = DiscoveryScope::open(discovery_root, &inventory)?;
    let path = absolute(path)?;
    let opened = scope.open_descendant(&path, false)?;
    let observed = observe_opened(&path, opened, &inventory)?;
    let listing = browse_observed(observed, false)?;
    verify_current(&scope.observed)?;
    Ok(listing)
}

/// Inspect only strict descendants of the server's current configured root.
/// The actual filesystem mount may be outside that discovery root.
pub fn inspect_within(
    discovery_root: &str,
    request: &InspectRequest,
    excluded_roots: &[PathBuf],
) -> Result<CandidateSelection> {
    Ok(inspect_held(request, excluded_roots, discovery_root, true)?.selected)
}

/// Activation must call this while holding the settings/lifecycle lock. A new
/// configured root or replaced discovery directory invalidates old previews.
pub fn revalidate_within(
    discovery_root: &str,
    selected: &CandidateSelection,
    excluded_roots: &[PathBuf],
) -> Result<HeldSelection> {
    if absolute(discovery_root)? != absolute(&selected.discovery_root)? {
        return Err(Error::Changed);
    }
    let held = inspect_held(
        &InspectRequest {
            mount_path: selected.mount_path.clone(),
            root_path: selected.root_path.clone(),
        },
        excluded_roots,
        discovery_root,
        true,
    )?;
    if held.selected != *selected {
        return Err(Error::Changed);
    }
    Ok(held)
}

#[cfg(test)]
#[path = "../../../test/server/unit/storage_directories.rs"]
mod tests;
