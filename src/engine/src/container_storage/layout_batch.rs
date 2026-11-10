//! Physical application of one admitted, complete directory-layout closure.
//!
//! State/slot authorization, the pure namespace plan and the writer fence stay
//! with the caller. A locator or a binding never grants permission to move an
//! entity. New directories already exist in exclusive private preparation
//! names; this executor neither creates nor adopts a user-visible directory.
use super::layout_locator::{
    DirectoryId, DirectoryIntent, EntityInode, FileAddress, FileIntent, HistoryAnchor,
    HistoryRelative, StableRelative, directory_locator_path, file_locator_path, file_staging,
    load_directory, load_file, locate_directory, locate_file, relocate_directory, store_directory,
    store_file, sync_retry_tails,
};
use crate::{Error, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub(super) struct DirectoryMove {
    pub(super) intent: DirectoryIntent,
    pub(super) previous: Option<DirectoryIntent>,
    /// The registered entity whose current physical path is the source parent.
    /// Metadata/preparation parents are represented by None.
    pub(super) previous_parent: Option<DirectoryId>,
    pub(super) next_parent: Option<DirectoryId>,
    pub(super) retired: bool,
}

#[derive(Clone, Debug)]
pub(super) struct FileMove {
    pub(super) intent: FileIntent,
    pub(super) previous: Option<FileIntent>,
}

#[derive(Clone, Debug)]
pub(super) struct FutureFile {
    pub(super) file_id: String,
    pub(super) to: FileAddress,
}

#[derive(Clone, Debug)]
pub(super) struct Batch {
    /// Initialized directory identities permitted to carry a local database.
    pub(super) databases: BTreeMap<DirectoryId, String>,
    /// Every registered directory in the admitted moving namespace, including
    /// unchanged descendants, retired directories and new private preparations.
    pub(super) directories: Vec<DirectoryMove>,
    /// Every existing file container in those namespaces, plus stable sources
    /// that this batch installs there. Owners are String file IDs.
    pub(super) files: Vec<FileMove>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Entity {
    Directory(DirectoryId),
    File(String),
}

struct Prepared {
    directories: BTreeMap<DirectoryId, DirectoryIntent>,
    directory_sources: BTreeMap<DirectoryId, PathBuf>,
    files: BTreeMap<String, FileIntent>,
    file_sources: BTreeMap<String, PathBuf>,
    independent_directories: BTreeSet<DirectoryId>,
    independent_files: BTreeSet<String>,
    placement_order: Vec<DirectoryId>,
    retry_addresses: Vec<HistoryRelative>,
}

fn invalid(message: impl Into<String>) -> Error {
    Error::new(storage_contract::ErrorCode::ContainerLayoutInvalid, message)
}

fn relative(anchor: &HistoryAnchor<'_>, path: &Path) -> Result<HistoryRelative> {
    let path = path
        .strip_prefix(anchor.path())
        .map_err(|_| invalid("batch address is outside its held history"))?;
    HistoryRelative::parse(
        path.to_str()
            .ok_or_else(|| invalid("batch address is not UTF-8"))?,
    )
}

/// Missing ancestors mean absence; an existing link/non-directory is a hard
/// error. DiskRoot retains the mount restriction and safe directory traversal.
fn probe(
    anchor: &HistoryAnchor<'_>,
    path: &Path,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<Option<EntityInode>> {
    if path == anchor.path() {
        anchor.check(anchor.history_id())?;
        return Ok(Some(EntityInode::capture(anchor.root(), path)?));
    }
    let address = relative(anchor, path)?;
    let mut current = anchor.path().to_path_buf();
    let mut inode = None;
    for component in address.as_path().components() {
        observe(0)?;
        current.push(component);
        let Some(metadata) = anchor.root().optional_metadata(&current)? else {
            return Ok(None);
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(invalid(format!(
                "batch address is not a directory: {current:?}"
            )));
        }
        inode = Some(EntityInode::capture(anchor.root(), &current)?);
    }
    Ok(inode)
}

fn record_present(
    anchor: &HistoryAnchor<'_>,
    path: &Path,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<bool> {
    let parent = path
        .parent()
        .expect("invariant: locator records have a parent");
    if probe(anchor, parent, observe)?.is_none() {
        return Ok(false);
    }
    observe(0)?;
    Ok(anchor.root().optional_metadata(path)?.is_some())
}

fn previous_directory(
    anchor: &HistoryAnchor<'_>,
    change: &DirectoryMove,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<Option<DirectoryIntent>> {
    let intent = &change.intent;
    if intent.history_id() != anchor.history_id()
        || change.previous.as_ref().is_some_and(|previous| {
            previous.history_id() != intent.history_id()
                || previous.directory_id() != intent.directory_id()
                || previous.inode() != intent.inode()
        })
    {
        return Err(invalid(
            "directory move changes its admitted history or stable inode",
        ));
    }
    if record_present(
        anchor,
        &directory_locator_path(anchor, intent.directory_id()),
        observe,
    )? {
        let current = load_directory(anchor, intent.directory_id(), observe)?;
        if current.inode() != intent.inode() {
            return Err(invalid(
                "directory move differs from its durable stable inode",
            ));
        }
        Ok(Some(current))
    } else if change.previous.is_some() {
        Err(invalid("previous directory locator is missing"))
    } else {
        Ok(None)
    }
}

fn previous_file(
    anchor: &HistoryAnchor<'_>,
    change: &FileMove,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<Option<FileIntent>> {
    let intent = &change.intent;
    if intent.history_id() != anchor.history_id()
        || change.previous.as_ref().is_some_and(|previous| {
            previous.history_id() != intent.history_id()
                || previous.file_id() != intent.file_id()
                || previous.inode() != intent.inode()
        })
    {
        return Err(invalid(
            "file move changes its admitted history, owner or stable inode",
        ));
    }
    if record_present(
        anchor,
        &file_locator_path(anchor, intent.file_id())?,
        observe,
    )? {
        let current = load_file(anchor, intent.file_id(), observe)?;
        if current.inode() != intent.inode() {
            return Err(invalid("file move differs from its durable stable inode"));
        }
        Ok(Some(current))
    } else if change.previous.is_some() {
        Err(invalid("previous file locator is missing"))
    } else {
        Ok(None)
    }
}

fn file_address(
    anchor: &HistoryAnchor<'_>,
    file_id: &str,
    address: &FileAddress,
    directories: &BTreeMap<DirectoryId, PathBuf>,
) -> Result<PathBuf> {
    match address {
        FileAddress::Stable { relative_path } => Ok(anchor
            .path()
            .join("file-containers")
            .join(file_id)
            .join(relative_path.as_str())),
        FileAddress::Directory {
            directory_id,
            relative_path,
        } => {
            if relative_path.as_path().components().count() != 1 {
                return Err(invalid(
                    "file target requires exactly one container basename",
                ));
            }
            let parent = directories.get(directory_id).ok_or_else(|| {
                invalid("file address refers to a directory outside the supplied closure")
            })?;
            Ok(parent.join(relative_path.as_path()))
        }
    }
}

fn file_owner(
    anchor: &HistoryAnchor<'_>,
    path: &Path,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<Option<String>> {
    let marker = path.join(super::OWNER_MARKER);
    observe(0)?;
    let Some(metadata) = anchor.root().optional_metadata(&marker)? else {
        return Ok(None);
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.nlink() != 1 {
        return Err(invalid(
            "file owner marker is not one ordinary single-link file",
        ));
    }
    let bytes = anchor.root().read_bytes_observed(&marker, 1024, observe)?;
    let owner: String = storage_contract::decode_strict(&bytes)?;
    storage_contract::validate_id(&owner, "file")?;
    Ok(Some(owner))
}

fn locate_input_file(
    anchor: &HistoryAnchor<'_>,
    intent: &FileIntent,
    current: Option<&FileIntent>,
    directories: &BTreeMap<DirectoryId, PathBuf>,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<PathBuf> {
    let mut candidates = BTreeSet::from([anchor
        .path()
        .join(file_staging(intent.file_id())?.as_path())]);
    for record in [Some(intent), current].into_iter().flatten() {
        for address in [record.from(), record.to()] {
            candidates.insert(file_address(
                anchor,
                intent.file_id(),
                address,
                directories,
            )?);
        }
    }
    let mut selected = None;
    for path in candidates {
        let Some(inode) = probe(anchor, &path, observe)? else {
            continue;
        };
        let owner = file_owner(anchor, &path, observe)?;
        if owner.as_deref() == Some(intent.file_id()) {
            if inode != intent.inode() {
                return Err(invalid(
                    "file owner appears with a different container inode",
                ));
            }
            super::reject_unsupported_container(anchor.root(), &path)?;
            if probe(anchor, &path.join("versions"), observe)?.is_none() {
                return Err(invalid("file container has no version directory"));
            }
            if selected.replace(path).is_some() {
                return Err(invalid(
                    "file owner resolves to multiple physical containers",
                ));
            }
        } else if inode == intent.inode() {
            return Err(invalid(
                "recorded file inode has no matching String owner marker",
            ));
        }
    }
    selected.ok_or_else(|| invalid("file move has no source with its recorded owner and inode"))
}

fn source_file_address(
    anchor: &HistoryAnchor<'_>,
    intent: &FileIntent,
    source: &Path,
    source_ids: &BTreeMap<PathBuf, DirectoryId>,
) -> Result<FileAddress> {
    if source
        == anchor
            .path()
            .join(file_staging(intent.file_id())?.as_path())
    {
        // The fixed staging candidate is independent of either directory root.
        return Ok(intent.from().clone());
    }
    if let Some(directory_id) = source.parent().and_then(|parent| source_ids.get(parent)) {
        let basename = source
            .file_name()
            .expect("invariant: a located container has a basename");
        return Ok(FileAddress::Directory {
            directory_id: directory_id.clone(),
            relative_path: HistoryRelative::parse(
                basename
                    .to_str()
                    .expect("invariant: admitted container basenames are UTF-8"),
            )?,
        });
    }
    let stable = anchor.path().join("file-containers").join(intent.file_id());
    let path = source.strip_prefix(&stable).map_err(|_| {
        invalid("file source has no registered physical parent or stable file root")
    })?;
    Ok(FileAddress::Stable {
        relative_path: StableRelative::parse(
            path.to_str()
                .expect("invariant: admitted stable addresses are UTF-8"),
        )?,
    })
}

fn directory_changes(batch: &Batch) -> Result<BTreeMap<DirectoryId, &DirectoryMove>> {
    let mut changes = BTreeMap::new();
    for change in &batch.directories {
        if changes
            .insert(change.intent.directory_id().clone(), change)
            .is_some()
        {
            return Err(invalid("batch repeats a directory identity"));
        }
    }
    Ok(changes)
}

fn directory_geometry(
    changes: &BTreeMap<DirectoryId, &DirectoryMove>,
    sources: &BTreeMap<DirectoryId, PathBuf>,
) -> Result<Vec<DirectoryId>> {
    let by_path: BTreeMap<_, _> = sources
        .iter()
        .map(|(id, path)| (path.clone(), id.clone()))
        .collect();
    if by_path.len() != sources.len() {
        return Err(invalid(
            "multiple directory identities share one physical source",
        ));
    }
    let mut children: BTreeMap<DirectoryId, Vec<DirectoryId>> = BTreeMap::new();
    let mut ready = BTreeSet::new();
    let mut active_count = 0;
    for (id, change) in changes {
        let actual_parent = sources[id].parent().and_then(|parent| by_path.get(parent));
        if actual_parent != change.previous_parent.as_ref() {
            return Err(invalid(
                "directory source parent differs from its admitted physical parent",
            ));
        }
        if change.retired {
            if change.next_parent.is_some() || change.intent.to() != change.intent.staging() {
                return Err(invalid(
                    "retired directory must remain at its own fixed metadata staging name",
                ));
            }
        } else if let Some(parent_id) = &change.next_parent {
            let parent = changes.get(parent_id).ok_or_else(|| {
                invalid("directory target parent is outside the supplied closure")
            })?;
            if parent.retired
                || change.intent.to().as_path().parent() != Some(parent.intent.to().as_path())
            {
                return Err(invalid(
                    "directory target is not directly below its active next parent",
                ));
            }
            children
                .entry(parent_id.clone())
                .or_default()
                .push(id.clone());
        } else if change.intent.to().as_str() != format!("directory-repositories/{}", id.as_str()) {
            return Err(invalid(
                "top-level directory target is not its fixed repository-root address",
            ));
        } else {
            ready.insert(id.clone());
        }
        if !change.retired {
            active_count += 1;
        }
    }
    let mut order = Vec::with_capacity(active_count);
    while let Some(id) = ready.pop_first() {
        if let Some(next) = children.remove(&id) {
            ready.extend(next);
        }
        order.push(id);
    }
    if order.len() != active_count {
        return Err(invalid("directory targets contain a physical parent cycle"));
    }
    Ok(order)
}

fn entity_locations(prepared: &Prepared) -> Result<BTreeMap<PathBuf, (Entity, EntityInode)>> {
    let mut locations = BTreeMap::new();
    let mut inodes = BTreeSet::new();
    for (entity, path, inode) in prepared
        .directory_sources
        .iter()
        .map(|(id, path)| {
            (
                Entity::Directory(id.clone()),
                path,
                prepared.directories[id].inode(),
            )
        })
        .chain(
            prepared
                .file_sources
                .iter()
                .map(|(id, path)| (Entity::File(id.clone()), path, prepared.files[id].inode())),
        )
    {
        let identity = inode.identity();
        if locations.insert(path.clone(), (entity, inode)).is_some()
            || !inodes.insert((identity.device, identity.inode))
        {
            return Err(invalid(
                "batch assigns multiple identities to one physical entity",
            ));
        }
    }
    for file in prepared.file_sources.values() {
        if locations
            .range(file.clone()..)
            .nth(1)
            .is_some_and(|(path, _)| path.starts_with(file))
        {
            return Err(invalid(
                "file container internals cannot be separate layout entities",
            ));
        }
    }
    Ok(locations)
}

fn outgoing(
    prepared: &Prepared,
    changes: &BTreeMap<DirectoryId, &DirectoryMove>,
    entity: &Entity,
) -> bool {
    match entity {
        Entity::Directory(id) => {
            prepared.independent_directories.contains(id)
                && (!changes[id].retired
                    || prepared.directories[id].from() != prepared.directories[id].staging())
        }
        Entity::File(id) => prepared.independent_files.contains(id),
    }
}

fn require_target(
    anchor: &HistoryAnchor<'_>,
    path: &Path,
    owner: Option<&Entity>,
    locations: &BTreeMap<PathBuf, (Entity, EntityInode)>,
    may_vacate: &BTreeSet<Entity>,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<()> {
    if let Some((ancestor, (Entity::File(_), _))) =
        locations.range(..=path.to_path_buf()).next_back()
        && ancestor != path
        && path.starts_with(ancestor)
    {
        return Err(invalid("layout target is inside an atomic file container"));
    }
    let parent = path
        .parent()
        .expect("invariant: a layout target has a parent");
    if probe(anchor, parent, observe)?.is_none() {
        return Err(invalid(
            "layout target has no existing controlled physical parent",
        ));
    }
    let Some(inode) = probe(anchor, path, observe)? else {
        return Ok(());
    };
    let Some((occupant, recorded_inode)) = locations.get(path) else {
        return Err(invalid(format!(
            "layout target has an unregistered occupant: {path:?}"
        )));
    };
    if inode != *recorded_inode || (owner != Some(occupant) && !may_vacate.contains(occupant)) {
        return Err(invalid(
            "layout target is not itself or a verified outgoing batch entity",
        ));
    }
    Ok(())
}

fn inventory(
    anchor: &HistoryAnchor<'_>,
    prepared: &Prepared,
    locations: &BTreeMap<PathBuf, (Entity, EntityInode)>,
    databases: &BTreeMap<DirectoryId, String>,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<()> {
    let mut roots: BTreeSet<_> = prepared.directory_sources.values().cloned().collect();
    let repositories = anchor.path().join("directory-repositories");
    if probe(anchor, &repositories, observe)?.is_some() {
        roots.insert(repositories);
    }
    let mut database_bytes = 0usize;
    for root in roots {
        for entry in anchor.root().entries(&root)? {
            observe(0)?;
            let path = root.join(entry?.as_path());
            if path
                .file_name()
                .is_some_and(|name| name == super::directory_database::NAME)
            {
                let directory = prepared
                    .directory_sources
                    .iter()
                    .find(|(_, source)| **source == root)
                    .and_then(|(id, _)| databases.get(id).map(|repository| (id, repository)))
                    .ok_or_else(|| invalid("database appears outside an initialized directory"))?;
                database_bytes += super::directory_database::verify_at(
                    anchor.root(),
                    &path,
                    directory.1,
                    directory.0.as_str(),
                )?;
                if database_bytes > super::directory_database::MAX_TOTAL_BYTES {
                    return Err(crate::Error::new(
                        storage_contract::ErrorCode::ResourceLimit,
                        "directory database inventory exceeds its byte budget",
                    ));
                }
                continue;
            }
            let Some((_, inode)) = locations.get(&path) else {
                return Err(invalid(format!(
                    "managed directory contains an unregistered entry: {path:?}"
                )));
            };
            if probe(anchor, &path, observe)? != Some(*inode) {
                return Err(invalid(
                    "managed directory entry changed its registered inode",
                ));
            }
        }
    }
    Ok(())
}

fn append_file_addresses(
    anchor: &HistoryAnchor<'_>,
    intent: &FileIntent,
    directories: &BTreeMap<DirectoryId, PathBuf>,
    addresses: &mut Vec<HistoryRelative>,
) -> Result<()> {
    addresses.push(file_staging(intent.file_id())?);
    for address in [intent.from(), intent.to()] {
        addresses.push(relative(
            anchor,
            &file_address(anchor, intent.file_id(), address, directories)?,
        )?);
    }
    Ok(())
}

fn prepare_directories(
    anchor: &HistoryAnchor<'_>,
    batch: &Batch,
    prepared: &mut Prepared,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<()> {
    for change in &batch.directories {
        let current = previous_directory(anchor, change, observe)?;
        let source = locate_directory(anchor, current.as_ref().unwrap_or(&change.intent), observe)?;
        // Final goals belong to the admitted batch, not the locating cache.
        // A future descendant can still be inside an outgoing file container.
        // Persist only the current source until relocate_directory records the
        // precise source/destination pair immediately before its physical move.
        let from = relative(anchor, &source)?;
        let next = DirectoryIntent::new(
            anchor.history_id(),
            change.intent.directory_id().clone(),
            change.intent.binding().clone(),
            change.intent.inode(),
            from.clone(),
            from,
        )?;
        if let Some(current) = &current
            && change.previous.as_ref() != Some(current)
            && *current != next
        {
            return Err(invalid(
                "directory replacement differs from its exact admitted previous record",
            ));
        }
        for intent in [Some(&change.intent), current.as_ref()]
            .into_iter()
            .flatten()
        {
            prepared.retry_addresses.extend([
                intent.from().clone(),
                intent.staging().clone(),
                intent.to().clone(),
            ]);
        }
        let id = next.directory_id().clone();
        if change.retired
            || change.previous_parent != change.next_parent
            || source.file_name() != change.intent.to().as_path().file_name()
            || (change.next_parent.is_none()
                && source != anchor.path().join(change.intent.to().as_path()))
        {
            prepared.independent_directories.insert(id.clone());
        }
        prepared.directory_sources.insert(id.clone(), source);
        prepared.directories.insert(id, next);
    }
    Ok(())
}

fn prepare_files(
    anchor: &HistoryAnchor<'_>,
    batch: &Batch,
    prepared: &mut Prepared,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<()> {
    let source_ids: BTreeMap<_, _> = prepared
        .directory_sources
        .iter()
        .map(|(id, path)| (path.clone(), id.clone()))
        .collect();
    for change in &batch.files {
        let current = previous_file(anchor, change, observe)?;
        let source = locate_input_file(
            anchor,
            &change.intent,
            current.as_ref(),
            &prepared.directory_sources,
            observe,
        )?;
        let from = source_file_address(anchor, &change.intent, &source, &source_ids)?;
        let next = FileIntent::new(
            anchor.history_id(),
            change.intent.file_id(),
            change.intent.binding().clone(),
            change.intent.inode(),
            from,
            change.intent.to().clone(),
        )?;
        if let Some(current) = &current
            && change.previous.as_ref() != Some(current)
            && *current != next
        {
            return Err(invalid(
                "file replacement differs from its exact admitted previous record",
            ));
        }
        for intent in [Some(&change.intent), current.as_ref()]
            .into_iter()
            .flatten()
        {
            append_file_addresses(
                anchor,
                intent,
                &prepared.directory_sources,
                &mut prepared.retry_addresses,
            )?;
        }
        if next.from() != next.to()
            || source == anchor.path().join(file_staging(next.file_id())?.as_path())
        {
            prepared.independent_files.insert(next.file_id().to_owned());
        }
        let id = next.file_id().to_owned();
        if prepared.files.insert(id.clone(), next).is_some() {
            return Err(invalid("batch repeats a file identity"));
        }
        prepared.file_sources.insert(id, source);
    }
    Ok(())
}

fn targets(
    anchor: &HistoryAnchor<'_>,
    prepared: &Prepared,
    changes: &BTreeMap<DirectoryId, &DirectoryMove>,
    future: &[FutureFile],
    locations: &BTreeMap<PathBuf, (Entity, EntityInode)>,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<()> {
    let final_directories: BTreeMap<_, _> = prepared
        .directories
        .keys()
        .map(|id| {
            (
                id.clone(),
                anchor.path().join(changes[id].intent.to().as_path()),
            )
        })
        .collect();
    let mut finals = BTreeMap::new();
    let mut probes = Vec::new();
    let mut stages = Vec::new();
    let may_vacate: BTreeSet<_> = locations
        .values()
        .filter(|(entity, _)| outgoing(prepared, changes, entity))
        .map(|(entity, _)| entity.clone())
        .collect();
    for (id, intent) in &prepared.directories {
        let entity = Entity::Directory(id.clone());
        let final_path = &final_directories[id];
        relative(anchor, final_path)?;
        if finals.insert(final_path.clone(), entity.clone()).is_some() {
            return Err(invalid("batch repeats a final physical address"));
        }
        let path = match &changes[id].next_parent {
            Some(parent) => prepared.directory_sources[parent].join(
                changes[id]
                    .intent
                    .to()
                    .as_path()
                    .file_name()
                    .expect("invariant: directory targets have a basename"),
            ),
            None => final_path.clone(),
        };
        probes.push((path, entity.clone()));
        stages.push((anchor.path().join(intent.staging().as_path()), entity));
    }
    let mut file_ids: BTreeSet<_> = prepared.files.keys().cloned().collect();
    for (file_id, to, concrete) in prepared
        .files
        .values()
        .map(|intent| (intent.file_id(), intent.to(), true))
        .chain(
            future
                .iter()
                .map(|file| (file.file_id.as_str(), &file.to, false)),
        )
    {
        storage_contract::validate_id(file_id, "file")?;
        if !concrete && !file_ids.insert(file_id.to_owned()) {
            return Err(invalid(
                "future file repeats an existing or future file identity",
            ));
        }
        if let FileAddress::Directory { directory_id, .. } = to
            && changes
                .get(directory_id)
                .is_none_or(|change| change.retired)
        {
            return Err(invalid("file target parent is missing or retired"));
        }
        let entity = Entity::File(file_id.to_owned());
        let final_path = file_address(anchor, file_id, to, &final_directories)?;
        relative(anchor, &final_path)?;
        if finals.insert(final_path, entity.clone()).is_some() {
            return Err(invalid("batch repeats a final physical address"));
        }
        probes.push((
            file_address(anchor, file_id, to, &prepared.directory_sources)?,
            entity.clone(),
        ));
        if concrete {
            stages.push((anchor.path().join(file_staging(file_id)?.as_path()), entity));
        }
    }
    for (path, entity) in &finals {
        if matches!(entity, Entity::File(_))
            && finals
                .range(path.clone()..)
                .nth(1)
                .is_some_and(|(other, _)| other.starts_with(path))
        {
            return Err(invalid(
                "a final file container cannot contain another layout entity",
            ));
        }
    }
    for (path, entity) in probes {
        require_target(
            anchor,
            &path,
            Some(&entity),
            locations,
            &may_vacate,
            observe,
        )?;
    }
    for (path, entity) in stages {
        require_target(
            anchor,
            &path,
            Some(&entity),
            locations,
            &BTreeSet::new(),
            observe,
        )?;
    }
    Ok(())
}

fn prepare(
    anchor: &HistoryAnchor<'_>,
    batch: &Batch,
    future: &[FutureFile],
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<Prepared> {
    anchor.check(anchor.history_id())?;
    let changes = directory_changes(batch)?;
    let mut prepared = Prepared {
        directories: BTreeMap::new(),
        directory_sources: BTreeMap::new(),
        files: BTreeMap::new(),
        file_sources: BTreeMap::new(),
        independent_directories: BTreeSet::new(),
        independent_files: BTreeSet::new(),
        placement_order: Vec::new(),
        retry_addresses: Vec::new(),
    };
    prepare_directories(anchor, batch, &mut prepared, observe)?;
    prepared.placement_order = directory_geometry(&changes, &prepared.directory_sources)?;
    prepare_files(anchor, batch, &mut prepared, observe)?;
    let locations = entity_locations(&prepared)?;
    targets(anchor, &prepared, &changes, future, &locations, observe)?;
    inventory(anchor, &prepared, &locations, &batch.databases, observe)?;
    anchor.check(anchor.history_id())?;
    Ok(prepared)
}

/// Read-only admission before ACCEPT/Finalize. Future files have no managed
/// source yet; apply requires their concrete inode- and owner-bound FileMove.
/// The caller supplies the complete State/inventory closure and retains its
/// authorization and writer fence throughout this call and subsequent apply.
pub(super) fn preflight(
    anchor: &HistoryAnchor<'_>,
    batch: &Batch,
    future: &[FutureFile],
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<()> {
    prepare(anchor, batch, future, observe).map(|_| ())
}

/// A directory's old descendants may now lie below a file that took its name.
/// Their old namespace did not move separately. Sync the current container's
/// parent name without interpreting that container's body as directory paths.
/// Resolve these boundaries again from owner/inode-bound records under the
/// retained fence; a cached or merely serialized file path is insufficient.
fn sync_batch_tails(
    anchor: &HistoryAnchor<'_>,
    addresses: &[HistoryRelative],
    files: &BTreeMap<String, FileIntent>,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<()> {
    let mut file_roots = BTreeSet::new();
    for intent in files.values() {
        file_roots.insert(locate_file(anchor, intent, observe)?);
    }
    let mut bounded = BTreeSet::new();
    for address in addresses {
        let path = anchor.path().join(address.as_path());
        let boundary = file_roots
            .range(..=path.clone())
            .next_back()
            .filter(|root| path.starts_with(root));
        bounded.insert(match boundary {
            Some(root) => relative(anchor, root)?,
            None => address.clone(),
        });
    }
    sync_retry_tails(anchor, &bounded.into_iter().collect::<Vec<_>>(), observe)
}

fn persist(
    anchor: &HistoryAnchor<'_>,
    batch: &Batch,
    prepared: &Prepared,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<()> {
    for change in &batch.directories {
        observe(0)?;
        store_directory(
            anchor,
            &prepared.directories[change.intent.directory_id()],
            change.previous.as_ref(),
        )?;
    }
    for change in &batch.files {
        observe(0)?;
        store_file(
            anchor,
            &prepared.files[change.intent.file_id()],
            change.previous.as_ref(),
        )?;
    }
    for (id, intent) in &prepared.directories {
        if locate_directory(anchor, intent, observe)? != prepared.directory_sources[id] {
            return Err(invalid(
                "directory source changed after batch intents were stored",
            ));
        }
    }
    for (id, intent) in &prepared.files {
        if locate_file(anchor, intent, observe)? != prepared.file_sources[id] {
            return Err(invalid(
                "file source changed after batch intents were stored",
            ));
        }
    }
    sync_batch_tails(anchor, &prepared.retry_addresses, &prepared.files, observe)
}

fn move_file(
    anchor: &HistoryAnchor<'_>,
    intent: &FileIntent,
    destination: &Path,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<()> {
    let source = locate_file(anchor, intent, observe)?;
    let tails = [relative(anchor, &source)?, relative(anchor, destination)?];
    observe(0)?;
    anchor.check(anchor.history_id())?;
    if source != destination {
        if file_owner(anchor, &source, observe)?.as_deref() != Some(intent.file_id())
            || !intent.inode().matches(anchor.root(), &source)?
        {
            return Err(invalid(
                "file source owner or inode changed immediately before rename",
            ));
        }
        anchor
            .root()
            .rename_noreplace_synced(&source, destination)?;
    }
    sync_retry_tails(anchor, &tails, observe)
}

/// Persist an inode-bound locator for every admitted entity before the first
/// physical rename. Final goals remain in the State-derived batch; each actual
/// directory relocation persists its precise source/destination pair first.
/// Every error, including observer/store/synchronization uncertainty, stops the
/// batch. The caller reloads exact durable records before any later retry; this
/// operation never rolls back, deletes an occupant, or retries an uncertain I/O.
pub(super) fn apply(
    anchor: &HistoryAnchor<'_>,
    batch: &Batch,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<()> {
    let mut prepared = prepare(anchor, batch, &[], observe)?;
    let final_targets: BTreeMap<_, _> = batch
        .directories
        .iter()
        .map(|change| {
            (
                change.intent.directory_id().clone(),
                change.intent.to().clone(),
            )
        })
        .collect();
    persist(anchor, batch, &prepared, observe)?;
    for id in &prepared.independent_files {
        move_file(
            anchor,
            &prepared.files[id],
            &anchor.path().join(file_staging(id)?.as_path()),
            observe,
        )?;
    }
    let mut evacuate: Vec<_> = prepared.independent_directories.iter().cloned().collect();
    evacuate.sort_by(|left, right| {
        prepared.directory_sources[right]
            .components()
            .count()
            .cmp(&prepared.directory_sources[left].components().count())
            .then_with(|| left.cmp(right))
    });
    for id in evacuate {
        let staging = prepared.directories[&id].staging().clone();
        relocate_directory(anchor, &mut prepared.directories, &id, staging, observe)?;
    }
    for id in &prepared.placement_order {
        if prepared.independent_directories.contains(id) {
            relocate_directory(
                anchor,
                &mut prepared.directories,
                id,
                final_targets[id].clone(),
                observe,
            )?;
        }
    }
    let mut current_directories = BTreeMap::new();
    for (id, intent) in &prepared.directories {
        let current = locate_directory(anchor, intent, observe)?;
        if current != anchor.path().join(final_targets[id].as_path()) {
            return Err(invalid(
                "directory did not reach its admitted final physical address",
            ));
        }
        current_directories.insert(id.clone(), current);
    }
    for intent in prepared.files.values_mut() {
        let target = file_address(anchor, intent.file_id(), intent.to(), &current_directories)?;
        move_file(anchor, intent, &target, observe)?;
        let settled = FileIntent::new(
            anchor.history_id(),
            intent.file_id(),
            intent.binding().clone(),
            intent.inode(),
            intent.to().clone(),
            intent.to().clone(),
        )?;
        observe(0)?;
        store_file(anchor, &settled, Some(intent))?;
        *intent = settled;
    }
    sync_batch_tails(anchor, &prepared.retry_addresses, &prepared.files, observe)
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/container_storage/layout_batch_tests.rs"]
mod tests;
