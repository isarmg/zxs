use crate::{disk::*, *};
use crate::persist::layout;
use data_serialization::{Deserialize, Serialize};
#[cfg(test)]
use std::io::Write;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{BufReader, Read},
    path::{Path, PathBuf},
};
mod retained_encoding;
pub(crate) use retained_encoding::{
    AuthenticatedMetadataRead, MetadataDomain, RawMetadataRead, RetainedMetadataEncoding,
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
    #[serialization(rename = "MEMBERSHIP_FENCE")]
    MembershipFence,
    #[serialization(rename = "MEMBERSHIP_CONTROL_BOOTSTRAP")]
    MembershipControlBootstrap,
    #[serialization(rename = "MEMBERSHIP_JOINT_ACCEPT")]
    MembershipJointAccept,
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
            "MEMBERSHIP_FENCE" => Ok(Self::MembershipFence),
            "MEMBERSHIP_CONTROL_BOOTSTRAP" => Ok(Self::MembershipControlBootstrap),
            "MEMBERSHIP_JOINT_ACCEPT" => Ok(Self::MembershipJointAccept),
            token => Err(data_serialization::de::Error::unknown_variant(
                token,
                &[
                    "PREPARE",
                    "ACCEPT",
                    "FINALIZE",
                    "PUBLICATION",
                    "REPROTECT",
                    "MEMBERSHIP_FENCE",
                    "MEMBERSHIP_CONTROL_BOOTSTRAP",
                    "MEMBERSHIP_JOINT_ACCEPT",
                ],
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Record {
    pub record_type: RecordType,
    #[serialization(with = "storage_contract::u64_string")]
    pub sequence: u64,
    pub candidate_hash: String,
    pub object_hash: String,
    pub previous_frame_hash: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
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
    d.root.join("metadata/application-log")
        .join(layout::JOURNAL.as_path())
}
#[cfg(test)]
pub(crate) fn scan(d: &DiskConfig) -> Result<Scan> {
    scan_at(&DiskRoot::open(&d.root)?, d)
}
#[cfg(test)]
pub(crate) fn scan_at(root: &DiskRoot, d: &DiskConfig) -> Result<Scan> {
    scan_path_at(root, &log_path(d))
}
pub(crate) fn scan_configured_at(
    root: &DiskRoot,
    d: &DiskConfig,
    config: &ValidatedConfig,
) -> Result<Scan> {
    scan_path_at(
        root,
        &crate::engine::membership_transition::pending::activation::log_path(config, d),
    )
}
pub(crate) fn scan_path_at(root: &DiskRoot, path: &Path) -> Result<Scan> {
    match root.open_optional_read_regular(path)? {
        Some(file) => scan_frames(BufReader::new(file)),
        None => Ok(Scan {
            records: Vec::new(),
            valid_bytes: 0,
            torn_tail: false,
        }),
    }
}

mod terminal_chain_first;
pub(crate) use terminal_chain_first::{TerminalChainFirstJournal, verify_terminal_chain_log_bytes};

mod backup_first;
pub(crate) use backup_first::BackupFirstJournal;

mod pending;
pub(crate) use pending::PendingJournal;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum JournalNamespace {
    Active,
    Pending,
    Joint,
}
impl JournalNamespace {
    fn accepts(self, kind: RecordType) -> bool {
        match kind {
            RecordType::Prepare
            | RecordType::Accept
            | RecordType::Finalize
            | RecordType::Publication
            | RecordType::Reprotect
            | RecordType::MembershipFence => self == Self::Active,
            RecordType::MembershipControlBootstrap => self == Self::Pending,
            RecordType::MembershipJointAccept => self == Self::Joint,
        }
    }
}

/// Existing bytes stay untouched until recovery has accepted the publication prefix.
pub(crate) struct OpenJournal {
    namespace: JournalNamespace,
    io: JournalIo,
    pub scan: Scan,
    #[cfg(feature = "fault-injection")]
    opening_fault: Option<OpeningFault>,
}
impl OpenJournal {
    #[cfg(test)]
    pub(crate) fn scan(d: &DiskConfig) -> Result<Self> {
        Self::scan_at(&DiskRoot::open(&d.root)?, d)
    }
    #[cfg(test)]
    pub(crate) fn scan_at(root: &DiskRoot, d: &DiskConfig) -> Result<Self> {
        let io = root.journal_directory(log_path(d).parent().expect("journal parent"))?;
        io.require_genesis()?;
        Self::scan_directory(io, JournalNamespace::Active)
    }
    pub(crate) fn scan_configured_at(
        root: &DiskRoot,
        d: &DiskConfig,
        config: &ValidatedConfig,
    ) -> Result<Self> {
        let path = crate::engine::membership_transition::pending::activation::log_path(config, d);
        let io = root.journal_directory(path.parent().expect("invariant: journal parent"))?;
        io.require_configured(config)?;
        Self::scan_directory(io, JournalNamespace::Active)
    }
    fn scan_directory(mut io: JournalIo, namespace: JournalNamespace) -> Result<Self> {
        let scan = io.scan(namespace)?;
        Ok(Self {
            namespace,
            io,
            scan,
            #[cfg(feature = "fault-injection")]
            opening_fault: None,
        })
    }
    /// Transfer the scanned history to recovery; only the current slot is
    /// duplicated for the writer's independently mutable receipt tail.
    pub(crate) fn into_writer(mut self) -> Result<(JournalWriter, Vec<(Record, String)>)> {
        // A surviving leaf may come from a failed creation before its directory fence.
        self.io.finish_open(
            #[cfg(feature = "fault-injection")]
            self.opening_fault,
        )?;
        let records = self.scan.records;
        let start = records.last().map_or(0, |(last, _)| {
            records.partition_point(|(record, _)| record.sequence < last.sequence)
        });
        let tail = records[start..].to_vec();
        Ok((
            JournalWriter {
                namespace: self.namespace,
                io: self.io,
                records: tail,
                state: if self.scan.torn_tail {
                    WriterState::TornTail(self.scan.valid_bytes)
                } else {
                    WriterState::Ready
                },
                #[cfg(feature = "fault-injection")]
                fault: None,
            },
            records,
        ))
    }
}

/// The same bounded frame/chain parser serves recovery and streaming scrub.
/// This reader never repairs a torn tail or opens a writable descriptor.
pub(crate) struct JournalFrameReader<R> {
    namespace: JournalNamespace,
    reader: R,
    previous: String,
    last_sequence: u64,
    pub(crate) valid_bytes: u64,
    pub(crate) torn_tail: bool,
}

impl<R: Read> JournalFrameReader<R> {
    pub(crate) fn new(reader: R) -> Self {
        Self::with_namespace(reader, JournalNamespace::Active)
    }
    fn with_namespace(reader: R, namespace: JournalNamespace) -> Self {
        Self {
            namespace,
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
        if self.namespace != JournalNamespace::Active && self.valid_bytes != 0 {
            return Err(Error::new(
                ErrorCode::LogCorrupt,
                "membership singleton journal contains a second frame",
            ));
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
        let record: Record = decode_strict(&payload).map_err(|source| {
            Error::from(source).context(ErrorCode::LogCorrupt, "log record JSON is invalid")
        })?;
        if !self.namespace.accepts(record.record_type)
            || canonical_json(&record)? != payload
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
    scan_frames_namespace(reader, JournalNamespace::Active)
}
fn scan_frames_namespace(reader: impl Read, namespace: JournalNamespace) -> Result<Scan> {
    let mut reader = JournalFrameReader::with_namespace(reader, namespace);
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
pub(crate) fn scan_frames_observed(
    reader: impl Read,
    namespace: JournalNamespace,
    observe: &mut impl FnMut(usize) -> Result<()>,
) -> Result<Scan> {
    let mut reader = JournalFrameReader::with_namespace(reader, namespace);
    let mut records = Vec::new();
    while let Some(record) = reader.next_record_observed(observe)? {
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
    Pending,
    Joint,
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
    namespace: JournalNamespace,
    io: JournalIo,
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

pub(crate) fn journal_name() -> platform_io::fs_safety::EntryName {
    layout::JOURNAL.clone()
}

impl JournalWriter {
    #[cfg(test)]
    pub(crate) fn create(d: &DiskConfig) -> Result<Self> {
        Self::create_at(&DiskRoot::open(&d.root)?, d)
    }
    pub(crate) fn create_at(root: &DiskRoot, d: &DiskConfig) -> Result<Self> {
        let mut io = root.journal_directory(log_path(d).parent().expect("journal parent"))?;
        io.require_genesis()?;
        io.create()?;
        Ok(Self {
            namespace: JournalNamespace::Active,
            io,
            records: Vec::new(),
            state: WriterState::Ready,
            #[cfg(feature = "fault-injection")]
            fault: None,
        })
    }
    pub(crate) fn records(&self) -> &[(Record, String)] {
        &self.records
    }
    /// Read-only name/held-file identity check; cached receipts alone cannot
    /// certify the currently named original journal. No descriptor is replaced.
    pub(crate) fn verify_retained_name(&self) -> Result<()> {
        self.io.verify_retained_name()
    }
    pub(crate) fn requires_reopen(&self) -> bool {
        self.state == WriterState::RecoveryRequired
    }
    pub(crate) fn needs_recovery(&self) -> bool {
        self.state != WriterState::Ready
    }
    /// A fact from the original read-only scan, not permission to truncate.
    /// An indeterminate live write still requires reopening, never this allowance.
    pub(crate) fn has_scanned_torn_tail(&self) -> bool {
        matches!(self.state, WriterState::TornTail(_))
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
        self.io.check_writer()?;
        match self.state {
            WriterState::Ready => Ok(()),
            WriterState::RecoveryRequired => Err(Error::new(
                ErrorCode::RecoveryRequired,
                "journal I/O failed; reopen for recovery",
            )),
            WriterState::TornTail(valid_bytes) => {
                self.state = WriterState::RecoveryRequired;
                self.io.repair(valid_bytes)?;
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
        if !self.namespace.accepts(kind)
            || !matches!(
                (mode, self.namespace),
                (
                    AppendMode::Live | AppendMode::Recovery,
                    JournalNamespace::Active
                ) | (AppendMode::Pending, JournalNamespace::Pending)
                    | (AppendMode::Joint, JournalNamespace::Joint)
            )
        {
            return Err(Error::new(
                ErrorCode::LogCorrupt,
                "record belongs to another journal namespace",
            ));
        }
        self.io.check_writer()?;
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
            if self.namespace != JournalNamespace::Active {
                self.io.verify_singleton()?;
            }
            return Ok(Receipt {
                disk_id: d.disk_id.clone(),
                slot_id: d.slot_id.clone(),
                frame_hash: hash.clone(),
                record: record.clone(),
            });
        }
        if self.namespace != JournalNamespace::Active && !self.records.is_empty() {
            return Err(Error::new(
                ErrorCode::LogFork,
                "membership singleton journal binding differs",
            ));
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
        let repair = match self.state {
            WriterState::TornTail(valid_bytes) => Some(valid_bytes),
            WriterState::Ready => None,
            WriterState::RecoveryRequired => {
                unreachable!("invariant: failed writer rejected before framing")
            }
        };
        // A partial write, failed sync, or unwind leaves the writer unusable until recovery.
        self.state = WriterState::RecoveryRequired;
        self.io.append(
            &bytes,
            repair,
            self.namespace,
            #[cfg(feature = "fault-injection")]
            self.fault.take(),
        )?;
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
#[cfg(test)]
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
    pub(crate) fn kind(&self) -> &'static str {
        self.kind
    }
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    /// Install already-hashed bytes through an admitted, held directory root.
    pub(crate) fn store_at_root(&self, root: &DiskRoot, disk: &DiskConfig) -> Result<()> {
        if self.bytes.len() > MAX_METADATA {
            return Err(Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                "serialized metadata exceeds reader limit",
            ));
        }
        write_object_bytes_at(root, disk, self.kind, &self.hash, &self.bytes)
    }
    #[cfg(test)]
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
        Self::authenticate_prepared(
            retained_encoding::ExpectedMetadataDigest(hash),
            PreparedMetadataObject { kind, hash: actual, bytes },
            value,
        )
    }

    fn authenticate_prepared(
        expected: retained_encoding::ExpectedMetadataDigest<'_>,
        prepared: PreparedMetadataObject,
        value: json_codec::Value,
    ) -> Result<Self> {
        if prepared.bytes.len() > MAX_METADATA {
            return Err(Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                "serialized metadata exceeds reader limit",
            ));
        }
        if prepared.hash != expected.0 {
            return Err(Error::new(
                storage_contract::ErrorCode::ObjectCorrupt,
                "immutable object digest differs",
            ));
        }
        Ok(Self { prepared, value })
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
    /// Consume the already authenticated decoded value without copying its tree.
    pub(crate) fn into_value(self) -> json_codec::Value {
        self.value
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
    let value = storage_contract::strict_json(bytes)?;
    if storage_contract::object_hash_value(kind, &value)? != hash {
        return Err(Error::new(
            storage_contract::ErrorCode::ObjectCorrupt,
            "immutable object digest differs",
        ));
    }
    decode_proof_value(value)
}

/// Local compound proofs have no outer digest. Preserve every nested hash input
/// before existing receipt/identity verification; a typed projection is not proof.
pub(crate) fn decode_proof<T: data_serialization::de::DeserializeOwned + Serialize>(
    bytes: &[u8],
) -> Result<T> {
    decode_proof_value(storage_contract::strict_json(bytes)?)
}

fn decode_proof_value<T: data_serialization::de::DeserializeOwned + Serialize>(
    value: json_codec::Value,
) -> Result<T> {
    let proof = T::deserialize(&value)?;
    if json_codec::to_value(&proof)? != value {
        return Err(Error::new(
            ErrorCode::ObjectCorrupt,
            "proof representation differs from its complete input",
        ));
    }
    Ok(proof)
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
    prepare_snapshot(state, maximum)?.store(members, state)
}
/// Same-process published provenance. Each map root clone only increments an Arc.
#[derive(Clone)]
pub(crate) struct SnapshotReuse {
    descriptor: Snapshot,
    hash: String,
    fields: BTreeMap<String, snapshot::FieldRoot>,
    auth_bytes: usize,
}
pub(crate) struct PreparedSnapshot {
    pub(crate) metadata_integrity:
        Option<std::sync::Arc<crate::engine::provenance::MetadataIntegrity>>,
    pub(crate) descriptor: Snapshot,
    pub(crate) hash: String,
    objects: Vec<PreparedMetadataObject>,
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
) -> Result<PreparedSnapshot> {
    snapshot::prepare_incremental(before, after, basis, maximum)
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
    load: impl FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject>,
) -> Result<crate::history_package::AdmittedState> {
    resolve_raw_snapshot(snapshot, load)?.try_into()
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
    out.insert("control_auth".into(), auth.into_value());
    for (field, reference) in snapshot.maps {
        out.insert(
            field,
            json_codec::Value::Object(crate::metadata_map::read_map(&reference, &mut load)?),
        );
    }
    let mut events = Vec::new();
    for (index, (key, value)) in crate::metadata_map::read_map(&snapshot.events, &mut load)?
        .into_iter()
        .enumerate()
    {
        if key != format!("{index:020}") {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "snapshot event indexes are not contiguous",
            ));
        }
        events.push(value);
    }
    out.insert("events".into(), json_codec::Value::Array(events));
    Ok(json_codec::from_value(json_codec::Value::Object(out))?)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct VersionEvidence {
    pub version: FileVersion,
    pub placement: Placement,
}
mod candidate;
pub(crate) use candidate::{Candidate, CandidateData};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Finalize {
    pub candidate_hash: String,
    pub accepts: Vec<Receipt>,
    #[serialization(default, skip_serializing_if = "Option::is_none")]
    pub recovery_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct PayloadReceipt {
    pub version_id: String,
    pub disk_id: String,
    pub slot_id: String,
    pub proof_hash: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct CommitProof {
    pub candidate_hash: String,
    pub finalize: Finalize,
    pub version_hash: String,
    pub disk_id: String,
    pub slot_id: String,
    pub route_slot_ids: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
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
    verify_publication_evidence(c, p, Some(seal))
}

/// Verify the candidate-bound ACCEPT and FINALIZE material before repairing an
/// incomplete publication. This grants no D3 barrier or namespace admission.
pub(crate) fn verify_finalized_publication(c: &Candidate, p: &Publication) -> Result<()> {
    verify_publication_evidence(c, p, None)
}

fn verify_publication_evidence(
    c: &Candidate,
    p: &Publication,
    seal: Option<&PublicationSeal>,
) -> Result<()> {
    let candidate = object_hash("candidate", c)?;
    let publication = object_hash("publication", p)?;
    let finalize = object_hash("finalize", &p.finalize)?;
    let mut ids = c.metadata_disk_ids.clone();
    ids.sort();
    ids.dedup();
    let receipts = match seal {
        Some(seal) => seal.receipts.as_slice(),
        None => p.finalize_receipts.as_slice(),
    };
    let mut actual = receipts
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
        || seal.is_some_and(|seal| {
            seal.receipts.len() != ids.len()
                || seal.candidate_hash != candidate
                || seal.publication_hash != publication
        })
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
    for r in receipts {
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
            || (seal.is_some() && r.record.previous_frame_hash != f.frame_hash)
            || !receipt_binds(f, RecordType::Finalize, c.sequence, &candidate, &finalize)?
        {
            return Err(Error::new(
                storage_contract::ErrorCode::PublicationInvalid,
                "member synchronized chain is invalid",
            ));
        }
        if seal.is_some()
            && !receipt_binds(
                r,
                RecordType::Publication,
                c.sequence,
                &candidate,
                &publication,
            )?
        {
            return Err(Error::new(
                storage_contract::ErrorCode::PublicationInvalid,
                "publication frame receipt is invalid",
            ));
        }
    }
    Ok(())
}
#[derive(Clone, Debug, Deserialize)]
#[serialization(deny_unknown_fields)]
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
/// Offline receipts must bind the saved pool membership, not a smaller set
/// declared by their own candidate. Verification never creates a missing anchor.
fn verify_local_membership(
    root: &Path,
    candidate: &Candidate,
    seal: &DiskSeal,
    read: &mut impl FnMut(&Path) -> Result<Vec<u8>>,
    metadata: Option<&crate::engine::provenance::MetadataReadContext<'_>>,
) -> Result<()> {
    let config =
        crate::engine::membership_transition::pending::activation::verify_local_membership(
            root, candidate, seal, read, metadata,
        )?;
    if seal.pool_id != config.pool_id
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
    let tombstone: ContentTombstone =
        decode_proof(&read_bytes(&directory.join("purged.json"), MAX_METADATA)?)?;
    verify_tombstone_content_in_context(directory, version, &tombstone, None)
}

mod version_publication;
pub(crate) use version_publication::{
    verify_local_publication, verify_local_publication_at, verify_local_publication_in_context,
    verify_local_publication_observed, verify_retained_version_publication_at,
    verify_tombstone_content_at, verify_tombstone_content_in_context,
};

/// A later protection transaction reuses immutable FULL identities. Preserve
/// the first locally published proof and retain each later receipt by candidate.
pub(crate) fn local_proof_directory_at(
    root: &DiskRoot,
    directory: &Path,
    candidate: &str,
) -> Result<PathBuf> {
    let original = directory.join("commit-proof.json");
    match root
        .read_bytes(&original, MAX_METADATA)
        .and_then(|bytes| decode_proof::<CommitProof>(&bytes))
    {
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

#[cfg(test)]
#[path = "../../../test/engine/unit/journal/proof_decoding_tests.rs"]
mod proof_decoding_tests;

#[cfg(test)]
#[path = "../../../test/engine/unit/journal/version_publication_fixture.rs"]
pub(crate) mod version_publication_fixture;

#[cfg(test)]
#[path = "../../../test/engine/unit/journal/container_tags_fixture.rs"]
pub(crate) mod container_tags_fixture;
