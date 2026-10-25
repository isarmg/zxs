use crate::{disk::*, *};
use data_serialization::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{BufReader, Read, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};
pub(crate) const MAX_FRAME: usize = 16 * 1024 * 1024;
const LOG_MAGIC: &[u8; 8] = b"ZXSLOG\0\0";
const LOG_HEADER_BYTES: usize = 12;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Record {
    pub record_type: String,
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
    kind: &str,
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
    pub stamp: Option<FileStamp>,
}
pub(crate) fn log_path(d: &DiskConfig) -> PathBuf {
    d.root.join("metadata/application-log/journal.bin")
}
#[cfg(test)]
thread_local! {
    static SCAN_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
pub(crate) fn scan(d: &DiskConfig) -> Result<Scan> {
    #[cfg(test)]
    SCAN_COUNT.with(|count| count.set(count.get() + 1));
    let path = log_path(d);
    let Some(file) = open_optional_regular(&path)? else {
        return Ok(Scan {
            records: Vec::new(),
            valid_bytes: 0,
            torn_tail: false,
            stamp: None,
        });
    };
    let before = FileStamp::of(&file)?;
    let mut out = scan_frames(BufReader::new(&file))?;
    // Bind reuse to the exact descriptor whose bytes were verified, not a new
    // fingerprint captured later. Torn or concurrently changed logs get no hint.
    if !out.torn_tail
        && FileStamp::of(&file)? == before
        && FileStamp::of(&open_regular(&path)?)? == before
    {
        out.stamp = Some(before);
    }
    Ok(out)
}

fn scan_frames(mut reader: impl Read) -> Result<Scan> {
    let mut out = Scan {
        records: Vec::new(),
        valid_bytes: 0,
        torn_tail: false,
        stamp: None,
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
            return Err(Error::new("LOG_CORRUPT", "invalid log frame header"));
        }
        let len = u32::from_le_bytes(
            header[8..LOG_HEADER_BYTES]
                .try_into()
                .expect("fixed header"),
        ) as usize;
        if len > MAX_FRAME {
            return Err(Error::new("LOG_CORRUPT", "log frame length exceeds limit"));
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
            return Err(Error::new("LOG_CORRUPT", "log frame checksum differs"));
        }
        let record: Record = decode_strict(&payload)?;
        if !matches!(
            record.record_type.as_str(),
            "PREPARE" | "ACCEPT" | "FINALIZE" | "PUBLICATION" | "REPROTECT"
        ) {
            return Err(Error::new(
                "LOG_FORMAT_UNSUPPORTED",
                "unknown journal record type",
            ));
        }
        if canonical_json(&record)? != payload
            || record.previous_frame_hash != previous
            || record.sequence < last_sequence
        {
            return Err(Error::new(
                "LOG_CORRUPT",
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
// A process-local optimization, never a recovery authority. Keep only the current
// decision slot per journal; older receipt retries deliberately use a full scan.
// Device/inode plus ctime detect replacement, truncation and same-size rewrites
// (including writers that restore mtime). Restart always verifies from byte zero.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileStamp {
    dev: u64,
    ino: u64,
    len: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}
impl FileStamp {
    fn of(file: &std::fs::File) -> Result<Self> {
        let m = file.metadata()?;
        Ok(Self {
            dev: m.dev(),
            ino: m.ino(),
            len: m.len(),
            mtime: (m.mtime(), m.mtime_nsec()),
            ctime: (m.ctime(), m.ctime_nsec()),
        })
    }
}
struct VerifiedTail {
    stamp: FileStamp,
    records: Vec<(Record, String)>,
}
const MAX_CACHED_JOURNALS: usize = 64;
fn tails() -> &'static Mutex<BTreeMap<PathBuf, VerifiedTail>> {
    static TAILS: OnceLock<Mutex<BTreeMap<PathBuf, VerifiedTail>>> = OnceLock::new();
    TAILS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn writers() -> &'static Mutex<BTreeMap<PathBuf, Arc<Mutex<()>>>> {
    static WRITERS: OnceLock<Mutex<BTreeMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    WRITERS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

// A lease exists for every holder AND waiter. Cache eviction never removes a
// writer lock, and the registry owns no paths once their last lease is gone.
// Configured roots are absolute, traversal-free and symlink-safe, like log I/O.
struct JournalWriter {
    path: PathBuf,
    lock: Option<Arc<Mutex<()>>>,
}
impl JournalWriter {
    fn acquire(path: &Path) -> Result<Self> {
        let mut writers = writers()
            .lock()
            .map_err(|_| Error::new("RECOVERY_REQUIRED", "journal writer registry poisoned"))?;
        let lock = writers.entry(path.to_path_buf()).or_default().clone();
        Ok(Self {
            path: path.to_path_buf(),
            lock: Some(lock),
        })
    }
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, ()>> {
        self.lock
            .as_ref()
            .expect("live journal writer lease")
            .lock()
            .map_err(|_| Error::new("RECOVERY_REQUIRED", "journal writer lock poisoned"))
    }
}
impl Drop for JournalWriter {
    fn drop(&mut self) {
        let Ok(mut writers) = writers().lock() else {
            return;
        };
        // Release this lease while holding the registry lock too: simultaneous
        // drops must not each see the other's soon-to-be-released ownership.
        drop(self.lock.take());
        if writers
            .get(&self.path)
            .is_some_and(|lock| Arc::strong_count(lock) == 1)
        {
            writers.remove(&self.path);
        }
    }
}

pub(crate) fn append(
    d: &DiskConfig,
    kind: &str,
    sequence: u64,
    candidate: &str,
    object: &str,
) -> Result<Receipt> {
    append_with_sync(d, kind, sequence, candidate, object, None, sync_journal)
}

// Borrow only the current record and the prefix hashes already held by recovery.
// This is same-invocation evidence, not a persistent or all-history receipt cache.
pub(crate) struct ScannedReceipt<'a> {
    pub stamp: &'a FileStamp,
    pub hashes: &'a BTreeSet<String>,
    pub record: &'a Record,
}
pub(crate) fn replay_publication(
    d: &DiskConfig,
    sequence: u64,
    candidate: &str,
    object: &str,
    scanned: Option<ScannedReceipt<'_>>,
) -> Result<Receipt> {
    append_with_sync(
        d,
        "PUBLICATION",
        sequence,
        candidate,
        object,
        scanned,
        sync_journal,
    )
}
fn sync_journal(file: &std::fs::File, parent: &Path) -> Result<()> {
    file.sync_all()?;
    sync_dir(parent)
}
fn sync_existing(
    file: &std::fs::File,
    path: &Path,
    before: &FileStamp,
    synchronize: impl FnOnce(&std::fs::File, &Path) -> Result<()>,
) -> Result<()> {
    if FileStamp::of(file)? != *before || FileStamp::of(&open_regular(path)?)? != *before {
        return Err(Error::new(
            "RECOVERY_REQUIRED",
            "journal changed during verification",
        ));
    }
    synchronize(file, path.parent().expect("log parent"))?;
    if FileStamp::of(file)? != *before || FileStamp::of(&open_regular(path)?)? != *before {
        return Err(Error::new(
            "RECOVERY_REQUIRED",
            "journal changed during synchronization",
        ));
    }
    Ok(())
}

fn append_with_sync(
    d: &DiskConfig,
    kind: &str,
    sequence: u64,
    candidate: &str,
    object: &str,
    scanned_receipt: Option<ScannedReceipt<'_>>,
    synchronize: impl FnOnce(&std::fs::File, &Path) -> Result<()>,
) -> Result<Receipt> {
    let path = log_path(d);
    let writer = JournalWriter::acquire(&path)?;
    let _writer_guard = writer.lock()?;
    private_dir(path.parent().expect("log parent"))?;
    // Only this journal stays locked across I/O. The global cache is held just
    // for removal/insertion, never for a scan or either durability barrier.
    // Remove before touching disk so failures force partial writes to be checked.
    let cached = tails()
        .lock()
        .map_err(|_| Error::new("RECOVERY_REQUIRED", "journal cache lock poisoned"))?
        .remove(&path);
    let mut file = open_append_regular(&path)?;
    let before = FileStamp::of(&file)?;
    if let Some(scanned) = scanned_receipt.filter(|s| *s.stamp == before) {
        let r = scanned.record;
        if r.record_type == kind
            && r.sequence == sequence
            && r.candidate_hash == candidate
            && r.object_hash == object
            && let Some(hash) =
                record_frame_hash(r)?.filter(|hash| scanned.hashes.contains(hash.to_hex().as_str()))
        {
            sync_existing(&file, &path, &before, synchronize)?;
            // Never seed the latest-slot cache with a historical receipt.
            return Ok(Receipt {
                disk_id: d.disk_id.clone(),
                slot_id: d.slot_id.clone(),
                frame_hash: hash.to_hex().to_string(),
                record: r.clone(),
            });
        }
    }
    let scanned = match cached {
        Some(tail)
            if tail.stamp == before
                && tail
                    .records
                    .last()
                    .is_some_and(|(r, _)| sequence >= r.sequence) =>
        {
            Scan {
                records: tail.records,
                valid_bytes: before.len,
                torn_tail: false,
                stamp: None,
            }
        }
        _ => scan(d)?,
    };
    if let Some((record, hash)) = scanned.records.iter().find(|(r, _)| {
        r.sequence == sequence
            && r.record_type == kind
            && r.candidate_hash == candidate
            && r.object_hash == object
    }) {
        // Visible bytes are not a durable receipt: the preceding attempt may
        // have failed after writing the frame but before either sync completed.
        sync_existing(&file, &path, &before, synchronize)?;
        let receipt = Receipt {
            disk_id: d.disk_id.clone(),
            slot_id: d.slot_id.clone(),
            frame_hash: hash.clone(),
            record: record.clone(),
        };
        if !scanned.torn_tail {
            cache_tail(path, before, scanned.records)?;
        }
        return Ok(receipt);
    }
    // Reject concurrent replacement/modification while scanning. Engine locks
    // serialize supported writers; this also catches accidental outside writes.
    if FileStamp::of(&file)? != before || FileStamp::of(&open_regular(&path)?)? != before {
        return Err(Error::new(
            "RECOVERY_REQUIRED",
            "journal changed during verification",
        ));
    }
    if scanned
        .records
        .iter()
        .any(|(r, _)| r.sequence == sequence && r.candidate_hash != candidate)
    {
        return Err(Error::new(
            "LOG_FORK",
            "two candidates occupy one decision slot",
        ));
    }
    if scanned
        .records
        .last()
        .is_some_and(|(r, _)| r.sequence > sequence)
    {
        return Err(Error::new(
            "LOG_FORK",
            "cannot append an older decision slot",
        ));
    }
    let record = Record {
        record_type: kind.into(),
        sequence,
        candidate_hash: candidate.into(),
        object_hash: object.into(),
        previous_frame_hash: scanned
            .records
            .last()
            .map(|(_, h)| h.clone())
            .unwrap_or_else(|| "0".repeat(64)),
    };
    let bytes = canonical_json(&record)?;
    if bytes.len() > MAX_FRAME {
        return Err(Error::new(
            "RESOURCE_LIMIT",
            "log record exceeds frame limit",
        ));
    }
    let mut header = Vec::with_capacity(LOG_HEADER_BYTES);
    header.extend(LOG_MAGIC);
    header.extend((bytes.len() as u32).to_le_bytes());
    let mut h = blake3::Hasher::new();
    h.update(&header);
    h.update(&bytes);
    let digest = h.finalize();
    if scanned.torn_tail {
        file.set_len(scanned.valid_bytes)?;
        file.sync_all()?;
    }
    file.write_all(&header)?;
    file.write_all(&bytes)?;
    file.write_all(digest.as_bytes())?;
    synchronize(&file, path.parent().expect("log parent"))?;
    let stamp = FileStamp::of(&file)?;
    if stamp.len != scanned.valid_bytes + header.len() as u64 + bytes.len() as u64 + 32
        || FileStamp::of(&open_regular(&path)?)? != stamp
    {
        return Err(Error::new(
            "RECOVERY_REQUIRED",
            "journal changed during append",
        ));
    }
    let mut records = scanned.records;
    records.push((record.clone(), digest.to_hex().to_string()));
    cache_tail(path, stamp, records)?;
    Ok(Receipt {
        disk_id: d.disk_id.clone(),
        slot_id: d.slot_id.clone(),
        frame_hash: digest.to_hex().to_string(),
        record,
    })
}
fn cache_tail(path: PathBuf, stamp: FileStamp, mut records: Vec<(Record, String)>) -> Result<()> {
    if let Some((last, _)) = records.last() {
        let sequence = last.sequence;
        records.retain(|(r, _)| r.sequence == sequence);
    }
    // A malformed workload may place arbitrarily many records in one slot.
    // Do not retain an unbounded receipt set in that case.
    let retained_bytes = records
        .iter()
        .map(|(r, hash)| {
            r.record_type.len()
                + r.candidate_hash.len()
                + r.object_hash.len()
                + r.previous_frame_hash.len()
                + hash.len()
        })
        .sum::<usize>();
    if records.len() > 64 || retained_bytes > 64 * 1024 {
        return Ok(());
    }
    // Do not retain the allocation of a full historical scan.
    records.shrink_to_fit();
    let mut tails = tails()
        .lock()
        .map_err(|_| Error::new("RECOVERY_REQUIRED", "journal cache lock poisoned"))?;
    // Bound cache size across unrelated engines; eviction only costs a rescan.
    if tails.len() >= MAX_CACHED_JOURNALS {
        tails.pop_first();
    }
    tails.insert(path, VerifiedTail { stamp, records });
    Ok(())
}

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
/// Encode an immutable object once for all replicas, retaining the same hash
/// function, reader bounds, atomic publication and existing-byte checks.
#[cfg(test)]
pub(crate) fn store_object_replicas_bounded<T: Serialize + ?Sized>(
    members: &[DiskConfig],
    kind: &'static str,
    value: &T,
    maximum: usize,
) -> Result<String> {
    PreparedMetadataObject::new(kind, value, maximum)?.store(members)
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
                "RESOURCE_LIMIT",
                format!("serialized metadata exceeds {maximum} byte reader limit"),
            ));
        }
        Ok(Self { kind, hash, bytes })
    }
    pub(crate) fn store(&self, members: &[DiskConfig]) -> Result<String> {
        if !members.is_empty() && self.bytes.len() > MAX_METADATA {
            return Err(Error::new(
                "RESOURCE_LIMIT",
                "serialized metadata exceeds reader limit",
            ));
        }
        for disk in members {
            write_object_bytes(disk, self.kind, &self.hash, &self.bytes)?;
        }
        Ok(self.hash.clone())
    }
}

pub(crate) fn load_object<T: data_serialization::de::DeserializeOwned + Serialize>(
    d: &DiskConfig,
    kind: &str,
    hash: &str,
) -> Result<T> {
    let value = decode_strict(&read_object_bytes(d, kind, hash)?)?;
    if object_hash(kind, &value)? != hash {
        return Err(Error::new(
            "OBJECT_CORRUPT",
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
    prepare_snapshot(state, maximum)?.store(members)
}
pub(crate) fn snapshot_descriptor(state: &State, maximum: usize) -> Result<Snapshot> {
    Ok(prepare_snapshot(state, maximum)?.descriptor)
}

pub(crate) struct PreparedSnapshot {
    pub(crate) descriptor: Snapshot,
    pub(crate) hash: String,
    objects: Vec<PreparedMetadataObject>,
}
impl PreparedSnapshot {
    pub(crate) fn store(&self, members: &[DiskConfig]) -> Result<String> {
        for object in &self.objects {
            object.store(members)?;
        }
        Ok(self.hash.clone())
    }
}
#[path = "journal/snapshot.rs"]
mod snapshot;
pub(crate) fn prepare_snapshot(state: &State, maximum: usize) -> Result<PreparedSnapshot> {
    snapshot::prepare(state, maximum)
}

// Differential oracle for exact current bytes and field-specific serde rules.
#[cfg(test)]
fn prepare_snapshot_reference(state: &State, maximum: usize) -> Result<PreparedSnapshot> {
    let value = json_codec::to_value(state)?;
    let obj = value
        .as_object()
        .ok_or_else(|| Error::new("STATE_INVALID", "state is not an object"))?;
    let mut pages = BTreeMap::new();
    let mut scalars = json_codec::Map::new();
    let mut objects = Vec::new();
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
            let mut hashes = Vec::new();
            for page in state_pages(map) {
                let object = PreparedMetadataObject::new("state-page", &page, maximum)?;
                hashes.push(object.hash.clone());
                objects.push(object);
            }
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

/// Decode the complete exact root using only authenticated page objects. The
/// resolver may select different replicas, never alternate roots or page hashes.
pub(crate) fn resolve_snapshot(
    snapshot: Snapshot,
    mut page: impl FnMut(&str) -> Result<json_codec::Map<String, json_codec::Value>>,
) -> Result<State> {
    #[cfg(test)]
    SNAPSHOT_LOAD_COUNT.with(|count| count.set(count.get() + 1));
    if snapshot.scalars.contains_key("events") && snapshot.pages.contains_key("events") {
        return Err(Error::new(
            "OBJECT_CORRUPT",
            "snapshot events are both scalar and paged",
        ));
    }
    if !snapshot.pages.contains_key("events") {
        return Err(Error::new(
            "FORMAT_UNSUPPORTED",
            "snapshot events must use the current paged representation",
        ));
    }
    let mut out = snapshot.scalars;
    for (field, hashes) in snapshot.pages {
        let mut map = json_codec::Map::new();
        for h in hashes {
            for (k, v) in page(&h)? {
                if map.insert(k, v).is_some() {
                    return Err(Error::new("OBJECT_CORRUPT", "duplicate snapshot entity"));
                }
            }
        }
        if out.contains_key(&field) {
            return Err(Error::new(
                "OBJECT_CORRUPT",
                "snapshot field is both scalar and paged",
            ));
        }
        let value = if field == "events" {
            let mut events = Vec::with_capacity(map.len());
            for (index, (key, value)) in map.into_iter().enumerate() {
                if key != format!("{index:020}") {
                    return Err(Error::new(
                        "OBJECT_CORRUPT",
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
    let state: State = json_codec::from_value(json_codec::Value::Object(out))?;
    if state.sequence != snapshot.sequence {
        return Err(Error::new("OBJECT_CORRUPT", "snapshot sequence differs"));
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
    pub operation_id: String,
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
            "PUBLICATION_INVALID",
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
                    "PUBLICATION_INVALID",
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
                || !receipt_binds(accept, "ACCEPT", c.sequence, &candidate, &candidate)?
            {
                return Err(Error::new("PUBLICATION_INVALID", "ACCEPT receipt differs"));
            }
            accept.frame_hash.as_str()
        };
        if !slots.insert(r.slot_id.clone())
            || r.slot_id != f.slot_id
            || f.record.previous_frame_hash != predecessor
            || r.record.previous_frame_hash != f.frame_hash
            || !receipt_binds(f, "FINALIZE", c.sequence, &candidate, &finalize)?
        {
            return Err(Error::new(
                "PUBLICATION_INVALID",
                "member synchronized chain is invalid",
            ));
        }
        if !receipt_binds(r, "PUBLICATION", c.sequence, &candidate, &publication)? {
            return Err(Error::new(
                "PUBLICATION_INVALID",
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
    pub state: String,
    pub candidate: Candidate,
    pub publication: Publication,
    pub publication_seal: PublicationSeal,
    pub snapshot: Snapshot,
    pub content_state_page: json_codec::Map<String, json_codec::Value>,
}
/// Offline receipts must bind the saved pool membership, not a smaller set
/// declared by their own candidate. Verification never creates a missing anchor.
fn verify_local_membership(root: &Path, candidate: &Candidate, seal: &DiskSeal) -> Result<()> {
    let config: Config = read_json(&root.join("metadata/pool-membership/config.json"))?;
    Engine::validate_configuration(&config)?;
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
            "PUBLICATION_INVALID",
            "local proof differs from saved pool membership",
        ));
    }
    Ok(())
}

pub(crate) fn verify_tombstone(directory: &Path, version: &FileVersion) -> Result<()> {
    // Intentional removal proves the content state, not the retained descriptor.
    // Its immutable identity and location must still bind to the local publication.
    verify_local_version_publication(directory, version)?;
    let t: ContentTombstone = read_json(&directory.join("purged.json"))?;
    verify_publication_seal(&t.candidate, &t.publication, &t.publication_seal)?;
    let root = directory
        .ancestors()
        .nth(4)
        .ok_or_else(|| Error::new("PUBLICATION_INVALID", "invalid version location"))?;
    let seal: DiskSeal = read_json(&root.join("disk.json"))?;
    verify_local_membership(root, &t.candidate, &seal)?;
    if t.version_id != version.version_id
        || !["PURGED", "MIGRATED"].contains(&t.state.as_str())
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
            "TOMBSTONE_INVALID",
            "content tombstone is not covered by a published state",
        ));
    }
    Ok(())
}

/// A later protection transaction reuses immutable FULL identities. Preserve
/// the first locally published proof and retain each later receipt by candidate.
pub(crate) fn local_proof_directory(directory: &Path, candidate: &str) -> Result<PathBuf> {
    let original = directory.join("commit-proof.json");
    if original.try_exists()? {
        let proof: CommitProof = read_json(&original)?;
        if proof.candidate_hash != candidate {
            return Ok(directory.join("protection-proofs").join(candidate));
        }
    }
    Ok(directory.to_owned())
}

pub(crate) fn verify_local_publication(directory: &Path, version: &FileVersion) -> Result<()> {
    verify_local_version_publication(directory, version)?;
    if directory.join("purged.json").exists() {
        return Err(Error::new(
            "CONTENT_PURGED",
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
            "PUBLICATION_INVALID",
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
    let root = directory
        .ancestors()
        .nth(4)
        .ok_or_else(|| Error::new("PUBLICATION_INVALID", "version is outside disk layout"))?;
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
            "PUBLICATION_INVALID",
            "local proof does not bind the version",
        ));
    }
    let evidence = c
        .versions
        .iter()
        .find(|e| e.version.version_id == version.version_id)
        .ok_or_else(|| Error::new("PUBLICATION_INVALID", "version is absent from candidate"))?;
    if object_hash("version", &evidence.version)? != object_hash("version", version)?
        || evidence.placement.route_slot_ids != proof.route_slot_ids
    {
        return Err(Error::new(
            "PUBLICATION_INVALID",
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
        .ok_or_else(|| Error::new("PUBLICATION_INVALID", "local payload receipt absent"))?;
    if receipt.proof_hash != object_hash("commit-proof", &proof)? {
        return Err(Error::new(
            "PUBLICATION_INVALID",
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
                "PUBLICATION_INVALID",
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
#[path = "../../../test/engine/unit/journal/tail_cache_tests.rs"]
mod tail_cache_tests;

#[cfg(test)]
#[path = "../../../test/engine/unit/journal/scanner_tests.rs"]
mod scanner_tests;

#[cfg(test)]
#[path = "../../../test/engine/unit/journal/writer_tests.rs"]
mod writer_tests;

#[cfg(test)]
#[path = "../../../test/engine/unit/journal/snapshot_partition_tests.rs"]
mod snapshot_partition_tests;
