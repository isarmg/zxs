//! Ordinary version files in one renameable directory per stable file identity.
use crate::{disk::*, *};
use date_time::{OffsetDateTime, format_description::well_known::Rfc3339};
use json_codec::json;
use platform_io::fs_safety::linux::{MountPolicy, OpenAt2Root};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Seek, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
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
    if version.content_hash.len() != 64
        || !version
            .content_hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid("version content digest is invalid"));
    }
    let name = &version.storage_name;
    sequence(name)?;
    let digest = name
        .splitn(3, '_')
        .nth(2)
        .and_then(|value| value.split('.').next())
        .unwrap_or("");
    if digest != &version.content_hash[..8] {
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
        validate_version(version)?;
        maximum = maximum.max(sequence(&version.storage_name)?);
    }
    let number = maximum.checked_add(1).ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::ResourceLimit,
            "file version sequence exhausted",
        )
    })?;
    let now = OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .expect("invariant: zero nanoseconds is representable");
    let timestamp = now.format(&Rfc3339)?;
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

fn marker_matches(path: &Path, file_id: &str) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            return Ok(false);
        }
        Ok(_) => {}
    }
    if !file_exists(path)? || read_json::<String>(path)? != file_id {
        return Err(invalid(
            "container location marker differs from its stable file identity",
        ));
    }
    Ok(true)
}

// Unsupported on-disk layouts are never migrated or cleaned automatically.
// Inspect metadata only; preserve all old bytes, including empty placeholders.
fn reject_unsupported_container(named: &Path) -> Result<()> {
    for name in ["pending-branches", "legacy"] {
        match fs::symlink_metadata(named.join(name)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
            Ok(_) => {
                return Err(Error::new(
                    storage_contract::ErrorCode::FormatUnsupported,
                    "legacy container storage is not supported",
                ));
            }
        }
    }
    Ok(())
}

fn named_containers(
    root: &Path,
    file_id: &str,
    depth: usize,
    selected: &mut Option<PathBuf>,
) -> Result<()> {
    if depth > 33 {
        return Err(invalid("container namespace exceeds the path depth limit"));
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if depth == 0 && entry.file_name() == NAMESPACE_MARKER {
            if !marker_matches(&entry.path(), file_id)? {
                return Err(invalid("container namespace marker is missing"));
            }
            continue;
        }
        if !entry.file_type()?.is_dir() || entry.file_type()?.is_symlink() {
            return Err(invalid(
                "container namespace contains a non-directory or symbolic link",
            ));
        }
        let name = entry.file_name().into_string().map_err(|name| {
            Error::PathEncoding(name).context(
                ErrorCode::ContainerLayoutInvalid,
                "container directory name is not UTF-8",
            )
        })?;
        name_policy::validate_name(&name)?;
        let path = entry.path();
        existing_private_dir(&path)?;
        if marker_matches(&path.join(OWNER_MARKER), file_id)? {
            reject_unsupported_container(&path)?;
            if !directory_exists(&path.join("versions"))? {
                return Err(invalid("owned container has no version directory"));
            }
            if selected.replace(path).is_some() {
                return Err(Error::new(
                    storage_contract::ErrorCode::ContainerAmbiguous,
                    "stable file identity has more than one named container",
                ));
            }
        } else {
            named_containers(&path, file_id, depth + 1, selected)?;
        }
    }
    Ok(())
}

fn container(history: &Path, file_id: &str) -> Result<Option<PathBuf>> {
    validate_id(file_id, "file")?;
    if !directory_exists(history)? {
        return Ok(None);
    }
    let root = history.join("file-containers");
    if !directory_exists(&root)? {
        return Ok(None);
    }
    let root = root.join(file_id);
    if !directory_exists(&root)? {
        return Ok(None);
    }
    let mut selected = None;
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() || entry.file_type()?.is_symlink() {
            return Err(invalid(
                "stable file directory contains a non-directory or symbolic link",
            ));
        }
        let name = entry.file_name().into_string().map_err(|name| {
            Error::PathEncoding(name).context(
                ErrorCode::ContainerLayoutInvalid,
                "container directory name is not UTF-8",
            )
        })?;
        name_policy::validate_name(&name)?;
        existing_private_dir(&entry.path())?;
        if name == "namespace" && marker_matches(&entry.path().join(NAMESPACE_MARKER), file_id)? {
            // The stable identity is the lookup key; logical aliases never
            // choose the physical location. Empty source parents left by an
            // interrupted move are harmless, but two payload owners fail closed.
            named_containers(&entry.path(), file_id, 0, &mut selected)?;
            continue;
        }
        reject_unsupported_container(&entry.path())?;
        if selected.replace(entry.path()).is_some() {
            return Err(Error::new(
                storage_contract::ErrorCode::ContainerAmbiguous,
                "stable file identity has more than one named container",
            ));
        }
    }
    Ok(selected)
}

fn file_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
        Ok(metadata)
            if metadata.is_file()
                && !metadata.file_type().is_symlink()
                && metadata.nlink() == 1 =>
        {
            open_regular(path)?;
            Ok(true)
        }
        Ok(_) => Err(invalid("version body must be one ordinary unlinked file")),
    }
}

/// Missing bodies return their expected path so repair can retain its existing
/// MISSING classification. Only malformed or ambiguous layout fails closed.
pub(crate) fn content_path(directory: &Path, version: &FileVersion) -> Result<PathBuf> {
    validate_version(version)?;
    let prepared = directory.join("content").join(&version.creation_name);
    let name = &version.storage_name;
    let history = history_root(directory)?;
    let named = container(history, &version.file_id)?.unwrap_or_else(|| {
        history
            .join("file-containers")
            .join(&version.file_id)
            .join(&version.creation_name)
    });
    let versions = named.join("versions");
    let target = versions.join(name);
    if directory_exists(&versions)? && file_exists(&target)? {
        return Ok(target);
    }
    // Current preparation and offline packages hold the verified body beside
    // its immutable metadata until installation moves it into the container.
    if file_exists(&prepared)? {
        return Ok(prepared);
    }
    Ok(target)
}

fn held_directory(path: &Path) -> Result<OpenAt2Root> {
    let private = existing_private_dir(path)?;
    OpenAt2Root::open(private.path(), MountPolicy::SameMount).map_err(|error| {
        let message = error.to_string();
        Error::from(error).context(storage_contract::ErrorCode::UnsafePath, message)
    })
}

fn verify_body(path: &Path, version: &FileVersion) -> Result<fs::File> {
    if !file_exists(path)? {
        return Err(Error::new(
            storage_contract::ErrorCode::ContentMissing,
            "version body is missing",
        ));
    }
    let mut file = open_regular(path)?;
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
    if size != version.logical_size || digest.finalize().to_hex().as_str() != version.content_hash {
        return Err(Error::new(
            storage_contract::ErrorCode::ContentCorrupt,
            "version body content differs",
        ));
    }
    file.sync_all()?;
    Ok(file)
}

pub(crate) enum PreparedContent<'a> {
    Existing(&'a FileVersion),
    Copied(VerifiedCopy<'a>),
}

/// Move a verified prepared body on its existing filesystem. Never hardlink,
/// reflink or duplicate the payload to represent a version or rename.
pub(crate) fn install_version_content(
    directory: &Path,
    content: PreparedContent<'_>,
) -> Result<PathBuf> {
    let (version, copied) = match content {
        PreparedContent::Existing(version) => (version, None),
        PreparedContent::Copied(copied) => {
            let (version, file) = copied.into_parts();
            (version, Some(file))
        }
    };
    validate_version(version)?;
    let source = directory.join("content").join(&version.creation_name);
    let name = &version.storage_name;
    let history = history_root(directory)?;
    existing_private_dir(history)?;
    let stable = history.join("file-containers").join(&version.file_id);
    private_dir(&stable)?;
    let named = match container(history, &version.file_id)? {
        Some(named) => named,
        None => {
            let named = stable.join(&version.creation_name);
            private_dir(&named)?;
            sync_dir(&stable)?;
            named
        }
    };
    let versions = named.join("versions");
    private_dir(&versions)?;
    let target = versions.join(name);
    if file_exists(&target)? {
        verify_body(&target, version)?;
        if file_exists(&source)? {
            let _source = match copied {
                Some(file) => file,
                None => verify_body(&source, version)?,
            };
            let parent = held_directory(source.parent().expect("source parent"))?;
            rustix::fs::unlinkat(
                parent.directory(),
                source.file_name().expect("source name"),
                rustix::fs::AtFlags::empty(),
            )
            .map_err(std::io::Error::from)?;
            parent.directory().sync_all()?;
        }
        // Retrying after an interrupted directory sync must complete durability
        // even when the exclusive move already made the target visible.
        if directory_exists(source.parent().expect("source parent"))? {
            sync_dir(source.parent().expect("source parent"))?;
        }
        sync_dir(&versions)?;
        sync_dir(&named)?;
        sync_dir(&stable)?;
        return Ok(target);
    }
    let _source = match copied {
        Some(file) => file,
        None => verify_body(&source, version)?,
    };
    let from = held_directory(source.parent().expect("source parent"))?;
    let to = held_directory(&versions)?;
    rustix::fs::renameat_with(
        from.directory(),
        source.file_name().expect("source name"),
        to.directory(),
        name.as_str(),
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(std::io::Error::from)?;
    from.directory().sync_all()?;
    to.directory().sync_all()?;
    sync_dir(&named)?;
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
        .map(|repository| {
            branches
                .get(&repository.main_branch_id)
                .expect("invariant: every repository has its main branch")
        })
        .map(|branch| {
            commits
                .get(&branch.head_commit_id)
                .expect("invariant: every branch has its head commit")
        })
        .flat_map(|commit| &commit.entries)
        .filter(|entry| entry.kind == NamespaceEntryKind::File);
    for entry in entries {
        if !file_names.contains_key(&entry.file_id) {
            file_names.insert(entry.file_id.clone(), entry.name().to_owned());
        }
        if !file_locations.contains_key(&entry.file_id) {
            file_locations.insert(entry.file_id.clone(), entry.path().to_owned());
        }
    }
}

fn changed_files(previous: &State, next: &State) -> BTreeSet<String> {
    let mut files = BTreeSet::new();
    for (id, node) in previous
        .file_metadata_nodes
        .iter()
        .chain(&next.file_metadata_nodes)
    {
        if !previous.file_metadata_nodes.contains_key(id)
            || !next.file_metadata_nodes.contains_key(id)
        {
            files.insert(node.file_id.clone());
        }
    }
    for (id, event) in previous
        .file_branch_merges
        .iter()
        .chain(&next.file_branch_merges)
    {
        if !previous.file_branch_merges.contains_key(id)
            || !next.file_branch_merges.contains_key(id)
        {
            files.insert(event.file_id.clone());
        }
    }
    for file in previous
        .file_backup_paths
        .keys()
        .chain(next.file_backup_paths.keys())
    {
        if previous.file_backup_paths.get(file) != next.file_backup_paths.get(file) {
            files.insert(file.clone());
        }
    }
    for file in previous
        .file_backup_policies
        .keys()
        .chain(next.file_backup_policies.keys())
    {
        if previous.file_backup_policies.get(file) != next.file_backup_policies.get(file) {
            files.insert(file.clone());
        }
    }
    for (id, node) in previous.file_path_nodes.iter().chain(&next.file_path_nodes) {
        if !previous.file_path_nodes.contains_key(id) || !next.file_path_nodes.contains_key(id) {
            files.insert(node.file_id.clone());
        }
    }
    for id in previous
        .file_path_notes
        .keys()
        .chain(next.file_path_notes.keys())
    {
        if previous.file_path_notes.get(id) != next.file_path_notes.get(id) {
            if let Some(node) = next
                .file_path_nodes
                .get(id)
                .or_else(|| previous.file_path_nodes.get(id))
            {
                files.insert(node.file_id.clone());
            } else if let Some(node) = next
                .file_metadata_nodes
                .get(id)
                .or_else(|| previous.file_metadata_nodes.get(id))
            {
                files.insert(node.file_id.clone());
            }
        }
    }
    for id in previous
        .file_node_parents
        .keys()
        .chain(next.file_node_parents.keys())
    {
        if previous.file_node_parents.get(id) != next.file_node_parents.get(id)
            && let Some(version) = next.versions.get(id).or_else(|| previous.versions.get(id))
        {
            files.insert(version.file_id.clone());
        }
    }
    for key in previous
        .file_locations
        .keys()
        .chain(next.file_locations.keys())
    {
        if previous.file_locations.get(key) != next.file_locations.get(key) {
            files.insert(key.clone());
        }
    }
    for key in previous.file_names.keys().chain(next.file_names.keys()) {
        if previous.file_names.get(key) != next.file_names.get(key) {
            files.insert(key.clone());
        }
    }
    for key in previous.file_notes.keys().chain(next.file_notes.keys()) {
        if previous.file_notes.get(key) != next.file_notes.get(key) {
            files.insert(key.clone());
        }
    }
    for key in previous
        .file_selections
        .keys()
        .chain(next.file_selections.keys())
    {
        if previous.file_selections.get(key) != next.file_selections.get(key) {
            files.insert(key.clone());
        }
    }
    for key in previous
        .file_contexts
        .keys()
        .chain(next.file_contexts.keys())
    {
        if previous.file_contexts.get(key) != next.file_contexts.get(key) {
            for (state, context) in previous
                .file_contexts
                .get(key)
                .map(|context| (previous, context))
                .into_iter()
                .chain(next.file_contexts.get(key).map(|context| (next, context)))
            {
                if let Some(file) = state
                    .branches
                    .get(&context.branch_id)
                    .and_then(|branch| branch.file_id.as_ref())
                {
                    files.insert(file.clone());
                }
            }
        }
    }
    for key in previous
        .version_notes
        .keys()
        .chain(next.version_notes.keys())
    {
        if previous.version_notes.get(key) != next.version_notes.get(key)
            && let Some(version) = next
                .versions
                .get(key)
                .or_else(|| previous.versions.get(key))
        {
            files.insert(version.file_id.clone());
        }
    }
    for (id, version) in previous.versions.iter().chain(next.versions.iter()) {
        let changed_placement = match (previous.placements.get(id), next.placements.get(id)) {
            (Some(before), Some(after)) => {
                before.route_slot_ids != after.route_slot_ids
                    || before.binding_generations != after.binding_generations
                    || before.primary_slot_id != after.primary_slot_id
                    || before.target_copies != after.target_copies
            }
            (None, None) => false,
            _ => true,
        };
        if !previous.versions.contains_key(id)
            || !next.versions.contains_key(id)
            || changed_placement
            || previous.content_states.get(id) != next.content_states.get(id)
        {
            files.insert(version.file_id.clone());
        }
    }
    // An inherited branch has no new FileVersion. Its reference metadata must
    // still update the cache, including removal after the first owned write.
    for id in previous.branches.keys().chain(next.branches.keys()) {
        let before = previous.branches.get(id);
        let after = next.branches.get(id);
        let unchanged = match (before, after) {
            (Some(before), Some(after)) => {
                before.pending_file_versions == after.pending_file_versions
                    && before.file_node_id == after.file_node_id
                    && before.name == after.name
                    && before.state == after.state
                    && before.history_tree_id == after.history_tree_id
            }
            (None, None) => true,
            _ => false,
        };
        if !unchanged {
            for branch in before.into_iter().chain(after) {
                files.extend(branch.pending_file_versions.keys().cloned());
            }
        }
    }
    files
}

fn synchronize_location(
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
            let parent = held_directory(current.parent().expect("container parent"))?;
            rustix::fs::renameat_with(
                parent.directory(),
                current.file_name().expect("container name"),
                parent.directory(),
                name,
                rustix::fs::RenameFlags::NOREPLACE,
            )
            .map_err(std::io::Error::from)?;
            parent.directory().sync_all()?;
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
    if current == namespace && !marker_matches(&namespace.join(NAMESPACE_MARKER), file_id)? {
        let temporary_name = format!(".zxs-relocate-{}", new_id("req"));
        let parent = held_directory(stable)?;
        rustix::fs::renameat_with(
            parent.directory(),
            "namespace",
            parent.directory(),
            temporary_name.as_str(),
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(std::io::Error::from)?;
        parent.directory().sync_all()?;
        current = stable.join(temporary_name);
    }
    private_dir(&namespace)?;
    if !marker_matches(&namespace.join(NAMESPACE_MARKER), file_id)? {
        write_json(&namespace.join(NAMESPACE_MARKER), &file_id, false)?;
    }
    let target = namespace.join(location);
    if target != current {
        if target.starts_with(&current) || current.starts_with(&target) {
            let temporary_name = format!(".zxs-relocate-{}", new_id("req"));
            let source = held_directory(current.parent().expect("current container parent"))?;
            let destination = held_directory(stable)?;
            rustix::fs::renameat_with(
                source.directory(),
                current.file_name().expect("current container name"),
                destination.directory(),
                temporary_name.as_str(),
                rustix::fs::RenameFlags::NOREPLACE,
            )
            .map_err(std::io::Error::from)?;
            source.directory().sync_all()?;
            destination.directory().sync_all()?;
            current = stable.join(temporary_name);
        }
        if directory_exists(&target)? {
            // Only structural empty parents can be replaced. Validate all of
            // them before removing any; unexpected bytes or links stay intact.
            let mut empty = Vec::new();
            collect_empty_directories(&target, 0, &mut empty)?;
            for directory in empty {
                let parent = held_directory(directory.parent().expect("empty parent"))?;
                rustix::fs::unlinkat(
                    parent.directory(),
                    directory.file_name().expect("empty name"),
                    rustix::fs::AtFlags::REMOVEDIR,
                )
                .map_err(std::io::Error::from)?;
                parent.directory().sync_all()?;
            }
        }
        let destination_parent = target.parent().expect("canonical location parent");
        private_dir(destination_parent)?;
        let mut directory = Some(destination_parent);
        while let Some(parent) = directory {
            sync_dir(parent)?;
            if parent == stable {
                break;
            }
            directory = parent.parent();
        }
        if !marker_matches(&current.join(OWNER_MARKER), file_id)? {
            write_json(&current.join(OWNER_MARKER), &file_id, false)?;
        }
        sync_dir(&current)?;
        let source = held_directory(current.parent().expect("current container parent"))?;
        let destination = held_directory(destination_parent)?;
        rustix::fs::renameat_with(
            source.directory(),
            current.file_name().expect("current container name"),
            destination.directory(),
            target.file_name().expect("target container name"),
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(std::io::Error::from)?;
        source.directory().sync_all()?;
        destination.directory().sync_all()?;
    }
    sync_dir(&namespace)?;
    sync_dir(stable)?;
    if let Some(mut parent) = original_parent {
        while parent != namespace && parent.starts_with(&namespace) && directory_exists(&parent)? {
            if fs::read_dir(&parent)?.next().is_some() {
                break;
            }
            let ancestor = parent.parent().expect("namespace ancestor").to_path_buf();
            let held = held_directory(&ancestor)?;
            rustix::fs::unlinkat(
                held.directory(),
                parent.file_name().expect("empty namespace parent"),
                rustix::fs::AtFlags::REMOVEDIR,
            )
            .map_err(std::io::Error::from)?;
            held.directory().sync_all()?;
            parent = ancestor;
        }
    }
    Ok(target)
}

fn collect_empty_directories(
    root: &Path,
    depth: usize,
    directories: &mut Vec<PathBuf>,
) -> Result<()> {
    if depth > 33 {
        return Err(invalid("container namespace exceeds the path depth limit"));
    }
    existing_private_dir(root)?;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() || entry.file_type()?.is_symlink() {
            return Err(invalid(
                "canonical container target contains unexpected data",
            ));
        }
        collect_empty_directories(&entry.path(), depth + 1, directories)?;
    }
    directories.push(root.to_path_buf());
    Ok(())
}

/// Fully encoded, bounded projections. Preparation never changes published names
/// or caches. Signed version evidence remains separate from this inspection cache.
pub(crate) struct PreparedNames {
    cache: Option<PreparedNameCache>,
    historical: bool,
}
struct PreparedNameCache {
    items: Vec<PreparedName>,
    // Lazily create one anonymous file only when a projection exists. Keeping
    // items with their file makes an unstaged projection unrepresentable.
    file: fs::File,
}
struct PreparedName {
    slot: String,
    history_id: String,
    file: String,
    location: Option<String>,
    name: String,
    offset: u64,
    length: usize,
}
impl PreparedNames {
    pub(crate) fn apply(self, config: &ValidatedConfig) -> Result<()> {
        let Some(mut cache) = self.cache else {
            return Ok(());
        };
        for item in cache.items {
            let disk = config
                .disks
                .iter()
                .find(|disk| disk.slot_id == item.slot)
                .ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::DiskNotFound,
                        "container slot disappeared",
                    )
                })?;
            if self.historical && disk_unavailable(disk)? {
                continue;
            }
            validate_seal(config, disk)?;
            let history = disk.root.join("histories").join(&item.history_id);
            let Some(current) = container(&history, &item.file)? else {
                continue;
            };
            let stable = history.join("file-containers").join(&item.file);
            let named = synchronize_location(
                current,
                &stable,
                &item.file,
                item.location.as_deref(),
                &item.name,
            )?;
            // Seal even a rename made visible by an earlier interrupted attempt.
            sync_dir(&stable)?;
            cache.file.seek(std::io::SeekFrom::Start(item.offset))?;
            let mut bytes = vec![0; item.length];
            cache.file.read_exact(&mut bytes)?;
            write_atomic(&named.join("container.json"), &bytes, true)?;
            sync_dir(&named)?;
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
    prepare_names(config, previous, next, MAX_METADATA)?.apply(config)
}

/// A verified Publication authorizes repair that skips unavailable replicas.
/// This includes historical replay and a newly validated degraded-recovery
/// publication; present-but-invalid identities are never ignored.
pub(crate) fn repair_historical_names(
    config: &ValidatedConfig,
    previous: &State,
    next: &State,
) -> Result<()> {
    prepare_names_inner(config, previous, next, MAX_METADATA, true)?.apply(config)
}

pub(crate) fn prepare_names(
    config: &ValidatedConfig,
    previous: &State,
    next: &State,
    maximum: usize,
) -> Result<PreparedNames> {
    prepare_names_inner(config, previous, next, maximum, false)
}

fn projection_locations(
    previous: &State,
    next: &State,
    file: &str,
) -> Result<BTreeSet<(String, String)>> {
    let mut locations = BTreeSet::new();
    for (state, version) in previous
        .versions
        .values()
        .map(|version| (previous, version))
        .chain(next.versions.values().map(|version| (next, version)))
        .filter(|(_, version)| version.file_id == file)
    {
        let placement = state
            .placements
            .get(&version.version_id)
            .expect("invariant: every admitted version has a placement");
        for slot in &placement.route_slot_ids {
            locations.insert((slot.clone(), version.history_tree_id.clone()));
        }
    }
    Ok(locations)
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
pub(crate) fn validate_new_projection_paths(previous: &State, next: &State) -> Result<()> {
    for file in changed_files(previous, next) {
        for (slot, _) in projection_locations(previous, next, &file)? {
            validate_new_projection_path(projection_path(next, &file, &slot).0)?;
        }
    }
    Ok(())
}

fn prepare_names_inner(
    config: &ValidatedConfig,
    previous: &State,
    next: &State,
    maximum: usize,
    historical: bool,
) -> Result<PreparedNames> {
    let mut cache: Option<PreparedNameCache> = None;
    let mut node_projection = crate::business::file_nodes::NodeProjection::new(next);
    for file in changed_files(previous, next) {
        validate_id(&file, "file")?;
        let versions = next
            .versions
            .values()
            .filter(|version| version.file_id == file)
            .collect::<Vec<_>>();
        let pending = next
            .branches
            .values()
            .filter_map(|branch| {
                branch
                    .pending_file_versions
                    .get(&file)
                    .map(|source| (branch, source))
            })
            .collect::<Vec<_>>();
        let locations = projection_locations(previous, next, &file)?;
        // Replica-independent version records are built once per file/history.
        let mut records_by_history = BTreeMap::new();
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
            if historical && disk_unavailable(disk)? {
                continue;
            }
            let history = disk.root.join("histories").join(&history_id);
            validate_seal(config, disk)?;
            let pending_here = pending
                .iter()
                .copied()
                .filter(|(branch, _)| branch.history_tree_id == history_id)
                .collect::<Vec<_>>();
            let (physical_location, physical_name) = projection_path(next, &file, &slot);
            // Other new-candidate callers retain the same admission boundary.
            // Published decisions use existing layout checks, not new-name rules.
            if !historical {
                validate_new_projection_path(physical_location)?;
            }
            // Metadata-only references and tombstones never synthesize bodies.
            let Some(current) = container(&history, &file)? else {
                continue;
            };
            let current_name = current
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| invalid("container directory name is not UTF-8"))?;
            let name = physical_name.unwrap_or(current_name);
            name_policy::validate_name(name)?;
            let records = records_by_history
                .entry(history_id.clone())
                .or_insert_with(|| {
                    let mut records = versions.iter().filter(|version| version.history_tree_id == history_id).map(|version| {
                        json!({"version_id":version.version_id,"parent_node_ids":node_projection.parents(&version.version_id),"tags":node_projection.tags(&version.version_id),"storage_name":version.storage_name,"created_at":version.created_at,"size":version.logical_size.to_string(),"content_hash":version.content_hash,"note":next.version_notes.get(&version.version_id).map(String::as_str).unwrap_or(""),"content_state":next.content_states.get(&version.version_id)})
                    }).collect::<Vec<_>>();
                    records.sort_by(|a, b| {
                        a["storage_name"]
                            .as_str()
                            .cmp(&b["storage_name"].as_str())
                            .then_with(|| a["version_id"].as_str().cmp(&b["version_id"].as_str()))
                    });
                    records
                });
            let inherited = pending_here
                .iter()
                .map(|(branch, source)| {
                    json!({"branch_id":branch.branch_id,"name":branch.name,"state":branch.state,"source_node_id":branch.file_node_id.as_ref().unwrap_or(source),"source_version_id":source,"version_id":source,"own_version_id":null,"inherited":true,"materialized":false})
                })
                .collect::<Vec<_>>();
            let contexts = next
                .file_contexts
                .iter()
                .filter(|(_, context)| {
                    next.branches
                        .get(&context.branch_id)
                        .and_then(|branch| branch.file_id.as_ref())
                        == Some(&file)
                })
                .collect::<std::collections::BTreeMap<_, _>>();
            let mut nodes = next
                .file_path_nodes
                .values()
                .filter(|node| node.file_id == file && node.history_tree_id == history_id)
                .collect::<Vec<_>>();
            nodes.sort_by(|a, b| {
                a.sequence
                    .cmp(&b.sequence)
                    .then_with(|| a.node_id.cmp(&b.node_id))
            });
            let path_nodes = nodes.into_iter().map(|node| json!({"node":node,"tags":node_projection.tags(&node.node_id),"note":next.file_path_notes.get(&node.node_id).map(String::as_str).unwrap_or("")})).collect::<Vec<_>>();
            let bytes = encode_metadata(
                &json!({"cache_only":true,"file_id":file,"history_tree_id":history_id,"name":name,"location":physical_location,"primary_location":next.file_locations.get(&file),"backup_policy_id":next.file_backup_policies.get(&file),"metadata_nodes":next.file_metadata_nodes.values().filter(|node|node.file_id==file&&node.history_tree_id==history_id).map(|node|json!({"node":node,"note":next.file_path_notes.get(&node.node_id).map(String::as_str).unwrap_or("")})).collect::<Vec<_>>(),"branch_merges":next.file_branch_merges.values().filter(|event|event.file_id==file&&event.history_tree_id==history_id).collect::<Vec<_>>(),"branch_aliases":next.file_branch_aliases.iter().filter(|(source,_)|next.branches.get(*source).is_some_and(|branch|branch.file_id.as_deref()==Some(file.as_str()))).collect::<BTreeMap<_,_>>(),"note":next.file_notes.get(&file).map(String::as_str).unwrap_or(""),"path_contexts":contexts,"path_nodes":path_nodes,"versions":records,"inherited_branches":inherited}),
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
                slot,
                history_id,
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
    Ok(PreparedNames { cache, historical })
}

#[cfg(test)]
#[path = "../../../test/engine/unit/container_storage/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../test/engine/unit/container_storage/projection_path_tests.rs"]
mod projection_path_tests;
