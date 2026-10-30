//! Owned, non-serializable evidence for the live prepare/decide path only.
//! Recovery cannot construct a proof without reading the complete payload.
use crate::engine::provenance::MetadataIntegrity;
use crate::file_stamp::FileStamp;
use crate::{disk::*, journal::*, *};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};

struct HeldFile {
    file: File,
    stamp: FileStamp,
}
impl HeldFile {
    fn open(root: &DiskRoot, path: &Path) -> Result<Self> {
        let file = root.open_regular(path)?;
        let stamp = FileStamp::of(&file)?;
        Ok(Self { file, stamp })
    }
    fn validate(&self, root: &DiskRoot, path: &Path) -> Result<()> {
        let current = root.open_regular(path)?;
        if FileStamp::of(&self.file)? != self.stamp || FileStamp::of(&current)? != self.stamp {
            return Err(Error::new(
                storage_contract::ErrorCode::PreparationStale,
                "verified payload or metadata changed after preparation",
            ));
        }
        Ok(())
    }
}
struct VerifiedPayload {
    disk: Arc<DiskHandle>,
    version_id: String,
    slot_id: String,
    version_hash: String,
    directory: PathBuf,
    body: HeldFile,
    metadata: Vec<(PathBuf, HeldFile)>,
}
struct HeldSeal {
    metadata_member: bool,
    disk: Arc<DiskHandle>,
    path: PathBuf,
    seal: HeldFile,
}
pub(super) struct VerifiedPayloadSet {
    metadata_integrity: Arc<MetadataIntegrity>,
    entries: Vec<VerifiedPayload>,
    disks: Vec<HeldSeal>,
    resources: Arc<DiskResources>,
    full_revalidation: bool,
}
pub(super) const MAX_HELD_PAYLOAD_FILES: usize = 128;
// Thread-local counters isolate concurrently running tests. No release cost.
#[cfg(test)]
std::thread_local! {
    static VERIFICATION_COST: std::cell::Cell<(u64, u64)> = const { std::cell::Cell::new((0, 0)) };
    static AFTER_REPLICA_VALIDATION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}
/// Disk identities are pinned before copying begins. Only complete verification
/// can turn this preparation into a live decision proof.
pub(super) struct PayloadPreparation {
    metadata_integrity: Arc<MetadataIntegrity>,
    disks: Vec<HeldSeal>,
    resources: Arc<DiskResources>,
}
impl PayloadPreparation {
    pub(super) fn new(
        config: &ValidatedConfig,
        resources: &Arc<DiskResources>,
        candidate: &Candidate,
        metadata_integrity: Arc<MetadataIntegrity>,
    ) -> Result<Self> {
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
            let identity = (|| {
                let handle = resources.get(disk)?;
                let path = disk.root.join("disk.json");
                let seal = HeldFile::open(&handle.root, &path)?;
                let stored = decode_seal(&handle.root.read_bytes(&path, MAX_METADATA)?)?;
                validate_stored_seal(&stored, &handle.seal)?;
                Ok(HeldSeal {
                    metadata_member: disk.metadata_member,
                    disk: Arc::clone(handle),
                    path,
                    seal,
                })
            })();
            disks.push(if disk.metadata_member {
                metadata_integrity.observe(identity)?
            } else {
                identity?
            });
        }
        Ok(Self {
            metadata_integrity,
            disks,
            resources: Arc::clone(resources),
        })
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
                    entries.push(verify_replica(
                        config,
                        &self.resources,
                        &evidence.version,
                        slot,
                    )?);
                }
            }
        }
        // Large candidates retain no per-replica evidence. validate below
        // already hashes every byte with held-inode checks; a preceding pass
        // would only hash and immediately discard the same proof.
        let proof = VerifiedPayloadSet {
            metadata_integrity: self.metadata_integrity,
            entries,
            disks: self.disks,
            resources: self.resources,
            full_revalidation,
        };
        proof.validate(config, candidate)?;
        Ok(proof)
    }
}
fn verify_replica(
    config: &ValidatedConfig,
    resources: &DiskResources,
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
    let handle = resources.get(disk)?;
    let directory = version_dir(disk, &version.history_tree_id, &version.version_id);
    let mut metadata = Vec::new();
    for name in ["version.json", "checksums.bin", "payload.json"] {
        let path = directory.join(name);
        metadata.push((path.clone(), HeldFile::open(&handle.root, &path)?));
    }
    let recorded: FileVersion = handle
        .root
        .read_json_observed(&directory.join("version.json"), &mut |_| Ok(()))?;
    let version_hash = object_hash("version", version)?;
    if object_hash("version", &recorded)? != version_hash {
        return Err(Error::new(
            storage_contract::ErrorCode::ObjectCollision,
            "prepared version metadata differs",
        ));
    }
    let checks = handle
        .root
        .read_bytes(&directory.join("checksums.bin"), MAX_CHECKSUM_MANIFEST)?;
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
    let path =
        crate::container_storage::content_path_at(&handle.root, &directory, version, &mut |_| {
            Ok(())
        })?;
    let mut body = HeldFile::open(&handle.root, &path)?;
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
    if remaining != 0 || ContentHash::from_digest(full.finalize()) != version.content_hash {
        return Err(Error::new(
            storage_contract::ErrorCode::ChecksumMismatch,
            "complete payload hash differs",
        ));
    }
    body.validate(&handle.root, &path)?;
    for (path, held) in &metadata {
        held.validate(&handle.root, path)?;
    }
    Ok(VerifiedPayload {
        disk: Arc::clone(handle),
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
    fn prepare(engine: &Engine, candidate: &Candidate) -> Result<Self> {
        PayloadPreparation::new(
            &engine.config,
            &engine.disk_resources,
            candidate,
            Arc::clone(&engine.metadata_integrity),
        )?
        .verify(&engine.config, candidate)
    }
    fn validate_disks(&self) -> Result<()> {
        for disk in &self.disks {
            let identity = disk.seal.validate(&disk.disk.root, &disk.path);
            if disk.metadata_member {
                self.metadata_integrity.observe(identity)?;
            } else {
                identity?;
            }
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
        // Root identity is held; mutable seal bytes must still survive the
        // complete verification interval, including the final replica read.
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
            verify_replica(config, &self.resources, version, slot)?;
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
        if entry.directory != version_dir(disk, &version.history_tree_id, &version.version_id)
            || entry.version_hash != object_hash("version", version)?
        {
            return Err(Error::new(
                storage_contract::ErrorCode::PreparationStale,
                "payload proof identity differs",
            ));
        }
        entry.body.validate(
            &entry.disk.root,
            &crate::container_storage::content_path_at(
                &entry.disk.root,
                &entry.directory,
                version,
                &mut |_| Ok(()),
            )?,
        )?;
        for (path, held) in &entry.metadata {
            held.validate(&entry.disk.root, path)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/verified_payloads/tests.rs"]
mod tests;
