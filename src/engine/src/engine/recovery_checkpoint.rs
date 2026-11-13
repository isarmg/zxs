//! Conservative durable replay checkpoint. Journals remain authoritative.
//!
//! All fixed trusted metadata members must agree on a checkpoint whose tips occur
//! in the fully verified journal prefixes. Covered metadata/proofs and sensitive layout
//! are reverified; unrelated append-only additions permit normal suffix replay.
//! Missing or changed covered dependencies fall back to full repair. This bounds
//! historical state decoding, not verification work or total startup time.
//! Registry-bearing generations require authoritative replay until checkpoints
//! can restore the admitted current namespace, not merely its raw snapshot.
use super::Engine;
use crate::journal::RecordType;
use crate::persist::layout;
use crate::{disk::*, journal::*, *};
use data_serialization::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Component, Path},
};

const DIRECTORY_LAYOUT_MARKER: &str = "metadata/directory-layout.json";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serialization(deny_unknown_fields)]
pub(super) struct Checkpoint {
    context: CheckpointContext,
    pub sequence: u64,
    pub candidate_hash: String,
    pub snapshot_hash: String,
    tips: BTreeMap<String, String>,
    coverage: BTreeMap<String, Vec<CoverageEntry>>,
    version_ids: BTreeSet<String>,
}

/// Persisted equality binding, never a source of membership or I/O authority.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serialization(deny_unknown_fields)]
struct CheckpointContext {
    config_hash: String,
    authority: CheckpointAuthority,
    io_disk_ids: BTreeSet<String>,
    metadata_disk_ids: BTreeSet<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serialization(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum CheckpointAuthority {
    Genesis,
    Generation { hash: String },
}

/// Only a live admitted Engine can choose the scope. Keep its immutable borrow
/// through verification/save so a serialized context cannot widen that scope.
struct CheckpointScope<'a> {
    engine: &'a Engine,
    context: CheckpointContext,
    members: Vec<DiskConfig>,
}

impl<'a> CheckpointScope<'a> {
    fn admit(engine: &'a Engine) -> Result<Option<Self>> {
        // A registry commitment alone cannot reconstruct the masked current
        // entries from a late historical snapshot. Even an already installed
        // view must replay D3 after restart; do not read or write an unusable
        // acceleration file. Successor Add/Abort generations can inherit it.
        if engine.config.fresh_bootstrap().is_some()
            || engine.snapshot_initialization_intent.is_some()
            || engine.membership_fence.is_some()
            || engine.pending_disk_exit.is_some()
            || engine
                .config
                .generation()
                .is_some_and(|generation| generation.registry().is_some())
            || engine
                .config
                .disks
                .iter()
                .any(|disk| engine.config.excludes_io_disk(&disk.disk_id))
        {
            return Ok(None);
        }
        // This runs before the recovered State is loaded. Shared directory
        // layouts are outside this checkpoint's per-file path coverage. Only
        // already admitted roots may prove marker absence; missing resources,
        // inspection failures and every present marker disable the optional
        // cache. Marker bytes, including damaged bytes, never grant I/O or
        // permission to use a checkpoint. The authoritative replay still runs.
        for disk in engine.config.io_disks() {
            let Some(held) = engine.disk_resources.available(disk) else {
                return Ok(None);
            };
            let marker = held.root.path().join(DIRECTORY_LAYOUT_MARKER);
            match held.root.optional_metadata(&marker) {
                Ok(None) => {}
                Ok(Some(_)) | Err(_) => return Ok(None),
            }
        }
        let members = engine.members();
        let authority = match engine.config.generation_hash() {
            None => CheckpointAuthority::Genesis,
            Some(hash) => CheckpointAuthority::Generation { hash: hash.into() },
        };
        Ok(Some(Self {
            engine,
            context: CheckpointContext {
                config_hash: object_hash("replay-config", &engine.config)?,
                authority,
                io_disk_ids: engine
                    .config
                    .io_disks()
                    .map(|disk| disk.disk_id.clone())
                    .collect(),
                // Fixed voters, never a projection of currently available I/O.
                metadata_disk_ids: members.iter().map(|disk| disk.disk_id.clone()).collect(),
            },
            members,
        }))
    }
}

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Envelope {
    checkpoint: Checkpoint,
    hash: String,
}
fn checkpoint_disk<'a>(engine: &'a Engine, disk: &DiskConfig) -> Result<&'a DiskHandle> {
    let held = engine.disk_resources.get(disk)?;
    let stored = decode_seal(&held.root.read_bytes_observed(
        &disk.root.join(layout::DISK_IDENTITY.as_path()),
        MAX_METADATA,
        &mut |_| Ok(()),
    )?)?;
    validate_stored_seal(&stored, &held.seal)?;
    Ok(held)
}

/// Covered absences include missing ancestors, but never links or other I/O failures.
fn metadata(root: &DiskRoot, relative: &Path) -> Result<Option<std::fs::Metadata>> {
    match root.optional_metadata(&root.path().join(relative)) {
        Err(Error::Filesystem(platform_io::fs_safety::Error::Io(source)))
        | Err(Error::Contract(storage_contract::Error::Io { source, .. }))
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(None)
        }
        result => result,
    }
}

fn checkpoint_bytes(mut file: ReadFile, maximum: usize) -> Result<Vec<u8>> {
    if file.metadata()?.len() > maximum as u64 {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "metadata input exceeds limit",
        ));
    }
    let mut output = Vec::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        file.check_running()?;
        let remaining = (maximum + 1 - output.len()).min(buffer.len());
        let count = read_observed_fill(&mut file, &mut buffer[..remaining], &mut |_| Ok(()))?;
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
    // EOF can complete while quiescing retires the last read guard.
    file.check_running()?;
    Ok(output)
}

// Read every repair-relevant byte, not metadata timestamps. Installed named
// version payloads are excluded: recovery reconstructs their publication proof
// without re-verifying the body. Prepared content remains hashed because replay
// must verify it before installation, including after an interrupted move.
fn named_container(root: &DiskRoot, relative: &Path) -> Result<bool> {
    let parts = relative_parts(relative)?;
    if parts.len() < 5 || parts[0] != "histories" || parts[2] != "file-containers" {
        return Ok(false);
    }
    // First installation creates a direct-name container before namespace
    // placement. Namespaced containers must carry their exact file ID.
    if parts.len() == 5 && parts[4] != "namespace" {
        return Ok(true);
    }
    let Some(file) = root.open_optional_read_regular(
        &root.path().join(relative).join(".zxs-container-owner.json"),
    )?
    else {
        return Ok(false);
    };
    let owner: String = decode_strict(&checkpoint_bytes(file, MAX_METADATA)?)?;
    Ok(owner == parts[3])
}
fn payload(root: &DiskRoot, relative: &Path) -> Result<bool> {
    let Some(parent) = relative.parent() else {
        return Ok(false);
    };
    if parent.file_name().is_none_or(|name| name != "versions") {
        return Ok(false);
    }
    let Some(container) = parent.parent() else {
        return Ok(false);
    };
    named_container(root, container)
}
const MAX_CHECKPOINT_BYTES: usize = 16 * 1024 * 1024;
const MAX_COVERAGE_BYTES: usize = 8 * 1024 * 1024;
const MAX_COVERAGE_ENTRIES: usize = 32_768;
pub(super) type Records = BTreeMap<u64, BTreeMap<String, Vec<Receipt>>>;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serialization(deny_unknown_fields)]
struct CoverageEntry {
    path: String,
    value: Covered,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serialization(tag = "kind", deny_unknown_fields)]
enum Covered {
    Directory {
        children: Option<Vec<String>>,
    },
    File {
        length: u64,
        hash: Option<String>,
        links: u64,
    },
    Absent,
}
fn relative_parts(path: &Path) -> Result<Vec<&str>> {
    path.components()
        .map(|part| match part {
            Component::Normal(name) => name
                .to_str()
                .ok_or_else(|| CheckpointError::InvalidPath(path.into()).into()),
            _ => Err(CheckpointError::InvalidPath(path.into()).into()),
        })
        .collect()
}
fn excluded(path: &Path) -> bool {
    path == Path::new("metadata/application-log")
        || path == layout::REPLAY_CHECKPOINT.as_path()
}
fn append_only(root: &DiskRoot, path: &Path) -> Result<bool> {
    let parts = relative_parts(path)?;
    if path.file_name().is_some_and(|name| name == "versions")
        && match path.parent() {
            Some(parent) => named_container(root, parent)?,
            None => false,
        }
    {
        return Ok(true);
    }
    // Each database installation adds a binding and a fresh attempt. Existing
    // attempts retain their exact children and bytes in checkpoint coverage.
    Ok(
        (parts.first() == Some(&"metadata") && parts.get(1) == Some(&"objects"))
            || path == Path::new("metadata/publication-seals")
            || (parts.first() == Some(&"histories")
                && (parts.len() <= 2
                    || (parts.len() == 3 && matches!(parts[2], "versions" | "file-containers"))
                    || ((parts.len() == 4 || parts.len() == 5)
                        && parts[2] == ".zxs-layout"
                        && parts[3] == "database-installs")
                    || (parts.len() == 5
                        && parts[2] == "versions"
                        && parts[4] == "protection-proofs"))),
    )
}
fn children(root: &DiskRoot, path: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut bytes = 0;
    for entry in root.entries(&root.path().join(path))? {
        let name = entry?
            .as_os_str()
            .to_os_string()
            .into_string()
            .map_err(|name| {
                Error::PathEncoding(name).context(
                    ErrorCode::CheckpointInvalid,
                    format!("checkpoint directory name is not UTF-8 at {path:?}"),
                )
            })?;
        if excluded(&path.join(&name)) {
            continue;
        }
        bytes += name.len();
        directory_budget(path, out.len(), bytes)?;
        out.push(name);
    }
    out.sort();
    Ok(out)
}
fn directory_budget(path: &Path, entries: usize, bytes: usize) -> Result<()> {
    if entries >= MAX_COVERAGE_ENTRIES {
        return Err(CheckpointError::DirectoryEntries {
            path: path.into(),
            actual: entries + 1,
            limit: MAX_COVERAGE_ENTRIES,
        }
        .into());
    }
    if bytes > MAX_COVERAGE_BYTES {
        return Err(CheckpointError::DirectoryNameBytes {
            path: path.into(),
            actual: bytes,
            limit: MAX_COVERAGE_BYTES,
        }
        .into());
    }
    Ok(())
}
fn file_hash(root: &DiskRoot, path: &Path) -> Result<String> {
    let mut file = root.open_read_regular(path)?;
    let hash = hash_reader(&mut file)?;
    // Keep the owner until EOF and finalization have both completed.
    file.check_running()?;
    Ok(hash)
}
fn hash_reader(mut file: impl Read) -> Result<String> {
    let mut h = blake3::Hasher::new();
    let mut bytes = [0; 64 * 1024];
    loop {
        let count = file.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        h.update(&bytes[..count]);
    }
    Ok(h.finalize().to_hex().to_string())
}
fn add_entry(out: &mut Vec<CoverageEntry>, entry: CoverageEntry, budget: &mut usize) -> Result<()> {
    let current = *budget;
    let added = canonical_json(&entry)?.len();
    *budget = current
        .checked_add(added)
        .ok_or_else(|| CheckpointError::CoverageBytes {
            current,
            added,
            path: entry.path.clone().into(),
            limit: MAX_COVERAGE_BYTES,
        })?;
    if *budget > MAX_COVERAGE_BYTES {
        return Err(CheckpointError::CoverageBytes {
            current,
            added,
            path: entry.path.into(),
            limit: MAX_COVERAGE_BYTES,
        }
        .into());
    }
    if out.len() >= MAX_COVERAGE_ENTRIES {
        return Err(CheckpointError::CoverageEntries {
            path: entry.path.into(),
            actual: out.len() + 1,
            limit: MAX_COVERAGE_ENTRIES,
        }
        .into());
    }
    out.push(entry);
    Ok(())
}
fn inventory(
    root: &DiskRoot,
    relative: &Path,
    out: &mut Vec<CoverageEntry>,
    budget: &mut usize,
) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let metadata =
        metadata(root, relative)?.ok_or_else(|| std::io::Error::from(rustix::io::Errno::NOENT))?;
    if metadata.file_type().is_symlink() || !(metadata.is_dir() || metadata.is_file()) {
        return Err(CheckpointError::FileType(root.path().join(relative)).into());
    }
    let name = relative
        .to_str()
        .ok_or_else(|| CheckpointError::InvalidPath(relative.to_path_buf()))?
        .to_owned();
    if metadata.is_dir() {
        let names = children(root, relative)?;
        add_entry(
            out,
            CoverageEntry {
                path: name,
                value: Covered::Directory {
                    children: if append_only(root, relative)? {
                        None
                    } else {
                        Some(names.clone())
                    },
                },
            },
            budget,
        )?;
        for child in names {
            inventory(root, &relative.join(child), out, budget)?;
        }
    } else {
        add_entry(
            out,
            CoverageEntry {
                path: name,
                value: Covered::File {
                    length: metadata.len(),
                    links: metadata.nlink(),
                    hash: if payload(root, relative)? {
                        None
                    } else {
                        Some(file_hash(root, &root.path().join(relative))?)
                    },
                },
            },
            budget,
        )?;
    }
    Ok(())
}
fn coverage(
    scope: &CheckpointScope<'_>,
    absent: &BTreeMap<String, BTreeSet<String>>,
) -> Result<BTreeMap<String, Vec<CoverageEntry>>> {
    let engine = scope.engine;
    let mut out = BTreeMap::new();
    let mut budget = 0;
    for d in engine.config.io_disks() {
        let root = &engine.disk_resources.get(d)?.root;
        let mut entries = Vec::new();
        inventory(root, Path::new("metadata"), &mut entries, &mut budget)?;
        // A newly admitted member receives control metadata without creating
        // an empty payload namespace. Cover that exact absence rather than
        // creating it or omitting it from the checkpoint: later appearance
        // must invalidate the shortcut and force authoritative replay.
        let histories = Path::new("histories");
        if metadata(root, histories)?.is_none() {
            add_entry(
                &mut entries,
                CoverageEntry {
                    path: "histories".into(),
                    value: Covered::Absent,
                },
                &mut budget,
            )?;
        } else {
            inventory(root, histories, &mut entries, &mut budget)?;
        }
        for path in absent.get(&d.disk_id).into_iter().flatten() {
            add_entry(
                &mut entries,
                CoverageEntry {
                    path: path.clone(),
                    value: Covered::Absent,
                },
                &mut budget,
            )?;
        }
        out.insert(d.disk_id.clone(), entries);
    }
    Ok(out)
}
fn suffix_rebuilt_containers(
    engine: &Engine,
    checkpoint: &Checkpoint,
    records: &Records,
) -> Result<BTreeSet<(String, String)>> {
    let mut out = BTreeSet::new();
    let mut seen = checkpoint.version_ids.clone();
    let mut previous = Some(checkpoint.snapshot_hash.clone());
    for (sequence, candidates) in records.range((
        std::ops::Bound::Excluded(checkpoint.sequence),
        std::ops::Bound::Unbounded,
    )) {
        let active = candidates
            .iter()
            .filter(|(_, records)| {
                records.iter().any(|receipt| {
                    matches!(
                        receipt.record.record_type,
                        RecordType::Accept | RecordType::Publication
                    )
                })
            })
            .collect::<Vec<_>>();
        if active.len() > 1 {
            return Err(CheckpointError::SuffixConflict(*sequence).into());
        }
        let Some((hash, records)) = active.first() else {
            continue;
        };
        let Some(receipt) = records
            .iter()
            .find(|receipt| receipt.record.record_type == RecordType::Publication)
        else {
            break;
        };
        let disk = engine.disk(&receipt.slot_id)?;
        let root = &engine.disk_resources.get(disk)?.root;
        let c: Candidate = load_object_at(root, disk, "candidate", hash)?;
        engine.verify_candidate_membership(&c)?;
        let p: Publication =
            load_object_at(root, disk, "publication", &receipt.record.object_hash)?;
        if c.pool_id != engine.config.pool_id
            || c.sequence != *sequence
            || c.previous_snapshot != previous
        {
            return Err(CheckpointError::SuffixChain(*sequence).into());
        }
        // Only a continuous, fully sealed suffix may waive checking a derived
        // container cache. Pending/forked decisions never authorize exceptions.
        let sealed = engine.members().iter().all(|d| {
            engine
                .disk_resources
                .get(d)
                .and_then(|held| {
                    held.root.read_json_observed::<PublicationSeal>(
                        &publication_seal_path(d, hash),
                        &mut |_| Ok(()),
                    )
                })
                .and_then(|seal| verify_publication_seal(&c, &p, &seal))
                .is_ok()
        });
        if !sealed {
            break;
        }
        previous = Some(c.snapshot_hash.clone());
        for evidence in &c.versions {
            if seen.insert(evidence.version.version_id.clone()) && p.recovery.is_none() {
                out.insert((
                    evidence.version.history_tree_id.clone(),
                    evidence.version.file_id.clone(),
                ));
            }
        }
    }
    Ok(out)
}
fn verify_coverage(
    scope: &CheckpointScope<'_>,
    checkpoint: &Checkpoint,
    rebuilt: &BTreeSet<(String, String)>,
) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let engine = scope.engine;
    if !checkpoint
        .coverage
        .keys()
        .eq(scope.context.io_disk_ids.iter())
    {
        return Err(CheckpointError::Members.into());
    }
    for d in engine.config.io_disks() {
        let root = &checkpoint_disk(engine, d)?.root;
        let entries = checkpoint
            .coverage
            .get(&d.disk_id)
            .ok_or(CheckpointError::Members)?;
        if entries.len() > MAX_COVERAGE_ENTRIES {
            return Err(CheckpointError::CoverageEntries {
                path: d.root.clone(),
                actual: entries.len(),
                limit: MAX_COVERAGE_ENTRIES,
            }
            .into());
        }
        for entry in entries {
            let path = Path::new(&entry.path);
            let parts = relative_parts(path)?;
            if !parts
                .first()
                .is_some_and(|p| matches!(*p, "metadata" | "histories"))
            {
                return Err(CheckpointError::InvalidPath(path.into()).into());
            }
            if parts.len() >= 6
                && parts[0] == "histories"
                && parts[2] == "file-containers"
                && parts.last().is_some_and(|name| {
                    matches!(
                        *name,
                        "container.json" | crate::container_storage::file_database::NAME
                    )
                })
                && match path.parent() {
                    Some(parent) => named_container(root, parent)?,
                    None => false,
                }
                && rebuilt.contains(&(parts[1].to_owned(), parts[3].to_owned()))
            {
                // A sealed suffix names the file whose projections will be
                // reconstructed from the final admitted State. The installer
                // retains displaced database bytes and verifies its bound
                // intent before publishing the replacement.
                continue;
            }
            let metadata = metadata(root, path);
            match (&entry.value, metadata) {
                (Covered::Absent, Ok(None)) => (),
                (Covered::Directory { children: expected }, Ok(Some(m)))
                    if m.is_dir() && !m.file_type().is_symlink() =>
                {
                    if let Some(expected) = expected {
                        if children(root, path)? != *expected {
                            return Err(
                                CheckpointError::DirectoryChildren(d.root.join(path)).into()
                            );
                        }
                    } else if !append_only(root, path)? {
                        return Err(CheckpointError::InvalidException(d.root.join(path)).into());
                    }
                }
                (
                    Covered::File {
                        length,
                        hash,
                        links,
                    },
                    Ok(Some(m)),
                ) if m.is_file() && !m.file_type().is_symlink() => {
                    if m.len() != *length || m.nlink() != *links {
                        return Err(CheckpointError::FileMetadata(d.root.join(path)).into());
                    }
                    match hash {
                        Some(hash) if file_hash(root, &d.root.join(path))? == *hash => {
                            // Append-only directories may gain an alternate layout for an
                            // already covered hash. That addition must not hide conflicting
                            // evidence behind the checkpoint's historical replay shortcut.
                            if parts.first() == Some(&"metadata")
                                && parts.get(1) == Some(&"objects")
                            {
                                let kind = parts
                                    .get(2)
                                    .ok_or_else(|| CheckpointError::InvalidPath(path.into()))?;
                                let object_hash = parts
                                    .last()
                                    .and_then(|name| name.strip_suffix(".json"))
                                    .ok_or_else(|| CheckpointError::InvalidPath(path.into()))?;
                                let paths = existing_object_paths_at(root, d, kind, object_hash)?;
                                let covered = d.root.join(path);
                                if !paths.contains(&covered) {
                                    return Err(
                                        CheckpointError::PathState(d.root.join(path)).into()
                                    );
                                }
                                for alternate in paths {
                                    if alternate != covered && file_hash(root, &alternate)? != *hash
                                    {
                                        return Err(CheckpointError::FileContent(alternate).into());
                                    }
                                }
                            }
                        }
                        None if payload(root, path)? => (),
                        Some(_) => {
                            return Err(CheckpointError::FileContent(d.root.join(path)).into());
                        }
                        None => {
                            return Err(CheckpointError::InvalidException(d.root.join(path)).into());
                        }
                    }
                }
                (_, Err(source)) => {
                    return Err(source.context(
                        ErrorCode::CheckpointInvalid,
                        format!("cannot inspect covered path {:?}", d.root.join(path)),
                    ));
                }
                (_, Ok(None)) => {
                    return Err(Error::from(std::io::Error::from(rustix::io::Errno::NOENT))
                        .context(
                            ErrorCode::CheckpointInvalid,
                            format!("cannot inspect covered path {:?}", d.root.join(path)),
                        ));
                }
                _ => return Err(CheckpointError::PathState(d.root.join(path)).into()),
            }
        }
    }
    Ok(())
}
pub(super) fn load(
    engine: &Engine,
    prefixes: &BTreeMap<String, BTreeSet<String>>,
    records: &Records,
) -> Result<Option<Checkpoint>> {
    let Some(scope) = CheckpointScope::admit(engine)? else {
        return Ok(None);
    };
    let mut chosen = None;
    for d in &scope.members {
        let Some(file) = engine
            .disk_resources
            .get(d)?
            .root
            .open_optional_read_relative(&layout::REPLAY_CHECKPOINT)?
        else {
            return Ok(None);
        };
        let envelope: Envelope = decode_strict(&checkpoint_bytes(file, MAX_CHECKPOINT_BYTES)?)?;
        let checkpoint = envelope.checkpoint;
        if checkpoint.context != scope.context
            || !checkpoint
                .tips
                .keys()
                .eq(scope.context.metadata_disk_ids.iter())
            || !checkpoint
                .tips
                .iter()
                .all(|(id, tip)| prefixes.get(id).is_some_and(|hashes| hashes.contains(tip)))
            || envelope.hash != object_hash("replay-checkpoint", &checkpoint)?
            || chosen.as_ref().is_some_and(|c| c != &checkpoint)
        {
            return Ok(None);
        }
        chosen = Some(checkpoint);
    }
    let Some(checkpoint) = chosen else {
        return Ok(None);
    };
    let verified = (|| {
        let rebuilt = suffix_rebuilt_containers(engine, &checkpoint, records)?;
        verify_coverage(&scope, &checkpoint, &rebuilt)
    })();
    match verified {
        Ok(()) => Ok(Some(checkpoint)),
        Err(error) if error.code() == ErrorCode::CheckpointInvalid => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) fn save(
    engine: &Engine,
    scanned_journals: &BTreeMap<String, Vec<(Record, String)>>,
) -> Result<()> {
    if engine.blocked
        || engine.snapshot.is_none()
        || !engine.state.directory_git_repositories.is_empty()
    {
        return Ok(());
    }
    let Some(scope) = CheckpointScope::admit(engine)? else {
        return Ok(());
    };
    for disk in engine.config.io_disks() {
        checkpoint_disk(engine, disk)?;
    }
    let mut tips = BTreeMap::new();
    let mut candidate = None;
    let mut absent = BTreeMap::<String, BTreeSet<String>>::new();
    let mut version_ids = BTreeSet::new();
    for d in &scope.members {
        let root = &engine.disk_resources.get(d)?.root;
        let records = &scanned_journals[&d.disk_id];
        // Preserve absences that made historical recovery skip local proof or
        // container work. Restoring such a prefix path requires full repair.
        for (record, _) in records
            .iter()
            .filter(|(r, _)| r.record_type == RecordType::Publication)
        {
            let c: Candidate = load_object_at(root, d, "candidate", &record.candidate_hash)?;
            for evidence in &c.versions {
                version_ids.insert(evidence.version.version_id.clone());
                if version_ids.len() > MAX_COVERAGE_ENTRIES {
                    return Err(CheckpointError::VersionIds {
                        actual: version_ids.len(),
                        limit: MAX_COVERAGE_ENTRIES,
                    }
                    .into());
                }
                for slot in &evidence.placement.route_slot_ids {
                    let target = engine.disk(slot)?;
                    if engine.config.excludes_io_disk(&target.disk_id) {
                        continue;
                    }
                    let target_root = &engine.disk_resources.get(target)?.root;
                    let history = Path::new("histories").join(&evidence.version.history_tree_id);
                    for relative in [
                        history.clone(),
                        history.join("versions").join(&evidence.version.version_id),
                        history
                            .join("file-containers")
                            .join(&evidence.version.file_id),
                    ] {
                        if metadata(target_root, &relative)?.is_none() {
                            let paths = absent.entry(target.disk_id.clone()).or_default();
                            paths.insert(
                                relative
                                    .to_str()
                                    .ok_or_else(|| {
                                        CheckpointError::InvalidPath(relative.to_path_buf())
                                    })?
                                    .into(),
                            );
                            if paths.len() > MAX_COVERAGE_ENTRIES {
                                return Err(CheckpointError::AbsentPaths {
                                    actual: paths.len(),
                                    disk: target.disk_id.clone(),
                                    limit: MAX_COVERAGE_ENTRIES,
                                }
                                .into());
                            }
                        }
                    }
                }
            }
        }
        let Some((last, hash)) = records.last() else {
            return Ok(());
        };
        tips.insert(d.disk_id.clone(), hash.clone());
        let published = records
            .iter()
            .rev()
            .find(|(r, _)| r.record_type == RecordType::Publication)
            .map(|(r, _)| (r.sequence, r.candidate_hash.clone()));
        if candidate
            .as_ref()
            .is_some_and(|c| Some(c) != published.as_ref())
        {
            return Ok(());
        }
        candidate = published;
        if last.sequence > engine.state.sequence {
            return Ok(());
        }
    }
    let Some((sequence, candidate_hash)) = candidate else {
        return Ok(());
    };
    let coverage = coverage(&scope, &absent)?;
    let checkpoint = Checkpoint {
        context: scope.context,
        sequence,
        candidate_hash,
        snapshot_hash: engine
            .snapshot
            .clone()
            .expect("invariant: checkpoint save requires a snapshot"),
        tips,
        coverage,
        version_ids,
    };
    let envelope = Envelope {
        hash: object_hash("replay-checkpoint", &checkpoint)?,
        checkpoint,
    };
    let bytes = encode_metadata(&envelope, MAX_CHECKPOINT_BYTES)?;
    for d in &scope.members {
        engine
            .disk_resources
            .get(d)?
            .root
            .write_relative_atomic(&layout::REPLAY_CHECKPOINT, &bytes, true)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/recovery_checkpoint/tests.rs"]
mod tests;
