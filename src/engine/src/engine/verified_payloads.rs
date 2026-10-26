//! Owned, non-serializable evidence for the live prepare/decide path only.
//! Recovery cannot construct a proof without reading the complete payload.
use crate::{disk::*, journal::*, *};
use std::{
    fs::File,
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

#[derive(Debug, PartialEq, Eq)]
struct Stamp {
    dev: u64,
    ino: u64,
    size: u64,
    links: u64,
    mode: u32,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl Stamp {
    fn of(file: &File) -> Result<Self> {
        let m = file.metadata()?;
        Ok(Self {
            dev: m.dev(),
            ino: m.ino(),
            size: m.len(),
            links: m.nlink(),
            mode: m.mode(),
            modified: (m.mtime(), m.mtime_nsec()),
            changed: (m.ctime(), m.ctime_nsec()),
        })
    }
}
struct HeldFile {
    file: File,
    stamp: Stamp,
}
impl HeldFile {
    fn open(path: &Path) -> Result<Self> {
        let file = open_regular(path)?;
        let stamp = Stamp::of(&file)?;
        Ok(Self { file, stamp })
    }
    fn validate(&self, path: &Path) -> Result<()> {
        let current = open_regular(path)?;
        if Stamp::of(&self.file)? != self.stamp || Stamp::of(&current)? != self.stamp {
            return Err(Error::new(
                storage_contract::ErrorCode::PreparationStale,
                "verified payload or metadata changed after preparation",
            ));
        }
        Ok(())
    }
}
struct VerifiedPayload {
    version_id: String,
    slot_id: String,
    version_hash: String,
    directory: PathBuf,
    body: HeldFile,
    metadata: Vec<(PathBuf, HeldFile)>,
}
struct HeldDisk {
    root: PathBuf,
    directory: File,
    device: u64,
    inode: u64,
    seal: HeldFile,
}
pub(super) struct VerifiedPayloadSet {
    entries: Vec<VerifiedPayload>,
    disks: Vec<HeldDisk>,
    full_revalidation: bool,
}
const MAX_HELD_PAYLOAD_FILES: usize = 128;
// Thread-local counters isolate concurrently running tests. No release cost.
#[cfg(test)]
std::thread_local! {
    static VERIFICATION_COST: std::cell::Cell<(u64, u64)> = const { std::cell::Cell::new((0, 0)) };
    static AFTER_REPLICA_VALIDATION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}
/// Disk identities are pinned before copying begins. Only complete verification
/// can turn this preparation into a live decision proof.
pub(super) struct PayloadPreparation {
    disks: Vec<HeldDisk>,
}
impl PayloadPreparation {
    pub(super) fn new(config: &ValidatedConfig, candidate: &Candidate) -> Result<Self> {
        let mut disks = Vec::new();
        for disk in &config.disks {
            if !disk.metadata_member
                && !candidate
                    .versions
                    .iter()
                    .any(|evidence| evidence.placement.route_slot_ids.contains(&disk.slot_id))
            {
                continue;
            }
            validate_seal(config, disk)?;
            let directory = File::open(&disk.root)?;
            let metadata = directory.metadata()?;
            disks.push(HeldDisk {
                root: disk.root.clone(),
                directory,
                device: metadata.dev(),
                inode: metadata.ino(),
                seal: HeldFile::open(&disk.root.join("disk.json"))?,
            });
        }
        Ok(Self { disks })
    }
    pub(super) fn verify(
        self,
        config: &ValidatedConfig,
        candidate: &Candidate,
    ) -> Result<VerifiedPayloadSet> {
        let replicas = candidate
            .versions
            .iter()
            .try_fold(0usize, |count, evidence| {
                count
                    .checked_add(evidence.placement.route_slot_ids.len())
                    .ok_or_else(|| {
                        Error::new(
                            storage_contract::ErrorCode::ResourceLimit,
                            "payload proof count overflow",
                        )
                    })
            })?;
        let full_revalidation = replicas > MAX_HELD_PAYLOAD_FILES / 4;
        let mut entries = Vec::new();
        if !full_revalidation {
            for evidence in &candidate.versions {
                for slot in &evidence.placement.route_slot_ids {
                    entries.push(verify_replica(config, &evidence.version, slot)?);
                }
            }
        }
        // Large candidates retain no per-replica evidence. validate below
        // already hashes every byte with held-inode checks; a preceding pass
        // would only hash and immediately discard the same proof.
        let proof = VerifiedPayloadSet {
            entries,
            disks: self.disks,
            full_revalidation,
        };
        proof.validate(config, candidate)?;
        Ok(proof)
    }
}
fn verify_replica(
    config: &ValidatedConfig,
    version: &FileVersion,
    slot: &str,
) -> Result<VerifiedPayload> {
    #[cfg(test)]
    VERIFICATION_COST.with(|cost| {
        let (replicas, bytes) = cost.get();
        cost.set((replicas + 1, bytes));
    });
    let disk = config
        .disks
        .iter()
        .find(|disk| disk.slot_id == slot)
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::SlotNotFound,
                "prepared payload slot disappeared",
            )
        })?;
    validate_seal(config, disk)?;
    let directory = version_dir(disk, &version.history_tree_id, &version.version_id);
    let mut metadata = Vec::new();
    for name in ["version.json", "checksums.bin", "payload.json"] {
        let path = directory.join(name);
        metadata.push((path.clone(), HeldFile::open(&path)?));
    }
    let recorded: FileVersion = read_json(&directory.join("version.json"))?;
    let version_hash = object_hash("version", version)?;
    if object_hash("version", &recorded)? != version_hash {
        return Err(Error::new(
            storage_contract::ErrorCode::ObjectCollision,
            "prepared version metadata differs",
        ));
    }
    let checks = read_bytes(&directory.join("checksums.bin"), MAX_CHECKSUM_MANIFEST)?;
    if blake3::hash(&checks).to_hex().as_str() != version.checksum_manifest_hash {
        return Err(Error::new(
            storage_contract::ErrorCode::ChecksumMismatch,
            "checksum manifest digest differs",
        ));
    }
    let manifest = ChecksumManifest::decode(&checks)?;
    if manifest.logical_size != version.logical_size
        || manifest.chunk_bytes != version.checksum_chunk_bytes
    {
        return Err(Error::new(
            storage_contract::ErrorCode::ChecksumMismatch,
            "checksum header differs from version",
        ));
    }
    let path = crate::container_storage::content_path(&directory, version)?;
    let mut body = HeldFile::open(&path)?;
    if body.stamp.size != version.logical_size {
        return Err(Error::new(
            storage_contract::ErrorCode::ChecksumMismatch,
            "payload length differs",
        ));
    }
    let mut full = blake3::Hasher::new();
    let mut buffer = vec![0; manifest.chunk_bytes as usize];
    let mut remaining = manifest.logical_size;
    for expected in manifest.chunks {
        let n = remaining.min(buffer.len() as u64) as usize;
        body.file.read_exact(&mut buffer[..n])?;
        #[cfg(test)]
        VERIFICATION_COST.with(|cost| {
            let (replicas, bytes) = cost.get();
            cost.set((replicas, bytes + n as u64));
        });
        if blake3::hash(&buffer[..n]).as_bytes() != &expected {
            return Err(Error::new(
                storage_contract::ErrorCode::ChecksumMismatch,
                "payload chunk differs",
            ));
        }
        full.update(&buffer[..n]);
        remaining -= n as u64;
    }
    if remaining != 0 || full.finalize().to_hex().as_str() != version.content_hash {
        return Err(Error::new(
            storage_contract::ErrorCode::ChecksumMismatch,
            "complete payload hash differs",
        ));
    }
    body.validate(&path)?;
    for (path, held) in &metadata {
        held.validate(path)?;
    }
    Ok(VerifiedPayload {
        version_id: version.version_id.clone(),
        slot_id: slot.to_owned(),
        version_hash,
        directory,
        body,
        metadata,
    })
}

impl VerifiedPayloadSet {
    #[cfg(test)]
    fn prepare(config: &ValidatedConfig, candidate: &Candidate) -> Result<Self> {
        PayloadPreparation::new(config, candidate)?.verify(config, candidate)
    }
    fn validate_disks(&self) -> Result<()> {
        for disk in &self.disks {
            let current = std::fs::symlink_metadata(&disk.root)?;
            let held = disk.directory.metadata()?;
            if !current.is_dir()
                || current.file_type().is_symlink()
                || current.dev() != disk.device
                || current.ino() != disk.inode
                || held.dev() != disk.device
                || held.ino() != disk.inode
            {
                return Err(Error::new(
                    storage_contract::ErrorCode::PreparationStale,
                    "prepared disk directory identity changed",
                ));
            }
            disk.seal.validate(&disk.root.join("disk.json"))?;
        }
        Ok(())
    }
    pub(super) fn validate(&self, config: &ValidatedConfig, candidate: &Candidate) -> Result<()> {
        self.validate_disks()?;
        let expected: usize = candidate
            .versions
            .iter()
            .map(|e| e.placement.route_slot_ids.len())
            .sum();
        if (!self.full_revalidation && expected != self.entries.len())
            || (self.full_revalidation && !self.entries.is_empty())
        {
            return Err(Error::new(
                storage_contract::ErrorCode::PreparationStale,
                "payload proof count differs",
            ));
        }
        for evidence in &candidate.versions {
            for slot in &evidence.placement.route_slot_ids {
                self.validate_payload(config, &evidence.version, slot)?;
            }
        }
        #[cfg(test)]
        AFTER_REPLICA_VALIDATION.with(|hook| {
            if let Some(run) = hook.borrow_mut().take() {
                run();
            }
        });
        // Long-running full revalidation must also preserve the disk identity
        // pinned before preparation, including after the final replica read.
        self.validate_disks()
    }
    pub(super) fn validate_payload(
        &self,
        config: &ValidatedConfig,
        version: &FileVersion,
        slot: &str,
    ) -> Result<()> {
        if self.full_revalidation {
            // Large candidates keep only disk identities pinned. Re-read every
            // byte with the same metadata and held-inode checks, one replica at
            // a time, rather than exhausting process-wide file descriptors.
            verify_replica(config, version, slot)?;
            return Ok(());
        }
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.version_id == version.version_id && entry.slot_id == slot)
            .ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::PreparationStale,
                    "payload proof missing",
                )
            })?;
        let disk = config
            .disks
            .iter()
            .find(|disk| disk.slot_id == slot)
            .ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::PreparationStale,
                    "payload slot changed",
                )
            })?;
        validate_seal(config, disk)?;
        if entry.directory != version_dir(disk, &version.history_tree_id, &version.version_id)
            || entry.version_hash != object_hash("version", version)?
        {
            return Err(Error::new(
                storage_contract::ErrorCode::PreparationStale,
                "payload proof identity differs",
            ));
        }
        entry
            .body
            .validate(&crate::container_storage::content_path(
                &entry.directory,
                version,
            )?)?;
        for (path, held) in &entry.metadata {
            held.validate(path)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/verified_payloads/tests.rs"]
mod tests;
