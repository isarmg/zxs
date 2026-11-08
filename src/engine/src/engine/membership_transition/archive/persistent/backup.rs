//! Convert an independently authenticated fixed-Genesis prefix plus the actual
//! original fenced journals into the normal native control-archive format.
//! The result is still frozen predecessor evidence, not a successor authority.
use super::*;
use crate::engine::backup_preparation::BackupNativeObjects;
use crate::engine::membership_transition::BackupFrozenControl;
use crate::history_package::preparation_checkpoint;
use std::sync::atomic::AtomicBool;

const MAX_FILES: usize = 32768;
const MAX_BYTES: usize = 256 * 1024 * 1024;

pub(in crate::engine) struct PreparedBackupControl {
    root_hash: String,
    transition_hash: String,
    files: BTreeMap<PathBuf, Vec<u8>>,
    journals: Vec<MembershipJournalInspection>,
}
struct Files {
    values: BTreeMap<PathBuf, Vec<u8>>,
    bytes: usize,
}
fn limit() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "backup generation control exceeds 32768 files / 256 MiB material / 512 MiB observed reads",
    )
}
impl Files {
    fn insert(&mut self, path: PathBuf, bytes: Vec<u8>) -> Result<()> {
        match self.values.entry(path) {
            std::collections::btree_map::Entry::Occupied(entry) if entry.get() == &bytes => Ok(()),
            std::collections::btree_map::Entry::Occupied(_) => Err(Error::new(
                ErrorCode::ObjectCollision,
                "native control objects disagree at an immutable address",
            )),
            std::collections::btree_map::Entry::Vacant(entry) => {
                self.bytes = self.bytes.checked_add(bytes.len()).ok_or_else(limit)?;
                if self.bytes > MAX_BYTES {
                    return Err(limit());
                }
                entry.insert(bytes);
                if self.values.len() > MAX_FILES {
                    return Err(limit());
                }
                Ok(())
            }
        }
    }
    fn object(&mut self, kind: &str, hash: &str, bytes: Vec<u8>) -> Result<()> {
        if !valid_hash(hash)
            || kind.is_empty()
            || !kind.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
        {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "invalid generated native control object address",
            ));
        }
        self.insert(
            PathBuf::from("objects")
                .join(kind)
                .join(&hash[..2])
                .join(format!("{hash}.json")),
            bytes,
        )
    }
    fn prepared(&mut self, object: PreparedMetadataObject) -> Result<()> {
        self.object(object.kind(), &object.hash, object.bytes().to_vec())
    }
    fn map(&mut self, values: json_codec::Map<String, json_codec::Value>) -> Result<Reference> {
        let mut encoder = Encoder::new(MAX_METADATA);
        let reference = encoder
            .map_root(values, ProofMeasure::Plain)?
            .reference()
            .clone();
        for object in encoder.finish() {
            self.prepared(object)?;
        }
        Ok(reference)
    }
}
impl PreparedBackupControl {
    pub(in crate::engine) fn capture(
        frozen: &BackupFrozenControl,
        prefix: BackupNativeObjects,
        stop: &AtomicBool,
    ) -> Result<Self> {
        let transition = object_hash(INTENT_KIND, &frozen.intent)?;
        let mut files = Files {
            values: BTreeMap::new(),
            bytes: 0,
        };
        for ((kind, hash), bytes) in prefix.objects {
            preparation_checkpoint(stop)?;
            // Those two domains describe the previous backup envelope, not
            // native membership archive addresses. Original member seals below
            // retain their exact bytes, and publication seals are re-encoded in
            // the native archive's own canonical domain.
            if matches!(
                kind.as_str(),
                "backup-original-seal" | "backup-publication-seal"
            ) {
                continue;
            }
            files.object(&kind, &hash, bytes)?;
        }
        let mut seals = json_codec::Map::new();
        for (candidate, seal) in prefix.seals {
            preparation_checkpoint(stop)?;
            let object = PreparedMetadataObject::new(SEAL_KIND, &seal, MAX_METADATA)?;
            seals.insert(candidate, object.hash.clone().into());
            files.prepared(object)?;
        }
        let mut members = json_codec::Map::new();
        let mut journals = Vec::with_capacity(frozen.members.len());
        for member in &frozen.members {
            preparation_checkpoint(stop)?;
            let path = PathBuf::from("journals")
                .join(&member.disk.disk_id)
                .join(&member.disk.slot_id)
                .join(member.disk.binding_generation.to_string());
            let metadata = ArchivedMember {
                journal: member.inspection.clone(),
                config_bytes_hash: content_hash(&member.config),
                seal_bytes_hash: content_hash(&member.seal),
                fence: member.fence.clone(),
            };
            members.insert(
                member.disk.disk_id.clone(),
                json_codec::to_value(&metadata)?,
            );
            files.insert(path.join("config.json"), member.config.clone())?;
            files.insert(path.join("disk.json"), member.seal.clone())?;
            files.insert(path.join("journal.bin"), member.journal.clone())?;
            journals.push(member.inspection.clone());
        }
        journals.sort_by(|a, b| a.disk_id.cmp(&b.disk_id));
        files.prepared(PreparedMetadataObject::new(
            INTENT_KIND,
            &frozen.intent,
            MAX_METADATA,
        )?)?;
        let format = Format::for_intent(&frozen.intent);
        let root = ArchiveRoot {
            format,
            transition_hash: transition.clone(),
            members: files.map(members)?,
            publication_seals: files.map(seals)?,
        };
        let root = PreparedMetadataObject::new(ROOT_KIND, &root, MAX_METADATA)?;
        let root_hash = root.hash.clone();
        files.insert(PathBuf::from(ROOT_FILE), root.bytes().to_vec())?;
        files.prepared(root)?;
        files.insert(
            PathBuf::from(BINDING_FILE),
            canonical_json(&ArchiveBinding {
                format,
                transition_hash: transition.clone(),
            })?,
        )?;
        Ok(Self {
            root_hash,
            transition_hash: transition,
            files: files.values,
            journals,
        })
    }
    pub(in crate::engine) fn root_hash(&self) -> &str {
        &self.root_hash
    }
    pub(in crate::engine) fn files(&self) -> impl Iterator<Item = (&PathBuf, &[u8])> {
        self.files
            .iter()
            .map(|(path, bytes)| (path, bytes.as_slice()))
    }
    pub(in crate::engine) fn verify(
        &self,
        root: DiskRoot,
        stop: &AtomicBool,
    ) -> Result<MembershipArchiveInspection> {
        let mut bytes = 0usize;
        let mut steps = 0usize;
        let observed = verify_generation_archive_observed(
            root,
            &self.root_hash,
            &self.transition_hash,
            &mut |count| {
                preparation_checkpoint(stop)?;
                bytes = bytes.checked_add(count).ok_or_else(limit)?;
                steps = steps.checked_add(1).ok_or_else(limit)?;
                if bytes > 2 * MAX_BYTES || steps > 131072 {
                    return Err(limit());
                }
                Ok(())
            },
            MAX_FILES,
        )?;
        if observed.journals != self.journals {
            return Err(Error::new(
                ErrorCode::LogFork,
                "native archive did not replay the exact originally decided fence journals",
            ));
        }
        Ok(observed)
    }
}
