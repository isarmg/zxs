//! Conservative durable replay checkpoint. Journals remain authoritative.
//!
//! All configured trusted members must agree on a checkpoint whose tips occur in the
//! fully verified journal prefixes. Covered metadata/proofs and sensitive layout
//! are reverified; unrelated append-only additions permit normal suffix replay.
//! Missing or changed covered dependencies fall back to full repair. This bounds
//! historical state decoding, not verification work or total startup time.
use super::Engine;
use crate::journal::RecordType;
use crate::{disk::*, journal::*, *};
use data_serialization::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Component, Path},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serialization(deny_unknown_fields)]
pub(super) struct Checkpoint {
    config_hash: String,
    pub sequence: u64,
    pub candidate_hash: String,
    pub snapshot_hash: String,
    tips: BTreeMap<String, String>,
    coverage: BTreeMap<String, Vec<CoverageEntry>>,
    version_ids: BTreeSet<String>,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Envelope {
    checkpoint: Checkpoint,
    hash: String,
}
fn path(d: &DiskConfig) -> std::path::PathBuf {
    d.root.join("metadata/replay-checkpoint.json")
}
fn invalid() -> Error {
    Error::new(
        storage_contract::ErrorCode::CheckpointInvalid,
        "replay checkpoint coverage differs",
    )
}

// Read every repair-relevant byte, not metadata timestamps. Installed named
// version payloads are excluded: recovery reconstructs their publication proof
// without re-verifying the body. Prepared content remains hashed because replay
// must verify it before installation, including after an interrupted move.
fn named_container(root: &Path, relative: &Path) -> Result<bool> {
    let parts = relative_parts(relative)?;
    if parts.len() < 5 || parts[0] != "histories" || parts[2] != "file-containers" {
        return Ok(false);
    }
    // First installation creates a direct-name container before namespace
    // placement. Namespaced containers must carry their exact file ID.
    if parts.len() == 5 && parts[4] != "namespace" {
        return Ok(true);
    }
    let Some(file) = open_optional_regular(&root.join(relative).join(".zxs-container-owner.json"))?
    else {
        return Ok(false);
    };
    let owner: String = decode_strict(&read_file_bytes(file, MAX_METADATA)?)?;
    Ok(owner == parts[3])
}
fn payload(root: &Path, relative: &Path) -> Result<bool> {
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
pub(super) type Records = BTreeMap<u64, BTreeMap<String, Vec<(String, Record)>>>;
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
            Component::Normal(name) => name.to_str().ok_or_else(invalid),
            _ => Err(invalid()),
        })
        .collect()
}
fn excluded(path: &Path) -> bool {
    path == Path::new("metadata/application-log")
        || path == Path::new("metadata/replay-checkpoint.json")
}
fn append_only(root: &Path, path: &Path) -> Result<bool> {
    let parts = relative_parts(path)?;
    if path.file_name().is_some_and(|name| name == "versions")
        && match path.parent() {
            Some(parent) => named_container(root, parent)?,
            None => false,
        }
    {
        return Ok(true);
    }
    Ok(
        (parts.first() == Some(&"metadata") && parts.get(1) == Some(&"objects"))
            || path == Path::new("metadata/publication-seals")
            || (parts.first() == Some(&"histories")
                && (parts.len() <= 2
                    || (parts.len() == 3 && matches!(parts[2], "versions" | "file-containers"))
                    || (parts.len() == 5
                        && parts[2] == "versions"
                        && parts[4] == "protection-proofs"))),
    )
}
fn children(root: &Path, path: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut bytes = 0;
    for entry in std::fs::read_dir(root.join(path))? {
        let name = entry?.file_name().into_string().map_err(|name| {
            Error::PathEncoding(name).context(
                ErrorCode::CheckpointInvalid,
                "replay checkpoint coverage differs",
            )
        })?;
        if excluded(&path.join(&name)) {
            continue;
        }
        bytes += name.len();
        if out.len() >= MAX_COVERAGE_ENTRIES || bytes > MAX_COVERAGE_BYTES {
            return Err(invalid());
        }
        out.push(name);
    }
    out.sort();
    Ok(out)
}
fn file_hash(path: &Path) -> Result<String> {
    let mut file = open_regular(path)?;
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
    *budget = budget
        .checked_add(canonical_json(&entry)?.len())
        .ok_or_else(invalid)?;
    if *budget > MAX_COVERAGE_BYTES || out.len() >= MAX_COVERAGE_ENTRIES {
        return Err(invalid());
    }
    out.push(entry);
    Ok(())
}
fn inventory(
    root: &Path,
    relative: &Path,
    out: &mut Vec<CoverageEntry>,
    budget: &mut usize,
) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(root.join(relative))?;
    if metadata.file_type().is_symlink() || !(metadata.is_dir() || metadata.is_file()) {
        return Err(invalid());
    }
    let name = relative.to_str().ok_or_else(invalid)?.to_owned();
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
                        Some(file_hash(&root.join(relative))?)
                    },
                },
            },
            budget,
        )?;
    }
    Ok(())
}
fn coverage(
    engine: &Engine,
    absent: &BTreeMap<String, BTreeSet<String>>,
) -> Result<BTreeMap<String, Vec<CoverageEntry>>> {
    let mut out = BTreeMap::new();
    let mut budget = 0;
    for d in &engine.config.disks {
        validate_seal(&engine.config, d)?;
        let mut entries = Vec::new();
        inventory(&d.root, Path::new("metadata"), &mut entries, &mut budget)?;
        inventory(&d.root, Path::new("histories"), &mut entries, &mut budget)?;
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
                records.iter().any(|(_, r)| {
                    matches!(r.record_type, RecordType::Accept | RecordType::Publication)
                })
            })
            .collect::<Vec<_>>();
        if active.len() > 1 {
            return Err(invalid());
        }
        let Some((hash, records)) = active.first() else {
            continue;
        };
        let Some((slot, r)) = records
            .iter()
            .find(|(_, r)| r.record_type == RecordType::Publication)
        else {
            break;
        };
        let disk = engine.disk(slot)?;
        let c: Candidate = load_object(disk, "candidate", hash)?;
        engine.verify_candidate_membership(&c)?;
        let p: Publication = load_object(disk, "publication", &r.object_hash)?;
        if c.pool_id != engine.config.pool_id
            || c.sequence != *sequence
            || c.previous_snapshot != previous
        {
            return Err(invalid());
        }
        // Only a continuous, fully sealed suffix may waive checking a derived
        // container cache. Pending/forked decisions never authorize exceptions.
        let sealed = engine.members().iter().all(|d| {
            read_json::<PublicationSeal>(&publication_seal_path(d, hash))
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
    engine: &Engine,
    checkpoint: &Checkpoint,
    rebuilt: &BTreeSet<(String, String)>,
) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    if checkpoint.coverage.len() != engine.config.disks.len() {
        return Err(invalid());
    }
    for d in &engine.config.disks {
        validate_seal(&engine.config, d)?;
        let entries = checkpoint.coverage.get(&d.disk_id).ok_or_else(invalid)?;
        if entries.len() > MAX_COVERAGE_ENTRIES {
            return Err(invalid());
        }
        for entry in entries {
            let path = Path::new(&entry.path);
            let parts = relative_parts(path)?;
            if !parts
                .first()
                .is_some_and(|p| matches!(*p, "metadata" | "histories"))
            {
                return Err(invalid());
            }
            if parts.len() >= 6
                && parts[0] == "histories"
                && parts[2] == "file-containers"
                && parts.last() == Some(&"container.json")
                && match path.parent() {
                    Some(parent) => named_container(&d.root, parent)?,
                    None => false,
                }
                && rebuilt.contains(&(parts[1].to_owned(), parts[3].to_owned()))
            {
                continue;
            }
            let metadata = std::fs::symlink_metadata(d.root.join(path));
            match (&entry.value, metadata) {
                (Covered::Absent, Err(e)) if e.kind() == std::io::ErrorKind::NotFound => (),
                (Covered::Directory { children: expected }, Ok(m))
                    if m.is_dir() && !m.file_type().is_symlink() =>
                {
                    if let Some(expected) = expected {
                        if children(&d.root, path)? != *expected {
                            return Err(invalid());
                        }
                    } else if !append_only(&d.root, path)? {
                        return Err(invalid());
                    }
                }
                (
                    Covered::File {
                        length,
                        hash,
                        links,
                    },
                    Ok(m),
                ) if m.is_file() && !m.file_type().is_symlink() => {
                    if m.len() != *length || m.nlink() != *links {
                        return Err(invalid());
                    }
                    match hash {
                        Some(hash) if file_hash(&d.root.join(path))? == *hash => {
                            // Append-only directories may gain an alternate layout for an
                            // already covered hash. That addition must not hide conflicting
                            // evidence behind the checkpoint's historical replay shortcut.
                            if parts.first() == Some(&"metadata")
                                && parts.get(1) == Some(&"objects")
                            {
                                let kind = parts.get(2).ok_or_else(invalid)?;
                                let object_hash = parts
                                    .last()
                                    .and_then(|name| name.strip_suffix(".json"))
                                    .ok_or_else(invalid)?;
                                let paths = existing_object_paths(d, kind, object_hash)?;
                                let covered = d.root.join(path);
                                if !paths.contains(&covered) {
                                    return Err(invalid());
                                }
                                for alternate in paths {
                                    if alternate != covered && file_hash(&alternate)? != *hash {
                                        return Err(invalid());
                                    }
                                }
                            }
                        }
                        None if payload(&d.root, path)? => (),
                        _ => return Err(invalid()),
                    }
                }
                _ => return Err(invalid()),
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
    let members = engine.members();
    let mut chosen = None;
    for d in &members {
        let Some(file) = open_optional_regular(&path(d))? else {
            return Ok(None);
        };
        let envelope: Envelope = decode_strict(&read_file_bytes(file, MAX_CHECKPOINT_BYTES)?)?;
        let checkpoint = envelope.checkpoint;
        if checkpoint.config_hash != object_hash("replay-config", &engine.config)?
            || checkpoint.tips.len() != members.len()
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
        verify_coverage(engine, &checkpoint, &rebuilt)
    })();
    match verified {
        Ok(()) => Ok(Some(checkpoint)),
        Err(error) if error.code() == ErrorCode::CheckpointInvalid => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) fn save(engine: &Engine) -> Result<()> {
    if engine.blocked || engine.snapshot.is_none() {
        return Ok(());
    }
    let mut tips = BTreeMap::new();
    let mut candidate = None;
    let mut absent = BTreeMap::<String, BTreeSet<String>>::new();
    let mut version_ids = BTreeSet::new();
    for d in engine.members() {
        validate_seal(&engine.config, &d)?;
        let scanned = scan(&d)?;
        if scanned.torn_tail {
            return Ok(());
        }
        // Preserve absences that made historical recovery skip local proof or
        // container work. Restoring such a prefix path requires full repair.
        for (record, _) in scanned
            .records
            .iter()
            .filter(|(r, _)| r.record_type == RecordType::Publication)
        {
            let c: Candidate = load_object(&d, "candidate", &record.candidate_hash)?;
            for evidence in &c.versions {
                version_ids.insert(evidence.version.version_id.clone());
                if version_ids.len() > MAX_COVERAGE_ENTRIES {
                    return Err(invalid());
                }
                for slot in &evidence.placement.route_slot_ids {
                    let target = engine.disk(slot)?;
                    let history = Path::new("histories").join(&evidence.version.history_tree_id);
                    for relative in [
                        history.clone(),
                        history.join("versions").join(&evidence.version.version_id),
                        history
                            .join("file-containers")
                            .join(&evidence.version.file_id),
                    ] {
                        match std::fs::symlink_metadata(target.root.join(&relative)) {
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                                let paths = absent.entry(target.disk_id.clone()).or_default();
                                paths.insert(relative.to_str().ok_or_else(invalid)?.into());
                                if paths.len() > MAX_COVERAGE_ENTRIES {
                                    return Err(invalid());
                                }
                            }
                            Err(e) => return Err(e.into()),
                            Ok(_) => (),
                        }
                    }
                }
            }
        }
        let Some((last, hash)) = scanned.records.last() else {
            return Ok(());
        };
        tips.insert(d.disk_id.clone(), hash.clone());
        let published = scanned
            .records
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
    let checkpoint = Checkpoint {
        config_hash: object_hash("replay-config", &engine.config)?,
        sequence,
        candidate_hash,
        snapshot_hash: engine
            .snapshot
            .clone()
            .expect("invariant: checkpoint save requires a snapshot"),
        tips,
        coverage: coverage(engine, &absent)?,
        version_ids,
    };
    let envelope = Envelope {
        hash: object_hash("replay-checkpoint", &checkpoint)?,
        checkpoint,
    };
    let bytes = encode_metadata(&envelope, MAX_CHECKPOINT_BYTES)?;
    for d in engine.members() {
        write_atomic(&path(&d), &bytes, true)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/recovery_checkpoint/tests.rs"]
mod tests;
