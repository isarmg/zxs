use crate::{disk::*, *};
use data_serialization::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
};
pub(crate) const MAX_FRAME: usize = 16 * 1024 * 1024;
const LOG_MAGIC: &[u8; 8] = b"ZXSLOG\0\0";
const LOG_HEADER_BYTES: usize = 12;

#[cfg(test)]
#[path = "../../../test/engine/unit/metadata_adversarial/segments.rs"]
mod metadata_adversarial_segments;
#[cfg(test)]
#[path = "../../../test/engine/unit/metadata_adversarial/budgets.rs"]
mod metadata_adversarial_budgets;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serialization(rename_all = "UPPERCASE")]
pub(crate) enum RecordType {
    Prepare,
    Accept,
    Finalize,
    Publication,
    Reprotect,
}
impl<'de> Deserialize<'de> for RecordType {
    fn deserialize<D: data_serialization::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let token = String::deserialize(deserializer)?;
        match token.as_str() {
            "PREPARE" => Ok(Self::Prepare),
            "ACCEPT" => Ok(Self::Accept),
            "FINALIZE" => Ok(Self::Finalize),
            "PUBLICATION" => Ok(Self::Publication),
            "REPROTECT" => Ok(Self::Reprotect),
            token => Err(data_serialization::de::Error::unknown_variant(
                token,
                &["PREPARE", "ACCEPT", "FINALIZE", "PUBLICATION", "REPROTECT"],
            )),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Record {
    pub record_type: RecordType,
    #[serialization(with = "storage_contract::u64_string")]
    pub sequence: u64,
    pub candidate_hash: String,
    pub object_hash: String,
    pub previous_frame_hash: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Receipt {
    pub disk_id: String,
    pub slot_id: String,
    pub frame_hash: String,
    pub record: Record,
}
pub(crate) fn receipt_binds(
    receipt: &Receipt,
    kind: RecordType,
    sequence: u64,
    candidate: &str,
    object: &str,
) -> Result<bool> {
    let r = &receipt.record;
    Ok(r.record_type == kind
        && r.sequence == sequence
        && r.candidate_hash == candidate
        && r.object_hash == object
        && r.previous_frame_hash.len() == 64
        && record_frame_hash(r)?.is_some_and(|hash| hash.to_hex().as_str() == receipt.frame_hash))
}
fn record_frame_hash(r: &Record) -> Result<Option<blake3::Hash>> {
    let payload = canonical_json(r)?;
    if payload.len() > MAX_FRAME {
        return Ok(None);
    }
    let mut header = Vec::with_capacity(LOG_HEADER_BYTES);
    header.extend(LOG_MAGIC);
    header.extend((payload.len() as u32).to_le_bytes());
    let mut digest = blake3::Hasher::new();
    digest.update(&header);
    digest.update(&payload);
    Ok(Some(digest.finalize()))
}
pub(crate) struct Scan {
    pub records: Vec<(Record, String)>,
    pub valid_bytes: u64,
    pub torn_tail: bool,
}
pub(crate) fn log_path(d: &DiskConfig) -> PathBuf {
    d.root.join("metadata/application-log/journal.bin")
}
#[cfg(test)]
pub(crate) fn scan(d: &DiskConfig) -> Result<Scan> {
    scan_at(&DiskRoot::open(&d.root)?, d)
}
pub(crate) fn scan_at(root: &DiskRoot, d: &DiskConfig) -> Result<Scan> {
    match root.open_optional_regular(&log_path(d))? {
        Some(file) => scan_frames(BufReader::new(file)),
        None => Ok(Scan {
            records: Vec::new(),
            valid_bytes: 0,
            torn_tail: false,
        }),
    }
}

/// Existing bytes stay untouched until recovery has accepted the publication prefix.
pub(crate) struct OpenJournal {
    directory: std::fs::File,
    file: Option<std::fs::File>,
    pub scan: Scan,
    #[cfg(feature = "fault-injection")]
    opening_fault: Option<OpeningFault>,
}
impl OpenJournal {
    #[cfg(test)]
    pub(crate) fn scan(d: &DiskConfig) -> Result<Self> {
        Self::scan_at(&DiskRoot::open(&d.root)?, d)
    }
    pub(crate) fn scan_at(root: &DiskRoot, d: &DiskConfig) -> Result<Self> {
        let directory = root.directory(log_path(d).parent().expect("journal parent"))?;
        let file = open_existing_journal(&directory, &journal_name())?;
        let scan = match &file {
            Some(file) => scan_frames(BufReader::new(file))?,
            None => Scan {
                records: Vec::new(),
                valid_bytes: 0,
                torn_tail: false,
            },
        };
        Ok(Self {
            directory,
            file,
            scan,
            #[cfg(feature = "fault-injection")]
            opening_fault: None,
        })
    }
    pub(crate) fn into_writer(self) -> Result<JournalWriter> {
        let file = match self.file {
            Some(file) => {
                // A surviving leaf may come from a failed creation before its directory fence.
                sync_open_journal(
                    &file,
                    &self.directory,
                    #[cfg(feature = "fault-injection")]
                    self.opening_fault,
                )?;
                file
            }
            None => create_synced_journal(&self.directory)?,
        };
        let mut records = self.scan.records;
        let start = records.last().map_or(0, |(last, _)| {
            records.partition_point(|(record, _)| record.sequence < last.sequence)
        });
        let records = records.split_off(start);
        Ok(JournalWriter {
            file,
            records,
            state: if self.scan.torn_tail {
                WriterState::TornTail(self.scan.valid_bytes)
            } else {
                WriterState::Ready
            },
            #[cfg(feature = "fault-injection")]
            fault: None,
        })
    }
}

/// The same bounded frame/chain parser serves recovery and streaming scrub.
/// This reader never repairs a torn tail or opens a writable descriptor.
pub(crate) struct JournalFrameReader<R> {
    reader: R,
    previous: String,
    last_sequence: u64,
    pub(crate) valid_bytes: u64,
    pub(crate) torn_tail: bool,
}

impl<R: Read> JournalFrameReader<R> {
    pub(crate) fn new(reader: R) -> Self {
        Self {
            reader,
            previous: "0".repeat(64),
            last_sequence: 0,
            valid_bytes: 0,
            torn_tail: false,
        }
    }

    /// One real-byte probe beyond a captured frontier, without chasing append-time EOF.
    pub(crate) fn probe_extra_observed(
        &mut self,
        observe: &mut impl FnMut(usize) -> Result<()>,
    ) -> Result<bool> {
        Ok(read_observed_fill(&mut self.reader, &mut [0u8; 1], observe)? != 0)
    }

    pub(crate) fn next_record(&mut self) -> Result<Option<(Record, String)>> {
        self.next_record_with_limit(usize::MAX, &mut |_| Ok(()))
    }

    pub(crate) fn next_record_observed(
        &mut self,
        observe: &mut impl FnMut(usize) -> Result<()>,
    ) -> Result<Option<(Record, String)>> {
        self.next_record_with_limit(64 * 1024, observe)
    }

    fn next_record_with_limit(
        &mut self,
        maximum_read: usize,
        observe: &mut impl FnMut(usize) -> Result<()>,
    ) -> Result<Option<(Record, String)>> {
        let mut header = [0u8; LOG_HEADER_BYTES];
        let got = read_fill_with_limit(&mut self.reader, &mut header, maximum_read, observe)?;
        if got == 0 {
            return Ok(None);
        }
        if got < LOG_HEADER_BYTES {
            self.torn_tail = true;
            return Ok(None);
        }
        if &header[..8] != LOG_MAGIC {
            return Err(Error::new(
                storage_contract::ErrorCode::LogCorrupt,
                "invalid log frame header",
            ));
        }
        let len = u32::from_le_bytes(
            header[8..LOG_HEADER_BYTES]
                .try_into()
                .expect("fixed header"),
        ) as usize;
        if len > MAX_FRAME {
            return Err(Error::new(
                storage_contract::ErrorCode::LogCorrupt,
                "log frame length exceeds limit",
            ));
        }
        let mut payload = vec![0; len];
        if read_fill_with_limit(&mut self.reader, &mut payload, maximum_read, observe)? != len {
            self.torn_tail = true;
            return Ok(None);
        }
        let mut digest = [0; 32];
        if read_fill_with_limit(&mut self.reader, &mut digest, maximum_read, observe)?
            != digest.len()
        {
            self.torn_tail = true;
            return Ok(None);
        }
        let mut h = blake3::Hasher::new();
        h.update(&header);
        h.update(&payload);
        if h.finalize().as_bytes() != &digest {
            return Err(Error::new(
                storage_contract::ErrorCode::LogCorrupt,
                "log frame checksum differs",
            ));
        }
        let record: Record = decode_strict(&payload)?;
        if canonical_json(&record)? != payload
            || record.previous_frame_hash != self.previous
            || record.sequence < self.last_sequence
        {
            return Err(Error::new(
                storage_contract::ErrorCode::LogCorrupt,
                "log chain or canonical encoding differs",
            ));
        }
        self.previous = blake3::Hash::from_bytes(digest).to_hex().to_string();
        self.last_sequence = record.sequence;
        self.valid_bytes += LOG_HEADER_BYTES as u64 + len as u64 + 32;
        Ok(Some((record, self.previous.clone())))
    }
}

fn scan_frames(reader: impl Read) -> Result<Scan> {
    let mut reader = JournalFrameReader::new(reader);
    let mut records = Vec::new();
    while let Some(record) = reader.next_record()? {
        records.push(record);
    }
    Ok(Scan {
        records,
        valid_bytes: reader.valid_bytes,
        torn_tail: reader.torn_tail,
    })
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum AppendMode {
    Live,
    Recovery,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WriterState {
    Ready,
    TornTail(u64),
    RecoveryRequired,
}

#[cfg(feature = "fault-injection")]
#[derive(Clone, Copy)]
pub enum JournalFault {
    PartialWrite,
    Sync,
}

/// The Engine owns this descriptor and its current decision-slot receipts.
pub(crate) struct JournalWriter {
    file: std::fs::File,
    records: Vec<(Record, String)>,
    state: WriterState,
    #[cfg(feature = "fault-injection")]
    pub(crate) fault: Option<JournalFault>,
}
#[cfg(feature = "fault-injection")]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpeningFault {
    FileSync,
    DirectorySync,
}

fn sync_open_journal(
    file: &std::fs::File,
    parent: &std::fs::File,
    #[cfg(feature = "fault-injection")] fault: Option<OpeningFault>,
) -> Result<()> {
    #[cfg(feature = "fault-injection")]
    if fault == Some(OpeningFault::FileSync) {
        return Err(std::io::Error::from_raw_os_error(5).into());
    }
    file.sync_data()?;
    #[cfg(feature = "fault-injection")]
    if fault == Some(OpeningFault::DirectorySync) {
        return Err(std::io::Error::from_raw_os_error(5).into());
    }
    parent.sync_all()?;
    Ok(())
}

fn journal_name() -> platform_io::fs_safety::EntryName {
    platform_io::fs_safety::EntryName::new("journal.bin")
        .expect("invariant: journal has one fixed entry name")
}

fn create_synced_journal(parent: &std::fs::File) -> Result<std::fs::File> {
    let file = create_journal(parent, &journal_name())?;
    sync_open_journal(
        &file,
        parent,
        #[cfg(feature = "fault-injection")]
        None,
    )?;
    Ok(file)
}
impl JournalWriter {
    #[cfg(test)]
    pub(crate) fn create(d: &DiskConfig) -> Result<Self> {
        Self::create_at(&DiskRoot::open(&d.root)?, d)
    }
    pub(crate) fn create_at(root: &DiskRoot, d: &DiskConfig) -> Result<Self> {
        let directory = root.directory(log_path(d).parent().expect("journal parent"))?;
        Ok(Self {
            file: create_synced_journal(&directory)?,
            records: Vec::new(),
            state: WriterState::Ready,
            #[cfg(feature = "fault-injection")]
            fault: None,
        })
    }
    pub(crate) fn records(&self) -> &[(Record, String)] {
        &self.records
    }
    pub(crate) fn requires_reopen(&self) -> bool {
        self.state == WriterState::RecoveryRequired
    }
    pub(crate) fn needs_recovery(&self) -> bool {
        self.state != WriterState::Ready
    }
    fn check_slot(&self, sequence: u64, candidate: &str) -> Result<()> {
        if self
            .records
            .last()
            .is_some_and(|(last, _)| last.sequence > sequence)
            || self.records.iter().any(|(record, _)| {
                record.sequence == sequence && record.candidate_hash != candidate
            })
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "journal decision slot conflicts with its tail",
            ));
        }
        Ok(())
    }
    pub(crate) fn repair_torn_tail(&mut self) -> Result<()> {
        match self.state {
            WriterState::Ready => Ok(()),
            WriterState::RecoveryRequired => Err(Error::new(
                ErrorCode::RecoveryRequired,
                "journal I/O failed; reopen for recovery",
            )),
            WriterState::TornTail(valid_bytes) => {
                self.state = WriterState::RecoveryRequired;
                self.file.set_len(valid_bytes)?;
                self.file.sync_data()?;
                self.state = WriterState::Ready;
                Ok(())
            }
        }
    }
    pub(crate) fn append(
        &mut self,
        d: &DiskConfig,
        kind: RecordType,
        sequence: u64,
        candidate: &str,
        object: &str,
    ) -> Result<Receipt> {
        self.append_mode(d, kind, sequence, candidate, object, AppendMode::Live)
    }
    pub(crate) fn append_recovered(
        &mut self,
        d: &DiskConfig,
        kind: RecordType,
        sequence: u64,
        candidate: &str,
        object: &str,
    ) -> Result<Receipt> {
        self.append_mode(d, kind, sequence, candidate, object, AppendMode::Recovery)
    }
    fn append_mode(
        &mut self,
        d: &DiskConfig,
        kind: RecordType,
        sequence: u64,
        candidate: &str,
        object: &str,
        mode: AppendMode,
    ) -> Result<Receipt> {
        if self.state == WriterState::RecoveryRequired
            || (mode == AppendMode::Live && self.needs_recovery())
        {
            return Err(Error::new(
                storage_contract::ErrorCode::RecoveryRequired,
                "journal I/O failed; reopen for recovery",
            ));
        }
        if let Some((record, hash)) = self.records.iter().find(|(record, _)| {
            record.sequence == sequence
                && record.record_type == kind
                && record.candidate_hash == candidate
                && record.object_hash == object
        }) {
            return Ok(Receipt {
                disk_id: d.disk_id.clone(),
                slot_id: d.slot_id.clone(),
                frame_hash: hash.clone(),
                record: record.clone(),
            });
        }
        self.check_slot(sequence, candidate)?;
        let record = Record {
            record_type: kind,
            sequence,
            candidate_hash: candidate.into(),
            object_hash: object.into(),
            previous_frame_hash: self
                .records
                .last()
                .map(|(_, hash)| hash.clone())
                .unwrap_or_else(|| "0".repeat(64)),
        };
        let payload = canonical_json(&record)?;
        let mut bytes = Vec::with_capacity(LOG_HEADER_BYTES + payload.len() + 32);
        bytes.extend(LOG_MAGIC);
        bytes.extend((payload.len() as u32).to_le_bytes());
        bytes.extend(payload);
        let hash = blake3::hash(&bytes);
        bytes.extend(hash.as_bytes());
        if mode == AppendMode::Recovery {
            self.repair_torn_tail()?;
        }
        // A partial write, failed sync, or unwind leaves the writer unusable until recovery.
        self.state = WriterState::RecoveryRequired;
        #[cfg(feature = "fault-injection")]
        let fault = self.fault.take();
        #[cfg(feature = "fault-injection")]
        if matches!(fault, Some(JournalFault::PartialWrite)) {
            self.file.write_all(&bytes[..bytes.len() / 2])?;
            return Err(std::io::Error::from_raw_os_error(28).into());
        }
        self.file.write_all(&bytes)?;
        #[cfg(feature = "fault-injection")]
        if matches!(fault, Some(JournalFault::Sync)) {
            return Err(std::io::Error::from_raw_os_error(5).into());
        }
        self.file.sync_data()?;
        if self
            .records
            .last()
            .is_some_and(|(last, _)| last.sequence != sequence)
        {
            self.records.clear();
        }
        let hash = hash.to_hex().to_string();
        self.records.push((record.clone(), hash.clone()));
        self.state = WriterState::Ready;
        Ok(Receipt {
            disk_id: d.disk_id.clone(),
            slot_id: d.slot_id.clone(),
            frame_hash: hash,
            record,
        })
    }
}

#[cfg(test)]
#[path = "../../../test/engine/unit/journal/test_support.rs"]
mod test_support;
#[cfg(test)]
pub(crate) use test_support::{
    append, candidate_catalog, map_addresses, map_pages, set_candidate_catalog, snapshot_addresses,
    store_object_replicas_bounded, stored_map,
};

mod object_storage;
#[cfg(any(test, feature = "test-support"))]
use object_storage::write_object_bytes;
use object_storage::write_object_bytes_at;
#[cfg(test)]
pub(crate) use object_storage::{existing_object_paths, list_object_hashes};
pub(crate) use object_storage::{existing_object_paths_at, list_object_hashes_at, object_path};
#[cfg(test)]
pub(crate) fn store_object<T: Serialize + ?Sized>(
    d: &DiskConfig,
    kind: &str,
    value: &T,
) -> Result<String> {
    let hash = object_hash(kind, value)?;
    write_object_bytes(d, kind, &hash, &encode_metadata(value, MAX_METADATA)?)?;
    Ok(hash)
}
pub(crate) fn store_object_at<T: Serialize + ?Sized>(
    root: &DiskRoot,
    d: &DiskConfig,
    kind: &str,
    value: &T,
) -> Result<String> {
    let hash = object_hash(kind, value)?;
    write_object_bytes_at(root, d, kind, &hash, &encode_metadata(value, MAX_METADATA)?)?;
    Ok(hash)
}
/// Frozen canonical bytes shared by size preflight and every replica write.
/// Construction performs all serialization and hashing; installation retains
/// atomic writes, immutable-byte collision checks and reader-size limits.
#[derive(Clone)]
pub(crate) struct PreparedMetadataObject {
    pub(crate) kind: &'static str,
    pub(crate) hash: String,
    pub(crate) bytes: Vec<u8>,
}
impl PreparedMetadataObject {
    pub(crate) fn new<T: Serialize + ?Sized>(
        kind: &'static str,
        value: &T,
        maximum: usize,
    ) -> Result<Self> {
        let (bytes, hash) = encode_hashed_object(kind, value)?;
        if bytes.len() > maximum {
            return Err(Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                format!("serialized metadata exceeds {maximum} byte reader limit"),
            ));
        }
        Ok(Self { kind, hash, bytes })
    }
    pub(crate) fn kind(&self) -> &'static str {
        self.kind
    }
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn store(&self, members: &[DiskConfig]) -> Result<String> {
        self.store_with(members, |disk| {
            write_object_bytes(disk, self.kind, &self.hash, &self.bytes)
        })
    }
    pub(crate) fn store_at(
        &self,
        resources: &DiskResources,
        members: &[DiskConfig],
    ) -> Result<String> {
        self.store_with(members, |disk| {
            write_object_bytes_at(
                &resources.get(disk)?.root,
                disk,
                self.kind,
                &self.hash,
                &self.bytes,
            )
        })
    }
    fn store_with(
        &self,
        members: &[DiskConfig],
        mut install: impl FnMut(&DiskConfig) -> Result<()>,
    ) -> Result<String> {
        if !members.is_empty() && self.bytes.len() > MAX_METADATA {
            return Err(Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                "serialized metadata exceeds reader limit",
            ));
        }
        for disk in members {
            install(disk)?;
        }
        Ok(self.hash.clone())
    }
}

/// An immutable JSON object authenticated against its referenced domain digest.
pub(crate) struct AuthenticatedMetadataObject {
    prepared: PreparedMetadataObject,
    value: json_codec::Value,
}
impl AuthenticatedMetadataObject {
    pub(crate) fn authenticate(
        kind: &'static str,
        hash: &str,
        value: json_codec::Value,
    ) -> Result<Self> {
        let (bytes, actual) = encode_hashed_value(kind, &value)?;
        if bytes.len() > MAX_METADATA {
            return Err(Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                "serialized metadata exceeds reader limit",
            ));
        }
        if actual != hash {
            return Err(Error::new(
                storage_contract::ErrorCode::ObjectCorrupt,
                "immutable object digest differs",
            ));
        }
        Ok(Self {
            prepared: PreparedMetadataObject {
                kind,
                hash: actual,
                bytes,
            },
            value,
        })
    }
    pub(crate) fn kind(&self) -> &'static str {
        self.prepared.kind
    }
    pub(crate) fn hash(&self) -> &str {
        &self.prepared.hash
    }
    pub(crate) fn value(&self) -> &json_codec::Value {
        &self.value
    }
    pub(crate) fn canonical_len(&self) -> usize {
        self.prepared.bytes.len()
    }
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.prepared.bytes
    }
    /// Test fixture: copy the prepared bytes for a second store target.
    #[cfg(test)]
    pub(crate) fn clone_prepared_fixture(&self) -> PreparedMetadataObject {
        self.prepared.clone()
    }
    pub(crate) fn into_prepared(self, maximum: usize) -> Result<PreparedMetadataObject> {
        if self.bytes().len() > maximum {
            return Err(Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                format!("serialized metadata exceeds {maximum} byte reader limit"),
            ));
        }
        Ok(self.prepared)
    }
}

#[cfg(test)]
pub(crate) fn load_authenticated_object(
    disk: &DiskConfig,
    kind: &'static str,
    hash: &str,
) -> Result<AuthenticatedMetadataObject> {
    AuthenticatedMetadataObject::authenticate(
        kind,
        hash,
        decode_strict(&object_storage::read_object_bytes_bounded(
            disk,
            kind,
            hash,
            MAX_METADATA,
        )?)?,
    )
}

pub(crate) fn load_authenticated_object_at(
    root: &DiskRoot,
    disk: &DiskConfig,
    kind: &'static str,
    hash: &str,
) -> Result<AuthenticatedMetadataObject> {
    AuthenticatedMetadataObject::authenticate(
        kind,
        hash,
        decode_strict(&object_storage::read_object_bytes_bounded_at(
            root,
            disk,
            kind,
            hash,
            MAX_METADATA,
        )?)?,
    )
}

pub(crate) fn load_authenticated_object_observed_at(
    root: &DiskRoot,
    disk: &DiskConfig,
    kind: &'static str,
    hash: &str,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<AuthenticatedMetadataObject> {
    AuthenticatedMetadataObject::authenticate(
        kind,
        hash,
        decode_strict(&object_storage::read_object_bytes_observed_at(
            root,
            disk,
            kind,
            hash,
            MAX_METADATA,
            observe,
        )?)?,
    )
}

#[cfg(test)]
pub(crate) fn load_object<T: data_serialization::de::DeserializeOwned + Serialize>(
    d: &DiskConfig,
    kind: &str,
    hash: &str,
) -> Result<T> {
    decode_object(
        kind,
        hash,
        &object_storage::read_object_bytes_bounded(d, kind, hash, MAX_METADATA)?,
    )
}

pub(crate) fn load_object_at<T: data_serialization::de::DeserializeOwned + Serialize>(
    root: &DiskRoot,
    d: &DiskConfig,
    kind: &str,
    hash: &str,
) -> Result<T> {
    decode_object(
        kind,
        hash,
        &object_storage::read_object_bytes_bounded_at(root, d, kind, hash, MAX_METADATA)?,
    )
}

pub(crate) fn load_object_observed_at<T: data_serialization::de::DeserializeOwned + Serialize>(
    root: &DiskRoot,
    d: &DiskConfig,
    kind: &str,
    hash: &str,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<T> {
    decode_object(
        kind,
        hash,
        &object_storage::read_object_bytes_observed_at(root, d, kind, hash, MAX_METADATA, observe)?,
    )
}

fn decode_object<T: data_serialization::de::DeserializeOwned + Serialize>(
    kind: &str,
    hash: &str,
    bytes: &[u8],
) -> Result<T> {
    let value = decode_strict(bytes)?;
    if object_hash(kind, &value)? != hash {
        return Err(Error::new(
            storage_contract::ErrorCode::ObjectCorrupt,
            "immutable object digest differs",
        ));
    }
    Ok(value)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Snapshot {
    pub format: crate::metadata_map::RootFormat,
    #[serialization(with = "storage_contract::u64_string")]
    pub sequence: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub business_generation: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub topology_generation: u64,
    pub maps: BTreeMap<String, crate::metadata_map::Reference>,
    pub events: crate::metadata_map::Reference,
    pub control_auth: ControlAuthReference,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
enum ControlAuthKind {
    #[serialization(rename = "state-control-auth")]
    StateControlAuth,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct ControlAuthReference {
    kind: ControlAuthKind,
    hash: String,
}
impl ControlAuthReference {
    pub(crate) fn hash(&self) -> &str {
        &self.hash
    }
    fn validate(&self) -> Result<()> {
        if self.hash.len() != 64
            || !self
                .hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "invalid control auth reference",
            ));
        }
        Ok(())
    }
}
#[cfg(test)]
pub(crate) const STATE_PAGE_ENTRIES: usize = 128;
#[cfg(test)]
pub(crate) fn store_snapshot(members: &[DiskConfig], state: &State) -> Result<String> {
    store_snapshot_bounded(members, state, MAX_METADATA)
}
#[cfg(test)]
pub(crate) fn store_snapshot_bounded(
    members: &[DiskConfig],
    state: &State,
    maximum: usize,
) -> Result<String> {
    let prepared = prepare_snapshot_fixture(state, maximum)?;
    prepared.store(members, state)
}
/// Test fixtures hand over published states whose one-shot cold index was
/// already consumed by the engine's publication. With no sealed segments the
/// complete locator index derives from the resident tail, keeping full-encode
/// helpers pure without a disk-read entry; the index never changes the
/// encoded bytes.
#[cfg(test)]
pub(crate) fn fixture_indexed_state(state: &State) -> Result<std::borrow::Cow<'_, State>> {
    if state.operations.cold_index().is_none() && state.operations.sealed().is_empty() {
        let mut indexed = state.clone();
        let index = indexed
            .operations
            .tail()
            .iter()
            .enumerate()
            .map(|(ordinal, entry)| (entry.result.operation_id, ordinal as u64))
            .collect();
        indexed.operations.attach_cold_index(index)?;
        return Ok(std::borrow::Cow::Owned(indexed));
    }
    Ok(std::borrow::Cow::Borrowed(state))
}
#[cfg(test)]
pub(crate) fn prepare_snapshot_fixture(state: &State, maximum: usize) -> Result<PreparedSnapshot> {
    prepare_snapshot(fixture_indexed_state(state)?.as_ref(), maximum)
}
/// Same-process published provenance. Each map root clone only increments an Arc.
#[derive(Clone)]
pub(crate) struct SnapshotReuse {
    pub(crate) descriptor: Snapshot,
    hash: String,
    fields: BTreeMap<String, snapshot::FieldRoot>,
    auth_bytes: usize,
}
pub(crate) struct PreparedSnapshot {
    pub(crate) metadata_integrity:
        Option<std::sync::Arc<crate::engine::provenance::MetadataIntegrity>>,
    pub(crate) descriptor: Snapshot,
    pub(crate) hash: String,
    pub(crate) objects: Vec<PreparedMetadataObject>,
    fields: BTreeMap<String, snapshot::FieldRoot>,
    auth_bytes: usize,
    maximum: usize,
}
#[path = "journal/snapshot.rs"]
mod snapshot;
pub(crate) use snapshot::validate_schema as validate_snapshot_schema;
#[path = "journal/snapshot_reuse.rs"]
mod snapshot_reuse;
pub(crate) fn prepare_snapshot(state: &State, maximum: usize) -> Result<PreparedSnapshot> {
    snapshot::prepare(state, maximum)
}
pub(crate) fn prepare_snapshot_incremental(
    before: &State,
    after: &State,
    basis: &SnapshotReuse,
    maximum: usize,
    reused: &mut dyn FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject>,
) -> Result<PreparedSnapshot> {
    snapshot::prepare_incremental(before, after, basis, maximum, reused)
}
#[cfg(test)]
pub(crate) use snapshot::no_reused_sources as no_reused_snapshot_sources;
#[cfg(test)]
pub(crate) fn test_reused_loader(
    disks: &[DiskConfig],
) -> impl FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject> + '_ {
    move |kind, hash| {
        let mut first = None;
        for disk in disks {
            match load_authenticated_object(disk, kind, hash) {
                Ok(object) => return Ok(object),
                Err(error) => {
                    first.get_or_insert(error);
                }
            }
        }
        Err(first.unwrap_or_else(|| {
            Error::new(ErrorCode::ObjectCorrupt, "published metadata has no source")
        }))
    }
}
#[cfg(test)]
fn prepare_snapshot_reference(state: &State, maximum: usize) -> Result<PreparedSnapshot> {
    snapshot::prepare_reference(state, maximum)
}
#[cfg(test)]
thread_local! { pub(crate) static SNAPSHOT_LOAD_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(crate) fn load_snapshot(d: &DiskConfig, hash: &str) -> Result<State> {
    let snapshot: Snapshot = load_object(d, "snapshot", hash)?;
    resolve_snapshot(snapshot, |kind, hash| {
        load_authenticated_object(d, kind, hash)
    })
}
#[cfg(test)]
fn load_raw_snapshot_fixture(d: &DiskConfig, hash: &str) -> Result<State> {
    let snapshot: Snapshot = load_object(d, "snapshot", hash)?;
    Ok(resolve_raw_snapshot(snapshot, |kind, hash| {
        load_authenticated_object(d, kind, hash)
    })?
    .into_codec_fixture())
}
#[cfg(test)]
pub(crate) fn resolve_snapshot(
    snapshot: Snapshot,
    load: impl FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject>,
) -> Result<State> {
    Ok(resolve_admitted_snapshot(snapshot, load)?.into_state())
}
pub(crate) fn resolve_admitted_snapshot(
    snapshot: Snapshot,
    mut load: impl FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject>,
) -> Result<crate::history_package::AdmittedState> {
    let raw = resolve_raw_snapshot(snapshot, &mut load)?;
    // Admission reads each commit's namespace through the same authenticated
    // object loader that produced the snapshot's own pages.
    let source = crate::namespace::ObjectLoaderSource::new(&mut load);
    crate::history_package::AdmittedState::admit(raw, &source)
}
fn resolve_raw_snapshot(
    snapshot: Snapshot,
    mut load: impl FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject>,
) -> Result<crate::history_package::RawState> {
    #[cfg(test)]
    SNAPSHOT_LOAD_COUNT.with(|count| count.set(count.get() + 1));
    validate_snapshot_schema(&snapshot)?;
    let auth = load("state-control-auth", snapshot.control_auth.hash())?;

    let mut out = json_codec::Map::new();
    out.insert("sequence".into(), snapshot.sequence.to_string().into());
    out.insert(
        "business_generation".into(),
        snapshot.business_generation.to_string().into(),
    );
    out.insert(
        "topology_generation".into(),
        snapshot.topology_generation.to_string().into(),
    );
    out.insert("control_auth".into(), auth.value().clone());
    let mut operation_log = None;
    let mut operation_locators = None;
    for (field, reference) in snapshot.maps {
        let map = crate::metadata_map::read_map(&reference, &mut load)?;
        match field.as_str() {
            "operation_log" => operation_log = Some(map),
            "operation_locators" => operation_locators = Some(map),
            _ => {
                out.insert(field, json_codec::Value::Object(map));
            }
        }
    }
    // The disk-read entry reads and verifies every sealed segment: format,
    // span/seal rules, contiguous directory keys, nondecreasing sequences
    // within and across segments, and recorded placement facts against the
    // decoded bodies. Only the directory and the open tail stay resident.
    let events = resolve_event_segments(
        crate::metadata_map::read_map(&snapshot.events, &mut load)?,
        snapshot.sequence,
        &mut load,
    )?;
    out.insert("events".into(), events);
    let operations = resolve_operation_segments(
        operation_log.expect("invariant: the validated schema carries operation_log"),
        operation_locators.expect("invariant: the validated schema carries operation_locators"),
        snapshot.sequence,
        &mut load,
    )?;
    out.insert("operations".into(), operations);
    Ok(json_codec::from_value(json_codec::Value::Object(out))?)
}

fn directory_entries<T: data_serialization::de::DeserializeOwned>(
    map: json_codec::Map<String, json_codec::Value>,
    what: &'static str,
) -> Result<Vec<T>> {
    let mut entries = Vec::with_capacity(map.len());
    for (index, (key, value)) in map.into_iter().enumerate() {
        if key != format!("{index:020}") {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "segment directory ordinals are not contiguous",
            ));
        }
        entries.push(
            json_codec::from_value(value)
                .map_err(|source| Error::from(source).context(ErrorCode::ObjectCorrupt, what))?,
        );
    }
    Ok(entries)
}

/// Visit every sealed/tail segment object referenced by this snapshot's
/// events and operation_log directories, verifying each body against its
/// typed directory entry. Shape-level: full cross-segment admission belongs
/// to the single disk-read entry.
pub(crate) fn visit_segment_objects(
    snapshot: &Snapshot,
    mut load: impl FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject>,
) -> Result<()> {
    use storage_contract::{EVENT_SEGMENT_KIND, EVENT_SEGMENT_SPAN, OPERATION_SEGMENT_KIND};
    let events = crate::metadata_map::read_map(&snapshot.events, &mut load)?;
    for entry in directory_entries::<storage_contract::EventDirectoryEntry>(
        events,
        "invalid event directory entry",
    )? {
        let segment: storage_contract::EventSegment =
            json_codec::from_value(load(EVENT_SEGMENT_KIND, &entry.hash)?.value().clone())?;
        if segment.events.is_empty()
            || segment.events.len() > EVENT_SEGMENT_SPAN
            || segment.events[0].sequence != entry.first_sequence
        {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "event segment differs from its directory entry",
            ));
        }
    }
    let log = snapshot
        .maps
        .get("operation_log")
        .ok_or_else(|| Error::new(ErrorCode::ObjectCorrupt, "snapshot map schema differs"))?;
    let log = crate::metadata_map::read_map(log, &mut load)?;
    for entry in directory_entries::<storage_contract::OperationDirectoryEntry>(
        log,
        "invalid operation directory entry",
    )? {
        let segment: storage_contract::OperationSegment =
            json_codec::from_value(load(OPERATION_SEGMENT_KIND, &entry.hash)?.value().clone())?;
        if segment.records.is_empty()
            || segment.records.len() as u64 != entry.records
            || segment.records[0].result.sequence != entry.first_sequence
        {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "operation segment differs from its directory entry",
            ));
        }
    }
    Ok(())
}

/// Visit every namespace page/index reachable from one snapshot's commits, for
/// scrub coverage. Commit values reference immutable namespace graphs that the
/// radix page walk stores but never decodes; each object is loaded once.
pub(crate) fn visit_commit_namespace_objects(
    snapshot: &Snapshot,
    mut load: impl FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject>,
) -> Result<()> {
    let commits = snapshot
        .maps
        .get("commits")
        .ok_or_else(|| Error::new(ErrorCode::ObjectCorrupt, "snapshot map schema differs"))?;
    let commits = crate::metadata_map::read_map(commits, &mut load)?;
    let mut visited = std::collections::BTreeSet::new();
    for (commit_id, value) in &commits {
        let namespace: storage_contract::CommitNamespace =
            json_codec::from_value(value["namespace"].clone()).map_err(|error| {
                Error::from(error).context(
                    ErrorCode::ObjectCorrupt,
                    format!("commit {commit_id} has no valid namespace root"),
                )
            })?;
        crate::namespace::visit_namespace_objects(&namespace.root, &mut load, &mut visited)?;
    }
    Ok(())
}

fn resolve_event_segments(
    directory: json_codec::Map<String, json_codec::Value>,
    published_sequence: u64,
    load: &mut impl FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject>,
) -> Result<json_codec::Value> {
    use storage_contract::{EVENT_SEGMENT_KIND, EVENT_SEGMENT_SPAN};
    let entries: Vec<storage_contract::EventDirectoryEntry> =
        directory_entries(directory, "invalid event directory entry")?;
    let mut sealed = Vec::new();
    let mut tail: Vec<storage_contract::Event> = Vec::new();
    let mut previous_sequence = None::<u64>;
    let count = entries.len();
    for (index, entry) in entries.into_iter().enumerate() {
        let segment: storage_contract::EventSegment =
            json_codec::from_value(load(EVENT_SEGMENT_KIND, &entry.hash)?.value().clone())?;
        let last = index + 1 == count;
        if segment.events.is_empty()
            || segment.events.len() > EVENT_SEGMENT_SPAN
            || (!last && segment.events.len() != EVENT_SEGMENT_SPAN)
        {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "event segment span differs from the seal rule",
            ));
        }
        if segment.events[0].sequence != entry.first_sequence
            || !segment
                .events
                .iter()
                .map(|event| event.sequence)
                .is_sorted()
            || previous_sequence.is_some_and(|previous| segment.events[0].sequence < previous)
        {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "event segment sequences are out of order",
            ));
        }
        previous_sequence = segment.events.last().map(|event| event.sequence);
        if previous_sequence.is_some_and(|sequence| sequence > published_sequence) {
            return Err(Error::new(
                ErrorCode::StateInvalid,
                "snapshot event sequence exceeds published sequence",
            ));
        }
        if last && segment.events.len() < EVENT_SEGMENT_SPAN {
            tail = segment.events;
        } else {
            sealed.push(storage_contract::EventSegmentRef {
                hash: entry.hash,
                first_sequence: entry.first_sequence,
            });
        }
    }
    Ok(json_codec::json!({
        "sealed": json_codec::to_value(&sealed)?,
        "tail": json_codec::to_value(&tail)?,
    }))
}

fn resolve_operation_segments(
    directory: json_codec::Map<String, json_codec::Value>,
    locators: json_codec::Map<String, json_codec::Value>,
    published_sequence: u64,
    load: &mut impl FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject>,
) -> Result<json_codec::Value> {
    use storage_contract::{
        OPERATION_SEGMENT_KIND, OPERATION_SEGMENT_RECORDS, OPERATION_SEGMENT_TARGET_BYTES,
    };
    let entries: Vec<storage_contract::OperationDirectoryEntry> =
        directory_entries(directory, "invalid operation directory entry")?;
    let mut sealed = Vec::new();
    let mut tail: Vec<storage_contract::OperationRecordEntry> = Vec::new();
    let mut next_ordinal = 0u64;
    let mut previous_sequence = None::<u64>;
    let mut index_map = std::collections::BTreeMap::new();
    let count = entries.len();
    for (position, entry) in entries.into_iter().enumerate() {
        let segment: storage_contract::OperationSegment =
            json_codec::from_value(load(OPERATION_SEGMENT_KIND, &entry.hash)?.value().clone())?;
        if segment.records.is_empty()
            || segment.records.len() as u64 != entry.records
            || entry.first_ordinal != next_ordinal
            || segment.records[0].result.sequence != entry.first_sequence
        {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "operation segment differs from its directory entry",
            ));
        }
        if !segment
            .records
            .iter()
            .map(|record| record.result.sequence)
            .is_sorted()
            || previous_sequence
                .is_some_and(|previous| segment.records[0].result.sequence < previous)
        {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "operation segment sequences are out of order",
            ));
        }
        let last = position + 1 == count;
        // A writer seals on the first append that reaches either threshold.
        // No body may include records after that first hit, even the last one.
        let mut bytes = 0usize;
        let mut full = false;
        for (offset, record) in segment.records.iter().enumerate() {
            if full {
                return Err(Error::new(
                    ErrorCode::ObjectCorrupt,
                    "operation segment continues beyond the seal rule",
                ));
            }
            bytes += storage_contract::canonical_json(record)?.len();
            full =
                offset + 1 == OPERATION_SEGMENT_RECORDS || bytes >= OPERATION_SEGMENT_TARGET_BYTES;
        }
        if !last && !full {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "operation segment ends before the seal rule",
            ));
        }
        for (offset, record) in segment.records.iter().enumerate() {
            if index_map
                .insert(record.result.operation_id, next_ordinal + offset as u64)
                .is_some()
            {
                return Err(Error::new(
                    ErrorCode::ObjectCorrupt,
                    "operation id appears in two receipts",
                ));
            }
        }
        previous_sequence = segment.records.last().map(|record| record.result.sequence);
        if previous_sequence.is_some_and(|sequence| sequence > published_sequence) {
            return Err(Error::new(
                ErrorCode::StateInvalid,
                "snapshot operation sequence exceeds published sequence",
            ));
        }
        next_ordinal += entry.records;
        if last && !full {
            tail = segment.records;
        } else {
            sealed.push(entry.reference());
        }
    }
    // The locator map is the published id index. A complete read must agree
    // exactly with the index derived from the receipt bodies.
    if locators.len() != index_map.len() {
        return Err(Error::new(
            ErrorCode::ObjectCorrupt,
            "operation locators differ from the receipt log",
        ));
    }
    for (key, value) in locators {
        let id: storage_contract::OperationId = key.parse()?;
        let ordinal =
            storage_contract::parse_u64_decimal(&json_codec::from_value::<String>(value)?)?;
        if index_map.get(&id) != Some(&ordinal) {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "operation locators differ from the receipt log",
            ));
        }
    }
    let index = index_map
        .into_iter()
        .map(|(id, ordinal)| (id.to_string(), ordinal.to_string()))
        .collect::<std::collections::BTreeMap<_, _>>();
    Ok(json_codec::json!({
        "sealed": json_codec::to_value(&sealed)?,
        "tail": json_codec::to_value(&tail)?,
        "index": json_codec::to_value(&index)?,
    }))
}

#[cfg(test)]
#[path = "../../../test/engine/unit/journal/segmented_admission_tests.rs"]
mod segmented_admission_tests;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct VersionEvidence {
    pub version: FileVersion,
    pub placement: Placement,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Candidate {
    pub pool_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub sequence: u64,
    pub transaction_id: String,
    pub operation_id: OperationId,
    pub request_digest: String,
    pub previous_snapshot: Option<String>,
    pub snapshot_hash: String,
    pub versions: Vec<VersionEvidence>,
    pub metadata_disk_ids: Vec<String>,
    pub history_roots: crate::metadata_map::Reference,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Finalize {
    pub candidate_hash: String,
    pub accepts: Vec<Receipt>,
    #[serialization(default, skip_serializing_if = "Option::is_none")]
    pub recovery_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct PayloadReceipt {
    pub version_id: String,
    pub disk_id: String,
    pub slot_id: String,
    pub proof_hash: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct CommitProof {
    pub candidate_hash: String,
    pub finalize: Finalize,
    pub version_hash: String,
    pub disk_id: String,
    pub slot_id: String,
    pub route_slot_ids: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Publication {
    pub candidate_hash: String,
    pub finalize: Finalize,
    pub finalize_receipts: Vec<Receipt>,
    pub payload_receipts: Vec<PayloadReceipt>,
    pub result: OperationResult,
    #[serialization(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<crate::degraded_recovery::RecoveryProof>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct PublicationSeal {
    pub candidate_hash: String,
    pub publication_hash: String,
    pub receipts: Vec<Receipt>,
}
pub(crate) fn publication_seal_path(d: &DiskConfig, candidate: &str) -> PathBuf {
    d.root
        .join("metadata/publication-seals")
        .join(format!("{candidate}.json"))
}
pub(crate) fn verify_publication_seal(
    c: &Candidate,
    p: &Publication,
    seal: &PublicationSeal,
) -> Result<()> {
    let candidate = object_hash("candidate", c)?;
    let publication = object_hash("publication", p)?;
    let finalize = object_hash("finalize", &p.finalize)?;
    let mut ids = c.metadata_disk_ids.clone();
    ids.sort();
    ids.dedup();
    let mut actual = seal
        .receipts
        .iter()
        .map(|r| r.disk_id.clone())
        .collect::<Vec<_>>();
    actual.sort();
    actual.dedup();
    let member_ids = |receipts: &[Receipt]| {
        let mut out = receipts
            .iter()
            .map(|r| r.disk_id.clone())
            .collect::<Vec<_>>();
        out.sort();
        out.dedup();
        out
    };
    if ids.is_empty()
        || c.metadata_disk_ids.len() != ids.len()
        || actual != ids
        || (p.recovery.is_none()
            && (member_ids(&p.finalize.accepts) != ids || p.finalize.accepts.len() != ids.len()))
        || member_ids(&p.finalize_receipts) != ids
        || p.finalize_receipts.len() != ids.len()
        || seal.receipts.len() != ids.len()
        || seal.candidate_hash != candidate
        || seal.publication_hash != publication
        || p.candidate_hash != candidate
        || p.finalize.candidate_hash != candidate
        || p.result.operation_id != c.operation_id
        || p.result.request_digest != c.request_digest
        || p.result.sequence != c.sequence
        || (p.recovery.is_none()
            && (p.result.commit_state != storage_contract::CommitState::Published
                || p.finalize.recovery_id.is_some()))
    {
        return Err(Error::new(
            storage_contract::ErrorCode::PublicationInvalid,
            "synchronized publication receipts from every configured member are required",
        ));
    }
    if let Some(recovery) = &p.recovery {
        recovery.verify(c, p)?;
    }
    let mut slots = std::collections::BTreeSet::new();
    for r in &seal.receipts {
        let f = p
            .finalize_receipts
            .iter()
            .find(|f| f.disk_id == r.disk_id)
            .expect("member sets checked");
        let predecessor = if let Some(recovery) = &p.recovery {
            let fence = recovery
                .reprotect
                .iter()
                .find(|fence| fence.disk_id == r.disk_id)
                .expect("recovery member sets checked");
            if fence.slot_id != r.slot_id {
                return Err(Error::new(
                    storage_contract::ErrorCode::PublicationInvalid,
                    "recovery fence slot differs",
                ));
            }
            fence.frame_hash.as_str()
        } else {
            let accept = p
                .finalize
                .accepts
                .iter()
                .find(|a| a.disk_id == r.disk_id)
                .expect("member sets checked");
            if accept.slot_id != r.slot_id
                || !receipt_binds(
                    accept,
                    RecordType::Accept,
                    c.sequence,
                    &candidate,
                    &candidate,
                )?
            {
                return Err(Error::new(
                    storage_contract::ErrorCode::PublicationInvalid,
                    "ACCEPT receipt differs",
                ));
            }
            accept.frame_hash.as_str()
        };
        if !slots.insert(r.slot_id.clone())
            || r.slot_id != f.slot_id
            || f.record.previous_frame_hash != predecessor
            || r.record.previous_frame_hash != f.frame_hash
            || !receipt_binds(f, RecordType::Finalize, c.sequence, &candidate, &finalize)?
        {
            return Err(Error::new(
                storage_contract::ErrorCode::PublicationInvalid,
                "member synchronized chain is invalid",
            ));
        }
        if !receipt_binds(
            r,
            RecordType::Publication,
            c.sequence,
            &candidate,
            &publication,
        )? {
            return Err(Error::new(
                storage_contract::ErrorCode::PublicationInvalid,
                "publication frame receipt is invalid",
            ));
        }
    }
    Ok(())
}
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct LocalPublication {
    pub candidate: Candidate,
    pub publication: Publication,
    pub commit_proof: CommitProof,
    pub publication_receipts: Vec<Receipt>,
}
/// One serialization schema for owned reads and borrowed publication writes.
#[derive(Serialize)]
#[serialization(rename = "LocalPublication")]
pub(crate) struct LocalPublicationRef<'a> {
    pub candidate: &'a Candidate,
    pub publication: &'a Publication,
    pub commit_proof: &'a CommitProof,
    pub publication_receipts: &'a [Receipt],
}
impl Serialize for LocalPublication {
    fn serialize<S: data_serialization::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        let Self {
            candidate,
            publication,
            commit_proof,
            publication_receipts,
        } = self;
        LocalPublicationRef {
            candidate,
            publication,
            commit_proof,
            publication_receipts,
        }
        .serialize(serializer)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct ContentTombstone {
    pub version_id: String,
    pub state: ContentState,
    pub candidate: Candidate,
    pub publication: Publication,
    pub publication_seal: PublicationSeal,
    pub snapshot: Snapshot,
    pub content_state_proof: crate::metadata_map::InclusionProof,
}
fn read_local_json<T: data_serialization::de::DeserializeOwned>(
    path: &Path,
    read: &mut impl FnMut(&Path) -> Result<Vec<u8>>,
) -> Result<T> {
    decode_strict(&read(path)?).map_err(Into::into)
}

/// Offline receipts must bind the saved pool membership, not a smaller set
/// declared by their own candidate. Verification never creates a missing anchor.
fn verify_local_membership(
    root: &Path,
    candidate: &Candidate,
    seal: &DiskSeal,
    read: &mut impl FnMut(&Path) -> Result<Vec<u8>>,
    metadata: Option<&crate::engine::provenance::MetadataReadContext<'_>>,
) -> Result<()> {
    let loaded =
        read_local_json::<Config>(&root.join("metadata/pool-membership/config.json"), read)
            .and_then(ValidatedConfig::try_from);
    let config = match metadata {
        Some(metadata) => metadata.membership(loaded)?,
        None => loaded?,
    };
    let expected = config
        .disks
        .iter()
        .filter(|d| d.metadata_member)
        .map(|d| d.disk_id.clone())
        .collect::<BTreeSet<_>>();
    let actual = candidate
        .metadata_disk_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if config.pool_id != candidate.pool_id
        || seal.pool_id != config.pool_id
        || actual != expected
        || actual.len() != candidate.metadata_disk_ids.len()
        || !config.disks.iter().any(|d| {
            d.disk_id == seal.disk_id
                && d.slot_id == seal.slot_id
                && d.binding_generation == seal.binding_generation
        })
    {
        return Err(Error::new(
            storage_contract::ErrorCode::PublicationInvalid,
            "local proof differs from saved pool membership",
        ));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn verify_tombstone(directory: &Path, version: &FileVersion) -> Result<()> {
    let tombstone: ContentTombstone = read_json(&directory.join("purged.json"))?;
    verify_tombstone_content_in_context(directory, version, &tombstone, None)
}

pub(crate) fn verify_tombstone_content_in_context(
    directory: &Path,
    version: &FileVersion,
    t: &ContentTombstone,
    metadata: Option<&crate::engine::provenance::MetadataReadContext<'_>>,
) -> Result<()> {
    verify_tombstone_content_with(
        directory,
        version,
        t,
        &mut |path| read_bytes(path, MAX_METADATA),
        metadata,
    )
}

pub(crate) fn verify_tombstone_content_at(
    root: &DiskRoot,
    directory: &Path,
    version: &FileVersion,
    t: &ContentTombstone,
    observe: &mut impl FnMut(usize) -> Result<()>,
    metadata: Option<&crate::engine::provenance::MetadataReadContext<'_>>,
) -> Result<()> {
    verify_tombstone_content_with(
        directory,
        version,
        t,
        &mut |path| root.read_bytes_observed(path, MAX_METADATA, observe),
        metadata,
    )
}

fn verify_tombstone_content_with(
    directory: &Path,
    version: &FileVersion,
    t: &ContentTombstone,
    read: &mut impl FnMut(&Path) -> Result<Vec<u8>>,
    metadata: Option<&crate::engine::provenance::MetadataReadContext<'_>>,
) -> Result<()> {
    // Intentional removal proves the content state, not the retained descriptor.
    // Its immutable identity and location must still bind to the local publication.
    verify_local_version_publication(directory, version, read, metadata)?;
    verify_publication_seal(&t.candidate, &t.publication, &t.publication_seal)?;
    let root = directory.ancestors().nth(4).ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::PublicationInvalid,
            "invalid version location",
        )
    })?;
    let loaded = read_local_json(&root.join("disk.json"), read);
    let seal: DiskSeal = match metadata {
        Some(metadata) => metadata.seal(loaded)?,
        None => loaded?,
    };
    verify_local_membership(root, &t.candidate, &seal, read, metadata)?;
    validate_snapshot_schema(&t.snapshot)?;
    let content_state = t
        .content_state_proof
        .verify(&t.snapshot.maps["content_states"], &t.version_id)?;
    if t.version_id != version.version_id
        || !matches!(t.state, ContentState::Purged | ContentState::Migrated)
        || t.candidate.pool_id != seal.pool_id
        || t.publication.candidate_hash != object_hash("candidate", &t.candidate)?
        || object_hash("snapshot", &t.snapshot)? != t.candidate.snapshot_hash
        || t.snapshot.sequence != t.candidate.sequence
        || content_state.as_ref().and_then(json_codec::Value::as_str) != Some(t.state.as_str())
    {
        return Err(Error::new(
            storage_contract::ErrorCode::TombstoneInvalid,
            "content tombstone is not covered by a published state",
        ));
    }
    Ok(())
}

/// A later protection transaction reuses immutable FULL identities. Preserve
/// the first locally published proof and retain each later receipt by candidate.
pub(crate) fn local_proof_directory_at(
    root: &DiskRoot,
    directory: &Path,
    candidate: &str,
) -> Result<PathBuf> {
    let original = directory.join("commit-proof.json");
    match root.read_json_observed::<CommitProof>(&original, &mut |_| Ok(())) {
        Ok(proof) => {
            if proof.candidate_hash != candidate {
                return Ok(directory.join("protection-proofs").join(candidate));
            }
        }
        Err(Error::Filesystem(platform_io::fs_safety::Error::Io(error)))
        | Err(Error::Contract(storage_contract::Error::Io { source: error, .. }))
            if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    Ok(directory.to_owned())
}

pub(crate) fn verify_local_publication(directory: &Path, version: &FileVersion) -> Result<()> {
    verify_local_publication_in_context(directory, version, None)
}

pub(crate) fn verify_local_publication_in_context(
    directory: &Path,
    version: &FileVersion,
    metadata: Option<&crate::engine::provenance::MetadataReadContext<'_>>,
) -> Result<()> {
    verify_local_publication_observed(directory, version, &mut |_| Ok(()), metadata)
}

pub(crate) fn verify_local_publication_observed(
    directory: &Path,
    version: &FileVersion,
    observe: &mut impl FnMut(usize) -> Result<()>,
    metadata: Option<&crate::engine::provenance::MetadataReadContext<'_>>,
) -> Result<()> {
    let directory = absolute_path(directory)?;
    let root = directory.ancestors().nth(4).ok_or_else(|| {
        Error::new(
            ErrorCode::PublicationInvalid,
            "version is outside disk layout",
        )
    })?;
    let root = DiskRoot::open(root)?;
    verify_local_publication_at(&root, &directory, version, observe, metadata)
}

pub(crate) fn verify_local_publication_at(
    root: &DiskRoot,
    directory: &Path,
    version: &FileVersion,
    observe: &mut impl FnMut(usize) -> Result<()>,
    metadata: Option<&crate::engine::provenance::MetadataReadContext<'_>>,
) -> Result<()> {
    verify_local_publication_with(
        directory,
        version,
        &mut |path| root.read_bytes_observed(path, MAX_METADATA, observe),
        &mut |path| root.open_optional_regular(path).map(|file| file.is_some()),
        metadata,
    )
}

fn verify_local_publication_with(
    directory: &Path,
    version: &FileVersion,
    read: &mut impl FnMut(&Path) -> Result<Vec<u8>>,
    present: &mut impl FnMut(&Path) -> Result<bool>,
    metadata: Option<&crate::engine::provenance::MetadataReadContext<'_>>,
) -> Result<()> {
    verify_local_version_publication(directory, version, read, metadata)?;
    if present(&directory.join("purged.json"))? {
        return Err(Error::new(
            storage_contract::ErrorCode::ContentPurged,
            "this version has a local content tombstone",
        ));
    }
    Ok(())
}

/// Verify immutable publication identity independently of current availability.
fn verify_local_version_publication(
    directory: &Path,
    version: &FileVersion,
    read: &mut impl FnMut(&Path) -> Result<Vec<u8>>,
    metadata: Option<&crate::engine::provenance::MetadataReadContext<'_>>,
) -> Result<()> {
    if directory.file_name().and_then(|s| s.to_str()) != Some(version.version_id.as_str())
        || directory
            .ancestors()
            .nth(1)
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            != Some("versions")
        || directory
            .ancestors()
            .nth(2)
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            != Some(version.history_tree_id.as_str())
        || directory
            .ancestors()
            .nth(3)
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            != Some("histories")
    {
        return Err(Error::new(
            storage_contract::ErrorCode::PublicationInvalid,
            "version directory identity differs from its descriptor",
        ));
    }
    let local: LocalPublication = read_local_json(&directory.join("publication-proof.json"), read)?;
    let proof: CommitProof = read_local_json(&directory.join("commit-proof.json"), read)?;
    let candidate_hash = object_hash("candidate", &local.candidate)?;
    let c = &local.candidate;
    let p = &local.publication;
    verify_publication_seal(
        c,
        p,
        &PublicationSeal {
            candidate_hash: candidate_hash.clone(),
            publication_hash: object_hash("publication", p)?,
            receipts: local.publication_receipts.clone(),
        },
    )?;
    let root = directory.ancestors().nth(4).ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::PublicationInvalid,
            "version is outside disk layout",
        )
    })?;
    let loaded = read_local_json(&root.join("disk.json"), read);
    let seal: DiskSeal = match metadata {
        Some(metadata) => metadata.seal(loaded)?,
        None => loaded?,
    };
    verify_local_membership(root, c, &seal, read, metadata)?;
    let finalize_hash = object_hash("finalize", &p.finalize)?;
    if p.candidate_hash != candidate_hash
        || proof.candidate_hash != candidate_hash
        || object_hash("commit-proof", &proof)? != object_hash("commit-proof", &local.commit_proof)?
        || proof.version_hash != object_hash("version", version)?
        || seal.pool_id != c.pool_id
        || seal.disk_id != proof.disk_id
        || seal.slot_id != proof.slot_id
        || object_hash("finalize", &proof.finalize)? != finalize_hash
    {
        return Err(Error::new(
            storage_contract::ErrorCode::PublicationInvalid,
            "local proof does not bind the version",
        ));
    }
    let evidence = c
        .versions
        .iter()
        .find(|e| e.version.version_id == version.version_id)
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::PublicationInvalid,
                "version is absent from candidate",
            )
        })?;
    if object_hash("version", &evidence.version)? != object_hash("version", version)?
        || evidence.placement.route_slot_ids != proof.route_slot_ids
    {
        return Err(Error::new(
            storage_contract::ErrorCode::PublicationInvalid,
            "candidate placement mismatch",
        ));
    }
    let receipt = p
        .payload_receipts
        .iter()
        .find(|r| {
            r.version_id == version.version_id
                && r.disk_id == proof.disk_id
                && r.slot_id == proof.slot_id
        })
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::PublicationInvalid,
                "local payload receipt absent",
            )
        })?;
    if receipt.proof_hash != object_hash("commit-proof", &proof)? {
        return Err(Error::new(
            storage_contract::ErrorCode::PublicationInvalid,
            "local receipt digest differs",
        ));
    }
    for slot in &evidence.placement.route_slot_ids {
        if !p
            .payload_receipts
            .iter()
            .any(|r| r.version_id == version.version_id && &r.slot_id == slot)
        {
            return Err(Error::new(
                storage_contract::ErrorCode::PublicationInvalid,
                "a required route receipt is absent",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../test/engine/unit/journal/replication_tests.rs"]
mod replication_tests;

#[cfg(test)]
#[path = "../../../test/engine/unit/journal/writer_lifecycle_tests.rs"]
mod writer_lifecycle_tests;

#[cfg(test)]
#[path = "../../../test/engine/unit/journal/scanner_tests.rs"]
mod scanner_tests;

#[cfg(test)]
#[path = "../../../test/engine/unit/journal/writer_tests.rs"]
mod writer_tests;

#[cfg(test)]
#[path = "../../../test/engine/unit/journal/snapshot_partition_tests.rs"]
mod snapshot_partition_tests;

#[cfg(test)]
#[path = "../../../test/engine/unit/journal/snapshot_incremental_oracle_tests.rs"]
mod snapshot_incremental_oracle_tests;
