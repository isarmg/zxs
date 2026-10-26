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
pub(crate) fn scan(d: &DiskConfig) -> Result<Scan> {
    match open_optional_regular(&log_path(d))? {
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
    file: Option<std::fs::File>,
    pub scan: Scan,
    #[cfg(feature = "fault-injection")]
    opening_fault: Option<OpeningFault>,
}
impl OpenJournal {
    pub(crate) fn scan(d: &DiskConfig) -> Result<Self> {
        let file = open_existing_journal(&log_path(d))?;
        let scan = match &file {
            Some(file) => scan_frames(BufReader::new(file))?,
            None => Scan {
                records: Vec::new(),
                valid_bytes: 0,
                torn_tail: false,
            },
        };
        Ok(Self {
            file,
            scan,
            #[cfg(feature = "fault-injection")]
            opening_fault: None,
        })
    }
    pub(crate) fn into_writer(self, d: &DiskConfig) -> Result<JournalWriter> {
        let file = match self.file {
            Some(file) => {
                // A surviving leaf may come from a failed creation before its directory fence.
                sync_open_journal(
                    &file,
                    log_path(d).parent().expect("journal parent"),
                    #[cfg(feature = "fault-injection")]
                    self.opening_fault,
                )?;
                file
            }
            None => create_synced_journal(d)?,
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

fn scan_frames(mut reader: impl Read) -> Result<Scan> {
    let mut out = Scan {
        records: Vec::new(),
        valid_bytes: 0,
        torn_tail: false,
    };
    let mut previous = "0".repeat(64);
    let mut last_sequence = 0;
    loop {
        let mut header = [0u8; LOG_HEADER_BYTES];
        let mut got = 0;
        while got < LOG_HEADER_BYTES {
            let n = reader.read(&mut header[got..])?;
            if n == 0 {
                break;
            }
            got += n;
        }
        if got == 0 {
            break;
        }
        if got < LOG_HEADER_BYTES {
            out.torn_tail = true;
            break;
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
        if let Err(e) = reader.read_exact(&mut payload) {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                out.torn_tail = true;
                break;
            }
            return Err(e.into());
        }
        let mut digest = [0; 32];
        if let Err(e) = reader.read_exact(&mut digest) {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                out.torn_tail = true;
                break;
            }
            return Err(e.into());
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
            || record.previous_frame_hash != previous
            || record.sequence < last_sequence
        {
            return Err(Error::new(
                storage_contract::ErrorCode::LogCorrupt,
                "log chain or canonical encoding differs",
            ));
        }
        previous = blake3::Hash::from_bytes(digest).to_hex().to_string();
        last_sequence = record.sequence;
        out.records.push((record, previous.clone()));
        out.valid_bytes += LOG_HEADER_BYTES as u64 + len as u64 + 32;
    }
    Ok(out)
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
    parent: &Path,
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
    sync_dir(parent)
}

fn create_synced_journal(d: &DiskConfig) -> Result<std::fs::File> {
    let path = log_path(d);
    let file = create_journal(&path)?;
    sync_open_journal(
        &file,
        path.parent().expect("journal parent"),
        #[cfg(feature = "fault-injection")]
        None,
    )?;
    Ok(file)
}
impl JournalWriter {
    pub(crate) fn create(d: &DiskConfig) -> Result<Self> {
        Ok(Self {
            file: create_synced_journal(d)?,
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
pub(crate) use test_support::{append, store_object_replicas_bounded};

mod object_storage;
pub(crate) use object_storage::{existing_object_paths, list_object_hashes, object_path};
use object_storage::{read_object_bytes, write_object_bytes};
pub(crate) fn store_object<T: Serialize + ?Sized>(
    d: &DiskConfig,
    kind: &str,
    value: &T,
) -> Result<String> {
    let hash = object_hash(kind, value)?;
    write_object_bytes(d, kind, &hash, &encode_metadata(value, MAX_METADATA)?)?;
    Ok(hash)
}
/// Frozen canonical bytes shared by size preflight and every replica write.
/// Construction performs all serialization and hashing; installation retains
/// atomic writes, immutable-byte collision checks and reader-size limits.
pub(crate) struct PreparedMetadataObject {
    kind: &'static str,
    pub(crate) hash: String,
    bytes: Vec<u8>,
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
    pub(crate) fn store(&self, members: &[DiskConfig]) -> Result<String> {
        if !members.is_empty() && self.bytes.len() > MAX_METADATA {
            return Err(Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                "serialized metadata exceeds reader limit",
            ));
        }
        for disk in members {
            write_object_bytes(disk, self.kind, &self.hash, &self.bytes)?;
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
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.prepared.bytes
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

pub(crate) fn load_authenticated_object(
    disk: &DiskConfig,
    kind: &'static str,
    hash: &str,
) -> Result<AuthenticatedMetadataObject> {
    AuthenticatedMetadataObject::authenticate(
        kind,
        hash,
        decode_strict(&read_object_bytes(disk, kind, hash)?)?,
    )
}

pub(crate) fn load_object<T: data_serialization::de::DeserializeOwned + Serialize>(
    d: &DiskConfig,
    kind: &str,
    hash: &str,
) -> Result<T> {
    let value = decode_strict(&read_object_bytes(d, kind, hash)?)?;
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
    #[serialization(with = "storage_contract::u64_string")]
    pub sequence: u64,
    pub pages: BTreeMap<String, Vec<String>>,
    pub scalars: json_codec::Map<String, json_codec::Value>,
}
pub(crate) const STATE_PAGE_ENTRIES: usize = 128;
/// Content-defined key boundaries keep an insertion from shifting every later
/// page (about 32 entries per page for uniformly distributed key hashes).
/// Values do not participate: updating an entity only rewrites its page.
/// The hard cap bounds adversarial keys within the single current on-disk format.
/// Readers verify exact bound pages; partitioning is not a format discriminator.
#[cfg(test)]
pub(crate) fn state_pages(
    map: &json_codec::Map<String, json_codec::Value>,
) -> impl Iterator<Item = json_codec::Map<String, json_codec::Value>> + '_ {
    let mut entries = map.iter().peekable();
    std::iter::from_fn(move || {
        let mut page = json_codec::Map::new();
        while let Some((key, _)) = entries.peek() {
            let boundary = blake3::hash(key.as_bytes()).as_bytes()[0] & 31 == 0;
            if !page.is_empty() && (page.len() == STATE_PAGE_ENTRIES || boundary) {
                break;
            }
            let (key, value) = entries.next().expect("peeked entry");
            page.insert(key.clone(), value.clone());
        }
        (!page.is_empty()).then_some(page)
    })
}

/// Each page is immutable and independently hashed; the journal holds only roots.
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
    prepare_snapshot(state, maximum)?.store(members, state)
}
/// Opaque in-process encoding provenance, never loaded from disk. Its parent
/// Engine publication receipt is the authority to reuse it for a future state.
#[derive(Clone)]
pub(crate) struct SnapshotReuse {
    descriptor: Snapshot,
    hash: String,
    page_maxima: BTreeMap<String, usize>,
}
struct SnapshotFieldPlan {
    /// None means exact references to previously published canonical pages.
    objects: Option<std::ops::Range<usize>>,
    max_page_bytes: usize,
}
pub(crate) struct PreparedSnapshot {
    pub(crate) descriptor: Snapshot,
    pub(crate) hash: String,
    objects: Vec<PreparedMetadataObject>,
    fields: BTreeMap<String, SnapshotFieldPlan>,
    maximum: usize,
}
#[path = "journal/snapshot.rs"]
mod snapshot;
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
) -> Result<PreparedSnapshot> {
    snapshot::prepare_incremental(before, after, basis, maximum)
}

// Differential oracle for exact current bytes and field-specific serde rules.
#[cfg(test)]
fn prepare_snapshot_reference(state: &State, maximum: usize) -> Result<PreparedSnapshot> {
    let value = json_codec::to_value(state)?;
    let obj = value.as_object().ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::StateInvalid,
            "state is not an object",
        )
    })?;
    let mut pages = BTreeMap::new();
    let mut scalars = json_codec::Map::new();
    let mut objects = Vec::new();
    let mut fields = BTreeMap::new();
    for (field, value) in obj {
        // Event indexes are fixed-width, so lexical order is chronological.
        // Reuse ordinary object pages: dependency repair and historical proofs
        // continue to traverse the exact same content-addressed object graph.
        let events = (field == "events").then(|| {
            value
                .as_array()
                .expect("State events serialize as an array")
                .iter()
                .enumerate()
                .map(|(i, event)| (format!("{i:020}"), event.clone()))
                .collect::<json_codec::Map<_, _>>()
        });
        if let Some(map) = events.as_ref().or_else(|| value.as_object()) {
            let start = objects.len();
            let mut max_page_bytes = 0;
            let mut hashes = Vec::new();
            for page in state_pages(map) {
                let object = PreparedMetadataObject::new("state-page", &page, maximum)?;
                max_page_bytes = max_page_bytes.max(object.bytes.len());
                hashes.push(object.hash.clone());
                objects.push(object);
            }
            fields.insert(
                field.clone(),
                SnapshotFieldPlan {
                    objects: Some(start..objects.len()),
                    max_page_bytes,
                },
            );
            pages.insert(field.clone(), hashes);
        } else {
            scalars.insert(field.clone(), value.clone());
        }
    }
    let descriptor = Snapshot {
        sequence: state.sequence,
        pages,
        scalars,
    };
    let root = PreparedMetadataObject::new("snapshot", &descriptor, maximum)?;
    let hash = root.hash.clone();
    objects.push(root);
    Ok(PreparedSnapshot {
        descriptor,
        hash,
        objects,
        fields,
        maximum,
    })
}

#[cfg(test)]
thread_local! {
    pub(crate) static SNAPSHOT_LOAD_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
#[cfg(test)]
pub(crate) fn load_snapshot(d: &DiskConfig, hash: &str) -> Result<State> {
    let snapshot: Snapshot = load_object(d, "snapshot", hash)?;
    resolve_snapshot(snapshot, |hash| load_object(d, "state-page", hash))
}

// Page-codec tests intentionally use partial maps, not engine-admissible state.
#[cfg(test)]
fn load_raw_snapshot_fixture(d: &DiskConfig, hash: &str) -> Result<State> {
    let snapshot: Snapshot = load_object(d, "snapshot", hash)?;
    Ok(
        resolve_raw_snapshot(snapshot, |hash| load_object(d, "state-page", hash))?
            .into_codec_fixture(),
    )
}

/// Decode the complete exact root using only authenticated page objects. The
/// resolver may select different replicas, never alternate roots or page hashes.
#[cfg(test)]
pub(crate) fn resolve_snapshot(
    snapshot: Snapshot,
    page: impl FnMut(&str) -> Result<json_codec::Map<String, json_codec::Value>>,
) -> Result<State> {
    Ok(resolve_admitted_snapshot(snapshot, page)?.into_state())
}

pub(crate) fn resolve_admitted_snapshot(
    snapshot: Snapshot,
    page: impl FnMut(&str) -> Result<json_codec::Map<String, json_codec::Value>>,
) -> Result<crate::history_package::AdmittedState> {
    resolve_raw_snapshot(snapshot, page)?.try_into()
}

fn resolve_raw_snapshot(
    snapshot: Snapshot,
    mut page: impl FnMut(&str) -> Result<json_codec::Map<String, json_codec::Value>>,
) -> Result<crate::history_package::RawState> {
    #[cfg(test)]
    SNAPSHOT_LOAD_COUNT.with(|count| count.set(count.get() + 1));
    if snapshot.scalars.contains_key("events") && snapshot.pages.contains_key("events") {
        return Err(Error::new(
            storage_contract::ErrorCode::ObjectCorrupt,
            "snapshot events are both scalar and paged",
        ));
    }
    if !snapshot.pages.contains_key("events") {
        return Err(Error::new(
            storage_contract::ErrorCode::FormatUnsupported,
            "snapshot events must use the current paged representation",
        ));
    }
    let mut out = snapshot.scalars;
    for (field, hashes) in snapshot.pages {
        let mut map = json_codec::Map::new();
        for h in hashes {
            for (k, v) in page(&h)? {
                if map.insert(k, v).is_some() {
                    return Err(Error::new(
                        storage_contract::ErrorCode::ObjectCorrupt,
                        "duplicate snapshot entity",
                    ));
                }
            }
        }
        if out.contains_key(&field) {
            return Err(Error::new(
                storage_contract::ErrorCode::ObjectCorrupt,
                "snapshot field is both scalar and paged",
            ));
        }
        let value = if field == "events" {
            let mut events = Vec::with_capacity(map.len());
            for (index, (key, value)) in map.into_iter().enumerate() {
                if key != format!("{index:020}") {
                    return Err(Error::new(
                        storage_contract::ErrorCode::ObjectCorrupt,
                        "snapshot event indexes are not contiguous",
                    ));
                }
                events.push(value);
            }
            json_codec::Value::Array(events)
        } else {
            json_codec::Value::Object(map)
        };
        out.insert(field, value);
    }
    let state: crate::history_package::RawState =
        json_codec::from_value(json_codec::Value::Object(out))?;
    if state.sequence() != snapshot.sequence {
        return Err(Error::new(
            storage_contract::ErrorCode::ObjectCorrupt,
            "snapshot sequence differs",
        ));
    }
    Ok(state)
}

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
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub history_manifest_hashes: BTreeMap<String, String>,
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
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct LocalPublication {
    pub candidate: Candidate,
    pub publication: Publication,
    pub commit_proof: CommitProof,
    pub publication_receipts: Vec<Receipt>,
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
    pub content_state_page: json_codec::Map<String, json_codec::Value>,
}
/// Offline receipts must bind the saved pool membership, not a smaller set
/// declared by their own candidate. Verification never creates a missing anchor.
fn verify_local_membership(root: &Path, candidate: &Candidate, seal: &DiskSeal) -> Result<()> {
    let config = ValidatedConfig::try_from(read_json::<Config>(
        &root.join("metadata/pool-membership/config.json"),
    )?)?;
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
    verify_tombstone_content(directory, version, &tombstone)
}

pub(crate) fn verify_tombstone_content(
    directory: &Path,
    version: &FileVersion,
    t: &ContentTombstone,
) -> Result<()> {
    // Intentional removal proves the content state, not the retained descriptor.
    // Its immutable identity and location must still bind to the local publication.
    verify_local_version_publication(directory, version)?;
    verify_publication_seal(&t.candidate, &t.publication, &t.publication_seal)?;
    let root = directory.ancestors().nth(4).ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::PublicationInvalid,
            "invalid version location",
        )
    })?;
    let seal: DiskSeal = read_json(&root.join("disk.json"))?;
    verify_local_membership(root, &t.candidate, &seal)?;
    if t.version_id != version.version_id
        || !matches!(t.state, ContentState::Purged | ContentState::Migrated)
        || t.candidate.pool_id != seal.pool_id
        || t.publication.candidate_hash != object_hash("candidate", &t.candidate)?
        || object_hash("snapshot", &t.snapshot)? != t.candidate.snapshot_hash
        || t.snapshot.sequence != t.candidate.sequence
        || t.content_state_page
            .get(&t.version_id)
            .and_then(|v| v.as_str())
            != Some(t.state.as_str())
        || !t.snapshot.pages.get("content_states").is_some_and(|pages| {
            object_hash("state-page", &t.content_state_page).is_ok_and(|hash| pages.contains(&hash))
        })
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
pub(crate) fn local_proof_directory(directory: &Path, candidate: &str) -> Result<PathBuf> {
    let original = directory.join("commit-proof.json");
    match read_json::<CommitProof>(&original) {
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
    verify_local_version_publication(directory, version)?;
    if open_optional_regular(&directory.join("purged.json"))?.is_some() {
        return Err(Error::new(
            storage_contract::ErrorCode::ContentPurged,
            "this version has a local content tombstone",
        ));
    }
    Ok(())
}

/// Verify immutable publication identity independently of current availability.
fn verify_local_version_publication(directory: &Path, version: &FileVersion) -> Result<()> {
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
    let local: LocalPublication = read_json(&directory.join("publication-proof.json"))?;
    let proof: CommitProof = read_json(&directory.join("commit-proof.json"))?;
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
    let seal: DiskSeal = read_json(&root.join("disk.json"))?;
    verify_local_membership(root, c, &seal)?;
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
