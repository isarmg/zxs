//! Ordinary version files in one renameable directory per stable file identity.
use crate::{disk::*, *};
mod changes;
mod projection;
use crate::state_projection::{ProjectionTransition, StateProjection};
use changes::changed_files;
use date_time::{OffsetDateTime, format_description::well_known::Rfc3339};
use json_codec::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Seek, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::Arc,
};

fn invalid(message: &str) -> Error {
    Error::new(storage_contract::ErrorCode::ContainerLayoutInvalid, message)
}

fn sequence(name: &str) -> Result<u64> {
    name_policy::validate_name(name)?;
    let mut parts = name.splitn(3, '_');
    let number = parts
        .next()
        .and_then(|part| part.strip_prefix('v'))
        .ok_or_else(|| invalid("version storage name has no sequence"))?;
    let stamp = parts
        .next()
        .ok_or_else(|| invalid("version storage name has no timestamp"))?;
    let suffix = parts
        .next()
        .ok_or_else(|| invalid("version storage name has no digest"))?;
    let digest = suffix.split('.').next().unwrap_or("");
    if number.len() < 6
        || !number.bytes().all(|byte| byte.is_ascii_digit())
        || stamp.len() != 16
        || stamp.as_bytes()[8] != b'T'
        || stamp.as_bytes()[15] != b'Z'
        || !stamp
            .bytes()
            .enumerate()
            .all(|(index, byte)| index == 8 || index == 15 || byte.is_ascii_digit())
        || digest.len() != 8
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid(
            "version storage name is not a numbered UTC checkpoint",
        ));
    }
    let value = number.parse::<u64>().map_err(|source| {
        Error::from(source).context(
            ErrorCode::ContainerLayoutInvalid,
            "version sequence exceeds the storage format",
        )
    })?;
    if value == 0 {
        return Err(invalid("version storage sequence starts at one"));
    }
    Ok(value)
}

pub(crate) fn validate_version(version: &FileVersion) -> Result<()> {
    validate_id(&version.file_id, "file")?;
    validate_id(&version.history_tree_id, "history")?;
    validate_id(&version.version_id, "ver")?;
    name_policy::validate_name(&version.creation_name)?;
    let name = &version.storage_name;
    sequence(name)?;
    let digest = name
        .splitn(3, '_')
        .nth(2)
        .and_then(|value| value.split('.').next())
        .unwrap_or("");
    if digest != &version.content_hash.digest().to_hex()[..8] {
        return Err(invalid(
            "version storage name does not identify its content digest",
        ));
    }
    if version.created_at.len() != 20
        || version.created_at.as_bytes()[10] != b'T'
        || !version.created_at.ends_with('Z')
    {
        return Err(invalid("version creation time must be whole-second UTC"));
    }
    OffsetDateTime::parse(&version.created_at, &Rfc3339).map_err(|source| {
        Error::from(source).context(
            ErrorCode::ContainerLayoutInvalid,
            "version creation time is not RFC3339",
        )
    })?;
    Ok(())
}

/// Per-file storage sequence; the existing global audit sequence is unchanged.
pub(crate) fn version_identity(
    state: &State,
    file_id: &str,
    path: &str,
    hash: &blake3::Hash,
) -> Result<(String, String)> {
    validate_id(file_id, "file")?;
    let mut maximum = 0;
    for version in state
        .versions
        .values()
        .filter(|version| version.file_id == file_id)
    {
        // Snapshot/import admission and version builders preserve this identity.
        maximum = maximum.max(
            sequence(&version.storage_name)
                .expect("invariant: admitted or generated versions have valid storage names"),
        );
    }
    let number = maximum.checked_add(1).ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::ResourceLimit,
            "file version sequence exhausted",
        )
    })?;
    let now = version_time(std::time::SystemTime::now())?;
    let timestamp = now
        .format(&Rfc3339)
        .expect("invariant: nonnegative UTC time is within the RFC3339 year range");
    let format = date_time::format_description::parse_borrowed::<3>(
        "[year][month][day]T[hour][minute][second]Z",
    )
    .expect("invariant: static checkpoint format is valid");
    let stamp = now
        .format(&format)
        .expect("invariant: UTC datetime contains every checkpoint format component");
    let current_name = state
        .file_names
        .get(file_id)
        .map(String::as_str)
        .unwrap_or(path);
    name_policy::validate_path(current_name)?;
    let extension = Path::new(current_name)
        .extension()
        .and_then(|name| name.to_str())
        .map(|name| format!(".{name}"))
        .unwrap_or_default();
    let name = format!("v{number:06}_{stamp}_{}{extension}", &hash.to_hex()[..8]);
    name_policy::validate_name(&name)?;
    Ok((name, timestamp))
}

#[cfg(test)]
fn directory_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            existing_private_dir(path)?;
            Ok(true)
        }
        Ok(_) => Err(invalid(
            "container path must be a private directory without symbolic links",
        )),
    }
}

fn directory_exists_at(root: &DiskRoot, path: &Path) -> Result<bool> {
    match root.optional_metadata(path)? {
        None => Ok(false),
        Some(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            root.directory(path)?;
            Ok(true)
        }
        Some(_) => Err(invalid(
            "container path must be a private directory without symbolic links",
        )),
    }
}

fn path_directory_exists_at(root: &DiskRoot, base: &Path, path: &Path) -> Result<bool> {
    if !path_exists_at(root, base, path)? {
        return Ok(false);
    }
    directory_exists_at(root, path)
}

// Missing ancestors are part of the existing MISSING classification. Inspect
// them through the held root without imposing new private-mode requirements.
fn path_exists_at(root: &DiskRoot, base: &Path, path: &Path) -> Result<bool> {
    let relative = path
        .strip_prefix(base)
        .expect("invariant: container paths remain below their admitted base");
    let mut current = base.to_path_buf();
    for component in relative.components() {
        current.push(component);
        if root.optional_metadata(&current)?.is_none() {
            return Ok(false);
        }
    }
    Ok(true)
}

fn history_root(directory: &Path) -> Result<&Path> {
    directory
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| invalid("version metadata has no history or package root"))
}

/// Return the unique visible container, rejecting ambiguity instead of choosing
/// an arbitrary directory. A missing or empty container remains a missing file.
const NAMESPACE_MARKER: &str = ".zxs-namespace.json";
const OWNER_MARKER: &str = ".zxs-container-owner.json";

fn marker_matches_at(
    root: &DiskRoot,
    path: &Path,
    file_id: &str,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<bool> {
    observe(0)?;
    match root.optional_metadata(path)? {
        None => return Ok(false),
        Some(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            return Ok(false);
        }
        Some(_) => {}
    }
    if !file_exists_at(root, path)? || root.read_json_observed::<String>(path, observe)? != file_id
    {
        return Err(invalid(
            "container location marker differs from its stable file identity",
        ));
    }
    Ok(true)
}

// Unsupported on-disk layouts are never migrated or cleaned automatically.
// Inspect metadata only; preserve all old bytes, including empty placeholders.
fn reject_unsupported_container(root: &DiskRoot, named: &Path) -> Result<()> {
    for name in ["pending-branches", "legacy"] {
        if root.optional_metadata(&named.join(name))?.is_some() {
            return Err(Error::new(
                storage_contract::ErrorCode::FormatUnsupported,
                "legacy container storage is not supported",
            ));
        }
    }
    Ok(())
}

fn named_containers(
    disk: &DiskRoot,
    root: &Path,
    file_id: &str,
    depth: usize,
    selected: &mut Option<PathBuf>,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<()> {
    if depth > 33 {
        return Err(invalid("container namespace exceeds the path depth limit"));
    }
    for entry in disk.entries(root)? {
        observe(0)?;
        let entry = entry?;
        let path = root.join(entry.as_path());
        if depth == 0 && entry.as_os_str() == NAMESPACE_MARKER {
            if !marker_matches_at(disk, &path, file_id, observe)? {
                return Err(invalid("container namespace marker is missing"));
            }
            continue;
        }
        if !disk
            .optional_metadata(&path)?
            .is_some_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        {
            return Err(invalid(
                "container namespace contains a non-directory or symbolic link",
            ));
        }
        let name = entry.as_os_str().to_str().ok_or_else(|| {
            Error::PathEncoding(entry.as_os_str().to_owned()).context(
                ErrorCode::ContainerLayoutInvalid,
                "container directory name is not UTF-8",
            )
        })?;
        name_policy::validate_name(name)?;
        disk.directory(&path)?;
        if marker_matches_at(disk, &path.join(OWNER_MARKER), file_id, observe)? {
            reject_unsupported_container(disk, &path)?;
            if !directory_exists_at(disk, &path.join("versions"))? {
                return Err(invalid("owned container has no version directory"));
            }
            if selected.replace(path).is_some() {
                return Err(Error::new(
                    storage_contract::ErrorCode::ContainerAmbiguous,
                    "stable file identity has more than one named container",
                ));
            }
        } else {
            named_containers(disk, &path, file_id, depth + 1, selected, observe)?;
        }
    }
    Ok(())
}

#[cfg(test)]
fn container(history: &Path, file_id: &str) -> Result<Option<PathBuf>> {
    validate_id(file_id, "file")?;
    if !directory_exists(history)? {
        return Ok(None);
    }
    let root = DiskRoot::open(history)?;
    container_at(&root, root.path(), file_id, &mut |_| Ok(()))
}

fn container_at(
    disk: &DiskRoot,
    history: &Path,
    file_id: &str,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<Option<PathBuf>> {
    let root = history.join("file-containers");
    if !directory_exists_at(disk, &root)? {
        return Ok(None);
    }
    let root = root.join(file_id);
    if !directory_exists_at(disk, &root)? {
        return Ok(None);
    }
    let mut selected = None;
    for entry in disk.entries(&root)? {
        observe(0)?;
        let entry = entry?;
        let path = root.join(entry.as_path());
        if !disk
            .optional_metadata(&path)?
            .is_some_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        {
            return Err(invalid(
                "stable file directory contains a non-directory or symbolic link",
            ));
        }
        let name = entry.as_os_str().to_str().ok_or_else(|| {
            Error::PathEncoding(entry.as_os_str().to_owned()).context(
                ErrorCode::ContainerLayoutInvalid,
                "container directory name is not UTF-8",
            )
        })?;
        name_policy::validate_name(name)?;
        disk.directory(&path)?;
        if name == "namespace"
            && marker_matches_at(disk, &path.join(NAMESPACE_MARKER), file_id, observe)?
        {
            // The stable identity is the lookup key; logical aliases never
            // choose the physical location. Empty source parents left by an
            // interrupted move are harmless, but two payload owners fail closed.
            named_containers(disk, &path, file_id, 0, &mut selected, observe)?;
            continue;
        }
        reject_unsupported_container(disk, &path)?;
        if selected.replace(path).is_some() {
            return Err(Error::new(
                storage_contract::ErrorCode::ContainerAmbiguous,
                "stable file identity has more than one named container",
            ));
        }
    }
    Ok(selected)
}

fn file_exists_at(root: &DiskRoot, path: &Path) -> Result<bool> {
    match root.optional_metadata(path)? {
        None => Ok(false),
        Some(metadata)
            if metadata.is_file()
                && !metadata.file_type().is_symlink()
                && metadata.nlink() == 1 =>
        {
            root.open_regular(path)?;
            Ok(true)
        }
        Some(_) => Err(invalid("version body must be one ordinary unlinked file")),
    }
}

#[cfg(test)]
pub(crate) fn content_path(directory: &Path, version: &FileVersion) -> Result<PathBuf> {
    validate_version(version)?;
    let history = history_root(directory)?;
    if !directory_exists(history)? {
        return Ok(expected_content_path(history, version));
    }
    let root = DiskRoot::open(history)?;
    let directory = root.path().join(
        directory
            .strip_prefix(history)
            .expect("invariant: version directory is below its history"),
    );
    content_path_inner(&root, &directory, version, &mut |_| Ok(()))
}

/// Missing bodies return their expected path so repair can retain its existing
/// MISSING classification. Only malformed or ambiguous layout fails closed.
pub(crate) fn content_path_at(
    root: &DiskRoot,
    directory: &Path,
    version: &FileVersion,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<PathBuf> {
    validate_version(version)?;
    observe(0)?;
    content_path_inner(root, directory, version, observe)
}

fn expected_content_path(history: &Path, version: &FileVersion) -> PathBuf {
    history
        .join("file-containers")
        .join(&version.file_id)
        .join(&version.creation_name)
        .join("versions")
        .join(&version.storage_name)
}

fn content_path_inner(
    root: &DiskRoot,
    directory: &Path,
    version: &FileVersion,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<PathBuf> {
    let prepared = directory.join("content").join(&version.creation_name);
    let history = history_root(directory)?;
    if !path_directory_exists_at(root, root.path(), history)? {
        return Ok(expected_content_path(history, version));
    }
    let container = container_at(root, history, &version.file_id, observe)?;
    let target = match container {
        Some(named) => {
            let versions = named.join("versions");
            let target = versions.join(&version.storage_name);
            if directory_exists_at(root, &versions)? && file_exists_at(root, &target)? {
                return Ok(target);
            }
            target
        }
        None => expected_content_path(history, version),
    };
    // Current preparation and offline packages hold the verified body beside
    // its immutable metadata until installation moves it into the container.
    if path_exists_at(root, history, &directory.join("content"))?
        && file_exists_at(root, &prepared)?
    {
        return Ok(prepared);
    }
    Ok(target)
}

fn verify_body(root: &DiskRoot, path: &Path, version: &FileVersion) -> Result<fs::File> {
    if !file_exists_at(root, path)? {
        return Err(Error::new(
            storage_contract::ErrorCode::ContentMissing,
            "version body is missing",
        ));
    }
    let mut file = root.open_regular(path)?;
    if file.metadata()?.len() != version.logical_size {
        return Err(Error::new(
            storage_contract::ErrorCode::ContentCorrupt,
            "version body length differs",
        ));
    }
    let mut digest = blake3::Hasher::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 65_536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        size = size.checked_add(count as u64).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::ContentCorrupt,
                "version body grew beyond its declared size",
            )
        })?;
        if size > version.logical_size {
            return Err(Error::new(
                storage_contract::ErrorCode::ContentCorrupt,
                "version body grew beyond its declared size",
            ));
        }
        digest.update(&buffer[..count]);
    }
    if size != version.logical_size
        || ContentHash::from_digest(digest.finalize()) != version.content_hash
    {
        return Err(Error::new(
            storage_contract::ErrorCode::ContentCorrupt,
            "version body content differs",
        ));
    }
    // Existing/recovered bodies carry no VerifiedCopy durability receipt. This
    // installation barrier is not a read-path flush: callers publish protection
    // only after the body and its destination directory are durable.
    file.sync_all()?;
    Ok(file)
}

pub(crate) enum PreparedContent<'a> {
    Verified(VerifiedExisting<'a>),
    Copied(VerifiedCopy<'a>),
}

#[cfg(test)]
pub(crate) fn install_version_content(
    directory: &Path,
    content: PreparedContent<'_>,
) -> Result<PathBuf> {
    let history = history_root(directory)?;
    let root = DiskRoot::open(history)?;
    let directory = root.path().join(
        directory
            .strip_prefix(history)
            .expect("invariant: version directory is below its history"),
    );
    install_version_content_at(&root, &directory, content)
}

/// Move a verified prepared body on its existing filesystem. Never hardlink,
/// reflink or duplicate the payload to represent a version or rename.
pub(crate) fn install_version_content_at(
    root: &DiskRoot,
    directory: &Path,
    content: PreparedContent<'_>,
) -> Result<PathBuf> {
    let (version, copied, verified) = match content {
        PreparedContent::Verified(verified) => (verified.version(), None, Some(verified)),
        PreparedContent::Copied(copied) => {
            let (version, file) = copied.into_parts();
            (version, Some(file), None)
        }
    };
    validate_version(version)?;
    let source = directory.join("content").join(&version.creation_name);
    let name = &version.storage_name;
    let history = history_root(directory)?;
    root.directory(history)?;
    let stable = history.join("file-containers").join(&version.file_id);
    root.private_dir(&stable)?;
    let named = match container_at(root, history, &version.file_id, &mut |_| Ok(()))? {
        Some(named) => named,
        None => {
            let named = stable.join(&version.creation_name);
            root.private_dir(&named)?;
            root.sync_dir(&stable)?;
            named
        }
    };
    let versions = named.join("versions");
    root.private_dir(&versions)?;
    let target = versions.join(name);
    if file_exists_at(root, &target)? {
        if let Some(verified) = &verified
            && verified.path() == target
        {
            verified.sync_at(root, directory, &target)?;
        } else {
            verify_body(root, &target, version)?;
        }
        let source_parent = source.parent().expect("invariant: source has a parent");
        if path_directory_exists_at(root, history, source_parent)? && file_exists_at(root, &source)?
        {
            let _source = match copied {
                Some(file) => Some(file),
                None => {
                    if let Some(verified) = &verified
                        && verified.path() == source
                    {
                        verified.sync_at(root, directory, &source)?;
                        None
                    } else {
                        Some(verify_body(root, &source, version)?)
                    }
                }
            };
            let parent = root.directory(source_parent)?;
            rustix::fs::unlinkat(
                &parent,
                source.file_name().expect("source name"),
                rustix::fs::AtFlags::empty(),
            )
            .map_err(std::io::Error::from)?;
            parent.sync_all()?;
        }
        // Retrying after an interrupted directory sync must complete durability
        // even when the exclusive move already made the target visible.
        if path_directory_exists_at(root, history, source_parent)? {
            root.sync_dir(source_parent)?;
        }
        root.sync_dir(&versions)?;
        root.sync_dir(&named)?;
        root.sync_dir(&stable)?;
        return Ok(target);
    }
    let _source = match copied {
        Some(file) => Some(file),
        None => {
            if let Some(verified) = &verified {
                verified.sync_at(root, directory, &source)?;
                None
            } else {
                Some(verify_body(root, &source, version)?)
            }
        }
    };
    let from = root.directory(source.parent().expect("source parent"))?;
    let to = root.directory(&versions)?;
    rustix::fs::renameat_with(
        &from,
        source.file_name().expect("source name"),
        &to,
        name.as_str(),
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(std::io::Error::from)?;
    from.sync_all()?;
    to.sync_all()?;
    root.sync_dir(&named)?;
    Ok(target)
}

/// Initialize missing physical identities from the published namespace. Saved
/// branch aliases never retarget an existing container; actual namespace edits
/// update these fields explicitly, and deletion retains the last owner.
pub(crate) fn capture_names(state: &mut State) {
    let State {
        repositories,
        branches,
        commits,
        file_names,
        file_locations,
        ..
    } = state;
    let entries = repositories
        .values()
        .flat_map(|repository| &main_head(repository, branches, commits).entries);
    capture_entry_names(entries, file_names, file_locations);
}

fn main_head<'a>(
    repository: &Repository,
    branches: &'a storage_contract::TrackedMap<Branch>,
    commits: &'a storage_contract::TrackedRecords<Commit>,
) -> &'a Commit {
    let branch = branches
        .get(&repository.main_branch_id)
        .expect("invariant: every repository has its main branch");
    commits
        .get(&branch.head_commit_id)
        .expect("invariant: every branch has its head commit")
}

fn capture_entry_names<'a>(
    entries: impl Iterator<Item = &'a Entry>,
    file_names: &mut storage_contract::TrackedMap<String>,
    file_locations: &mut storage_contract::TrackedMap<String>,
) {
    for entry in entries.filter(|entry| entry.kind == NamespaceEntryKind::File) {
        if !file_names.contains_key(&entry.file_id) {
            file_names.insert(entry.file_id.clone(), entry.name().to_owned());
        }
        if !file_locations.contains_key(&entry.file_id) {
            file_locations.insert(entry.file_id.clone(), entry.path().to_owned());
        }
    }
}

/// The planner captured every previous main HEAD before invoking its private
/// mutation closure. Business writers retain identity keys; only explicit
/// namespace edits replace their values. Unchanged heads therefore need no scan.
pub(crate) fn capture_changed_names(previous: &State, next: &mut State) {
    let Some(repositories) = changed_namespace_repositories(previous, next) else {
        capture_names(next);
        return;
    };
    let State {
        repositories: current,
        branches,
        commits,
        file_names,
        file_locations,
        ..
    } = next;
    // Preserve the full capture's repository order and each commit's entry order.
    let entries = repositories
        .iter()
        .map(|id| &current[id])
        .flat_map(|repository| &main_head(repository, branches, commits).entries);
    capture_entry_names(entries, file_names, file_locations);
}

fn changed_namespace_repositories(previous: &State, next: &State) -> Option<BTreeSet<String>> {
    use storage_contract::MapChanges;
    let MapChanges::Keys(repositories) = next.repositories.changes_since(&previous.repositories)
    else {
        return None;
    };
    let MapChanges::Keys(branches) = next.branches.changes_since(&previous.branches) else {
        return None;
    };
    let MapChanges::Keys(commits) = next.commits.changes_since(&previous.commits) else {
        return None;
    };
    let mut repositories = repositories.as_ref().clone();
    for state in [previous, next] {
        for branch in branches.iter().filter_map(|id| state.branches.get(id)) {
            let repository = namespace_repository(state, &branch.history_tree_id);
            if repository.main_branch_id == branch.branch_id {
                repositories.insert(repository.repository_id.clone());
            }
        }
        for commit in commits.iter().filter_map(|id| state.commits.get(id)) {
            let repository = namespace_repository(state, &commit.history_tree_id);
            let head = main_head(repository, &state.branches, &state.commits);
            if head.commit_id == commit.commit_id {
                repositories.insert(repository.repository_id.clone());
            }
        }
    }
    repositories.retain(|id| {
        let Some(repository) = next.repositories.get(id) else {
            return false;
        };
        let head = main_head(repository, &next.branches, &next.commits);
        let previous_head = previous
            .repositories
            .get(id)
            .map(|repository| main_head(repository, &previous.branches, &previous.commits));
        previous_head.is_none_or(|previous| previous.commit_id != head.commit_id)
            || commits.contains(&head.commit_id)
    });
    Some(repositories)
}

fn namespace_repository<'a>(state: &'a State, history: &str) -> &'a Repository {
    let history = state
        .histories
        .get(history)
        .expect("invariant: every branch and commit belongs to its history");
    state
        .repositories
        .get(&history.repository_id)
        .expect("invariant: every history has its repository")
}

fn synchronize_location(
    root: &DiskRoot,
    current: PathBuf,
    stable: &Path,
    file_id: &str,
    location: Option<&str>,
    name: &str,
) -> Result<PathBuf> {
    let original_parent = current.parent().map(Path::to_path_buf);
    let Some(location) = location else {
        let target = current.parent().expect("container parent").join(name);
        if target != current {
            let parent = root.directory(current.parent().expect("container parent"))?;
            rustix::fs::renameat_with(
                &parent,
                current.file_name().expect("container name"),
                &parent,
                name,
                rustix::fs::RenameFlags::NOREPLACE,
            )
            .map_err(std::io::Error::from)?;
            parent.sync_all()?;
        }
        return Ok(target);
    };
    name_policy::validate_path(location)?;
    if location.rsplit('/').next() != Some(name) {
        return Err(invalid(
            "canonical container name differs from its physical location",
        ));
    }
    let namespace = stable.join("namespace");
    let mut current = current;
    // Initial installation may name a current-format file `namespace` before
    // its location is projected. Move that container aside for retry-safe projection.
    if current == namespace
        && !marker_matches_at(
            root,
            &namespace.join(NAMESPACE_MARKER),
            file_id,
            &mut |_| Ok(()),
        )?
    {
        let temporary_name = format!(".zxs-relocate-{}", new_id("req"));
        let parent = root.directory(stable)?;
        rustix::fs::renameat_with(
            &parent,
            "namespace",
            &parent,
            temporary_name.as_str(),
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(std::io::Error::from)?;
        parent.sync_all()?;
        current = stable.join(temporary_name);
    }
    root.private_dir(&namespace)?;
    if !marker_matches_at(
        root,
        &namespace.join(NAMESPACE_MARKER),
        file_id,
        &mut |_| Ok(()),
    )? {
        root.write_json(&namespace.join(NAMESPACE_MARKER), &file_id, false)?;
    }
    let target = namespace.join(location);
    if target != current {
        if target.starts_with(&current) || current.starts_with(&target) {
            let temporary_name = format!(".zxs-relocate-{}", new_id("req"));
            let source = root.directory(current.parent().expect("current container parent"))?;
            let destination = root.directory(stable)?;
            rustix::fs::renameat_with(
                &source,
                current.file_name().expect("current container name"),
                &destination,
                temporary_name.as_str(),
                rustix::fs::RenameFlags::NOREPLACE,
            )
            .map_err(std::io::Error::from)?;
            source.sync_all()?;
            destination.sync_all()?;
            current = stable.join(temporary_name);
        }
        if path_directory_exists_at(root, &namespace, &target)? {
            // Only structural empty parents can be replaced. Validate all of
            // them before removing any; unexpected bytes or links stay intact.
            let mut empty = Vec::new();
            collect_empty_directories(root, &target, 0, &mut empty)?;
            for directory in empty {
                let parent = root.directory(directory.parent().expect("empty parent"))?;
                rustix::fs::unlinkat(
                    &parent,
                    directory.file_name().expect("empty name"),
                    rustix::fs::AtFlags::REMOVEDIR,
                )
                .map_err(std::io::Error::from)?;
                parent.sync_all()?;
            }
        }
        let destination_parent = target.parent().expect("canonical location parent");
        root.private_dir(destination_parent)?;
        let mut directory = Some(destination_parent);
        while let Some(parent) = directory {
            root.sync_dir(parent)?;
            if parent == stable {
                break;
            }
            directory = parent.parent();
        }
        if !marker_matches_at(root, &current.join(OWNER_MARKER), file_id, &mut |_| Ok(()))? {
            root.write_json(&current.join(OWNER_MARKER), &file_id, false)?;
        }
        root.sync_dir(&current)?;
        let source = root.directory(current.parent().expect("current container parent"))?;
        let destination = root.directory(destination_parent)?;
        rustix::fs::renameat_with(
            &source,
            current.file_name().expect("current container name"),
            &destination,
            target.file_name().expect("target container name"),
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(std::io::Error::from)?;
        source.sync_all()?;
        destination.sync_all()?;
    }
    root.sync_dir(&namespace)?;
    root.sync_dir(stable)?;
    if let Some(mut parent) = original_parent {
        while parent != namespace
            && parent.starts_with(&namespace)
            && path_directory_exists_at(root, &namespace, &parent)?
        {
            if root.entries(&parent)?.next().transpose()?.is_some() {
                break;
            }
            let ancestor = parent.parent().expect("namespace ancestor").to_path_buf();
            let held = root.directory(&ancestor)?;
            rustix::fs::unlinkat(
                &held,
                parent.file_name().expect("empty namespace parent"),
                rustix::fs::AtFlags::REMOVEDIR,
            )
            .map_err(std::io::Error::from)?;
            held.sync_all()?;
            parent = ancestor;
        }
    }
    Ok(target)
}

fn collect_empty_directories(
    disk: &DiskRoot,
    root: &Path,
    depth: usize,
    directories: &mut Vec<PathBuf>,
) -> Result<()> {
    if depth > 33 {
        return Err(invalid("container namespace exceeds the path depth limit"));
    }
    disk.directory(root)?;
    for entry in disk.entries(root)? {
        let entry = entry?;
        let path = root.join(entry.as_path());
        if !disk
            .optional_metadata(&path)?
            .is_some_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        {
            return Err(invalid(
                "canonical container target contains unexpected data",
            ));
        }
        collect_empty_directories(disk, &path, depth + 1, directories)?;
    }
    directories.push(root.to_path_buf());
    Ok(())
}

/// Fully encoded, bounded projections. Preparation never changes published names
/// or caches. Signed version evidence remains separate from this inspection cache.
pub(crate) struct PreparedNames {
    cache: Option<PreparedNameCache>,
}
struct PreparedNameCache {
    items: Vec<PreparedName>,
    // Lazily create one anonymous file only when a projection exists. Keeping
    // items with their file makes an unstaged projection unrepresentable.
    file: fs::File,
}
struct PreparedName {
    disk: Arc<DiskHandle>,
    history: PathBuf,
    file: String,
    location: Option<String>,
    name: String,
    offset: u64,
    length: usize,
}
impl PreparedNames {
    pub(crate) fn apply(self) -> Result<()> {
        let Some(mut cache) = self.cache else {
            return Ok(());
        };
        for item in cache.items {
            let root = &item.disk.root;
            if !path_directory_exists_at(root, root.path(), &item.history)? {
                continue;
            }
            let Some(current) = container_at(root, &item.history, &item.file, &mut |_| Ok(()))?
            else {
                continue;
            };
            let stable = item.history.join("file-containers").join(&item.file);
            let named = synchronize_location(
                root,
                current,
                &stable,
                &item.file,
                item.location.as_deref(),
                &item.name,
            )?;
            // Seal even a rename made visible by an earlier interrupted attempt.
            root.sync_dir(&stable)?;
            cache.file.seek(std::io::SeekFrom::Start(item.offset))?;
            let mut bytes = vec![0; item.length];
            cache.file.read_exact(&mut bytes)?;
            root.write_atomic(&named.join("container.json"), &bytes, true)?;
            root.sync_dir(&named)?;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn synchronize_names(
    config: &ValidatedConfig,
    previous: &State,
    next: &State,
) -> Result<()> {
    let projection = ProjectionTransition::new(previous, next, None)?;
    let resources = Arc::new(DiskResources::open(config, true)?);
    prepare_names(config, &resources, &projection, MAX_METADATA)?.apply()
}

/// A verified Publication authorizes repair that skips unavailable replicas.
/// This includes historical replay and a newly validated degraded-recovery
/// publication; present-but-invalid identities are never ignored.
pub(crate) fn repair_historical_names(
    config: &ValidatedConfig,
    resources: &Arc<DiskResources>,
    previous: &State,
    next: &State,
) -> Result<()> {
    let projection = ProjectionTransition::new(previous, next, None)?;
    prepare_names_inner(config, resources, &projection, MAX_METADATA, true)?.apply()
}

pub(crate) fn prepare_names(
    config: &ValidatedConfig,
    resources: &Arc<DiskResources>,
    projection: &ProjectionTransition<'_>,
    maximum: usize,
) -> Result<PreparedNames> {
    prepare_names_inner(config, resources, projection, maximum, false)
}

fn projection_locations(
    previous: StateProjection<'_>,
    next: StateProjection<'_>,
    file: &str,
) -> BTreeSet<(String, String)> {
    let mut locations = BTreeSet::new();
    for view in [previous, next] {
        for key in view.owners().versions(file) {
            let version = &view.state().versions[key];
            let placement = view
                .state()
                .placements
                .get(&version.version_id)
                .expect("invariant: every admitted version has a placement");
            for slot in &placement.route_slot_ids {
                locations.insert((slot.clone(), version.history_tree_id.clone()));
            }
        }
    }
    locations
}

fn projection_path<'a>(
    state: &'a State,
    file: &str,
    slot: &str,
) -> (Option<&'a String>, Option<&'a str>) {
    let backup = state
        .file_backup_paths
        .get(file)
        .and_then(|paths| paths.get(slot));
    let location = backup.or_else(|| state.file_locations.get(file));
    let name = backup
        .and_then(|path| path.rsplit('/').next())
        .or_else(|| state.file_names.get(file).map(String::as_str));
    (location, name)
}

fn validate_new_projection_path(location: Option<&String>) -> Result<()> {
    // Only the first physical component shares a directory with the marker.
    // Equivalent spellings must not admit a later case-only rename collision.
    if location.is_some_and(|location| {
        location
            .split('/')
            .next()
            .is_some_and(|first| name_policy::lookup_key(first) == NAMESPACE_MARKER)
    }) {
        return Err(Error::new(
            storage_contract::ErrorCode::NameCollision,
            format!("{NAMESPACE_MARKER} is reserved as the first primary or backup path component"),
        ));
    }
    Ok(())
}

/// Pure new-candidate admission, before copying any payload. Use the exact same
/// routed projections as cache preparation, not unused backup bindings.
pub(crate) fn validate_new_projection_paths(projection: &ProjectionTransition<'_>) -> Result<()> {
    let previous = projection.previous();
    let next = projection.next();
    for file in changed_files(previous.state(), next.state()) {
        for (slot, _) in projection_locations(previous, next, &file) {
            validate_new_projection_path(projection_path(next.state(), &file, &slot).0)?;
        }
    }
    Ok(())
}

fn prepare_names_inner(
    config: &ValidatedConfig,
    resources: &Arc<DiskResources>,
    projection: &ProjectionTransition<'_>,
    maximum: usize,
    historical: bool,
) -> Result<PreparedNames> {
    let previous = projection.previous();
    let view = projection.next();
    let next = view.state();
    let mut cache: Option<PreparedNameCache> = None;
    for file in changed_files(previous.state(), next) {
        let locations = projection_locations(previous, view, &file);
        let mut description = None;
        let mut encoded_histories = BTreeMap::new();
        // No merge traversal or tag memo is built until an actual container
        // descriptor needs it. Every cache lives only within this frozen file.
        let mut node_projection = None;
        for (slot, history_id) in locations {
            let disk = config
                .disks
                .iter()
                .find(|disk| disk.slot_id == slot)
                .ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::DiskNotFound,
                        "container placement references an absent slot",
                    )
                })?;
            let held = match resources.available(disk) {
                Some(held) => held,
                None if historical => continue,
                None => resources.get(disk)?,
            };
            let history = disk.root.join("histories").join(&history_id);
            let (physical_location, physical_name) = projection_path(next, &file, &slot);
            // Other new-candidate callers retain the same admission boundary.
            // Published decisions use existing layout checks, not new-name rules.
            if !historical {
                validate_new_projection_path(physical_location)?;
            }
            // Metadata-only references and tombstones never synthesize bodies.
            if !path_directory_exists_at(&held.root, held.root.path(), &history)? {
                continue;
            }
            let Some(current) = container_at(&held.root, &history, &file, &mut |_| Ok(()))? else {
                continue;
            };
            let current_name = current
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| invalid("container directory name is not UTF-8"))?;
            let name = physical_name.unwrap_or(current_name);
            name_policy::validate_name(name)?;
            let description =
                description.get_or_insert_with(|| projection::FileDescription::new(view, &file));
            let nodes = node_projection.get_or_insert_with(|| {
                let events = view
                    .owners()
                    .file_rows(&file)
                    .into_iter()
                    .flat_map(|rows| rows.merges.keys())
                    .map(|key| &next.file_branch_merges[key]);
                crate::business::file_nodes::NodeProjection::from_merge_events(next, events)
            });
            let descriptor = encoded_histories
                .entry(history_id.clone())
                .or_insert_with(|| description.encode_history(&history_id, next, nodes));
            let bytes = encode_metadata(
                &json!({
                    "cache_only": true,
                    "file_id": file,
                    "history_tree_id": history_id,
                    "name": name,
                    "location": physical_location,
                    "primary_location": next.file_locations.get(&file),
                    "backup_policy_id": next.file_backup_policies.get(&file),
                    "metadata_nodes": descriptor.metadata,
                    "branch_merges": descriptor.merges,
                    "branch_aliases": description.aliases,
                    "note": next.file_notes.get(&file).map(String::as_str).unwrap_or(""),
                    "path_contexts": description.contexts,
                    "path_nodes": descriptor.paths,
                    "versions": descriptor.versions,
                    "inherited_branches": descriptor.inherited,
                }),
                maximum,
            )?;
            let staged = match &mut cache {
                Some(staged) => staged,
                empty @ None => empty.insert(PreparedNameCache {
                    items: Vec::new(),
                    file: tempfile::tempfile()?,
                }),
            };
            let offset = staged.file.stream_position()?;
            staged.file.write_all(&bytes)?;
            staged.items.push(PreparedName {
                disk: Arc::clone(held),
                history,
                file: file.clone(),
                location: physical_location.cloned(),
                name: name.to_owned(),
                offset,
                length: bytes.len(),
            });
        }
    }
    // Surface staging allocation/writeback failures before any decision.
    if let Some(staged) = &cache {
        staged.file.sync_data()?;
    }
    Ok(PreparedNames { cache })
}

#[cfg(test)]
#[path = "../../../test/engine/unit/container_storage/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../test/engine/unit/container_storage/name_capture.rs"]
mod name_capture_tests;

#[cfg(test)]
#[path = "../../../test/engine/unit/container_storage/projection_path_tests.rs"]
mod projection_path_tests;

#[cfg(test)]
#[path = "../../../test/engine/unit/container_storage/indexed_projection_tests.rs"]
mod indexed_projection_tests;

fn version_time(time: std::time::SystemTime) -> Result<OffsetDateTime> {
    let seconds = time.duration_since(std::time::UNIX_EPOCH)?.as_secs();
    // Duration seconds may exceed i64; nanoseconds still fit i128 exactly.
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(seconds) * 1_000_000_000)
        .map_err(Error::ClockRange)
}
