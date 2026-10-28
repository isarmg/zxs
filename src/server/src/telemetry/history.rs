//! Bounded, rebuildable monitoring cache; never part of storage publication.
use super::{Snapshot, statistics::Statistics};
use data_serialization::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

pub(super) const RETENTION_SECONDS: i64 = 31 * 24 * 60 * 60;
const RECENT_SECONDS: i64 = 60 * 60;
const MAX_MINUTE_POINTS: usize = (RETENTION_SECONDS / 60) as usize + 2;
const MAX_RECENT_POINTS: usize = (RECENT_SECONDS / 5) as usize + 2;
const MAX_CACHE_FILE: u64 = 16 * 1024 * 1024;
const MAX_CACHE_LINE: usize = 64 * 1024;
const MAX_MEMORY_BYTES: usize = 128 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Sample {
    pub unix_seconds: i64,
    pub observations: u32,
    #[serialization(default, skip_serializing_if = "Option::is_none")]
    pub statistics: Option<Statistics>,
    pub status: Snapshot,
}

impl Sample {
    fn valid_at(&self, now: i64) -> bool {
        self.unix_seconds >= now - RETENTION_SECONDS
            && self.unix_seconds <= now
            && super::timestamp(self.unix_seconds).ok().as_deref()
                == Some(self.status.collected_at.as_str())
            && (1..=60).contains(&self.observations)
            && self
                .statistics
                .as_ref()
                .map_or(self.observations == 1, |statistics| {
                    statistics.valid_for(&self.status, self.observations)
                })
            && self.status.disks.len() <= 256
            && self
                .status
                .cpu
                .usage_percent
                .is_none_or(|value| value.is_finite() && (0.0..=100.0).contains(&value))
            && self
                .status
                .cpu
                .power_watts
                .is_none_or(|value| value.is_finite() && value >= 0.0)
            && self.status.disks.iter().all(|disk| {
                disk.power_watts
                    .is_none_or(|value| value.is_finite() && value >= 0.0)
            })
    }
}

struct ChargedSample {
    sample: Sample,
    encoded_bytes: usize,
}

// Entries expose only shared samples: changing an envelope must also replace its
// charge. Sorting moves the sample and charge together; removal never re-encodes.
#[derive(Default)]
struct SampleQueue {
    entries: VecDeque<ChargedSample>,
    encoded_bytes: usize,
}

impl SampleQueue {
    fn push_back(&mut self, sample: Sample) {
        let encoded_bytes = memory_charge(&json_codec::to_vec(&sample));
        self.push_charged(ChargedSample {
            sample,
            encoded_bytes,
        });
    }

    fn push_charged(&mut self, sample: ChargedSample) {
        self.encoded_bytes += sample.encoded_bytes;
        self.entries.push_back(sample);
    }

    fn pop_front(&mut self) {
        let entry = self
            .entries
            .pop_front()
            .expect("invariant: telemetry pruning removes an existing sample");
        self.encoded_bytes -= entry.encoded_bytes;
    }

    fn iter(&self) -> impl Iterator<Item = &Sample> {
        self.entries.iter().map(|entry| &entry.sample)
    }

    fn front(&self) -> Option<&Sample> {
        self.entries.front().map(|entry| &entry.sample)
    }

    fn back(&self) -> Option<&Sample> {
        self.entries.back().map(|entry| &entry.sample)
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn sort_by_time(&mut self) {
        self.entries
            .make_contiguous()
            .sort_by_key(|entry| entry.sample.unix_seconds);
    }
}

impl FromIterator<Sample> for SampleQueue {
    fn from_iter<T: IntoIterator<Item = Sample>>(samples: T) -> Self {
        let mut result = Self::default();
        for sample in samples {
            result.push_back(sample);
        }
        result
    }
}

fn memory_charge(encoded: &json_codec::Result<Vec<u8>>) -> usize {
    // Retain the existing conservative accounting fallback. Persistence reports
    // its own encoding/I/O failure; accounting must not change that precedence.
    encoded.as_ref().map_or(MAX_CACHE_LINE, |bytes| bytes.len())
}

#[derive(Default)]
pub(super) struct HistoryStore {
    recent: SampleQueue,
    minutes: SampleQueue,
}

// Startup owns this value until its first refresh, then moves it into the single
// sampling thread. HTTP handles hold only HistoryStore, never a writer.
pub(super) struct HistoryWriter {
    directory: Option<PathBuf>,
    pending_minute: Vec<Sample>,
    last_maintenance_minute: i64,
    last_recorded_seconds: Option<i64>,
}

struct PreparedRecord {
    minute: Option<ChargedSample>,
    recent: ChargedSample,
    prune_bytes: bool,
    persistence: io::Result<()>,
}

impl HistoryWriter {
    pub(super) fn memory(now: i64) -> (HistoryStore, Self) {
        (
            HistoryStore::default(),
            Self {
                directory: None,
                pending_minute: Vec::new(),
                last_maintenance_minute: now / 60,
                last_recorded_seconds: None,
            },
        )
    }

    pub(super) fn open(directory: &Path, now: i64) -> io::Result<(HistoryStore, Self)> {
        ensure_private_directory(directory)?;
        let mut writer = Self {
            directory: Some(directory.into()),
            pending_minute: Vec::new(),
            last_maintenance_minute: -1,
            last_recorded_seconds: None,
        };
        writer.maintenance(now)?;
        let mut result = HistoryStore::default();
        let mut files = fs::read_dir(directory)?
            .filter_map(Result::ok)
            .filter(|entry| {
                cache_name(&entry.file_name().to_string_lossy())
                    .is_some_and(|(recent, bucket)| file_in_window(recent, bucket, now))
            })
            .take(36)
            .collect::<Vec<_>>();
        files.sort_by_key(|entry| std::cmp::Reverse(entry.file_name()));
        let mut recovery_bytes = 0_usize;
        let mut recent = BTreeMap::new();
        let mut minutes = BTreeMap::new();
        // At most 32 daily files and 2 recent hourly files pass admission.
        for entry in files {
            let Some((recent_file, bucket)) = cache_name(&entry.file_name().to_string_lossy())
            else {
                continue;
            };
            if !file_in_window(recent_file, bucket, now) {
                continue;
            }
            let metadata = entry.path().symlink_metadata()?;
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() > MAX_CACHE_FILE
            {
                continue;
            }
            let bytes = match read_bounded_file(&entry.path()) {
                Ok(bytes) => bytes,
                Err(error) => {
                    tracing::warn!(error = ?error, "unreadable monitoring cache skipped");
                    continue;
                }
            };
            for line in bytes.split(|byte| *byte == b'\n') {
                if line.is_empty()
                    || line.len() > MAX_CACHE_LINE
                    || recovery_bytes.saturating_add(line.len()) > MAX_MEMORY_BYTES
                {
                    continue;
                }
                let sample = match json_codec::from_slice::<Sample>(line) {
                    Ok(sample) => sample,
                    Err(error) => {
                        tracing::debug!(error = ?error, "invalid monitoring cache sample skipped");
                        continue;
                    }
                };
                if !sample.valid_at(now) {
                    continue;
                }
                recovery_bytes += line.len();
                if recent_file
                    && sample.unix_seconds >= now - RECENT_SECONDS
                    && sample.unix_seconds / 3600 == bucket
                {
                    recent.insert(sample.unix_seconds, sample);
                } else if !recent_file && sample.unix_seconds / 86400 == bucket {
                    minutes.insert(sample.unix_seconds / 60, sample);
                }
            }
        }
        result.recent = recent.into_values().collect();
        result.minutes = minutes.into_values().collect();
        // A restart can interrupt the last minute. Recover completed minute
        // aggregates from real recent samples without creating any missing data.
        let mut recovered = BTreeMap::<i64, Vec<Sample>>::new();
        for sample in result.recent.iter() {
            recovered
                .entry(sample.unix_seconds / 60)
                .or_default()
                .push(sample.clone());
        }
        for (minute, samples) in recovered {
            if minute == now / 60 {
                writer.pending_minute = samples;
            } else if !result
                .minutes
                .iter()
                .any(|sample| sample.unix_seconds / 60 == minute)
            {
                let sample = average(&samples);
                let (sample, persistence) = writer.prepare_sample(false, sample);
                if let Err(error) = persistence {
                    tracing::warn!(error = ?error, "monitoring cache recovery is read-only; using bounded memory history");
                    writer.directory = None;
                }
                result.minutes.push_charged(sample);
            }
        }
        result.minutes.sort_by_time();
        result.prune(now, -1 != now / 60);
        writer.last_maintenance_minute = now / 60;
        writer.last_recorded_seconds = result.recent.back().map(|sample| sample.unix_seconds);
        Ok((result, writer))
    }

    pub(super) fn record(
        &mut self,
        sample: Sample,
        history: &Mutex<HistoryStore>,
    ) -> io::Result<()> {
        let Some(prepared) = self.prepare(sample)? else {
            return Ok(());
        };
        // The sole writer synchronously publishes every accepted record, even
        // after I/O failure. No competing writer can invalidate its preparation.
        history
            .lock()
            .expect("service state lock poisoned")
            .publish(prepared)
    }

    fn prepare(&mut self, sample: Sample) -> io::Result<Option<PreparedRecord>> {
        let now = sample.unix_seconds;
        if !sample.valid_at(now) {
            return Err(io::Error::other("invalid telemetry sample"));
        }
        // Repeated observations in the same second never multiply history points.
        if self
            .last_recorded_seconds
            .is_some_and(|previous| previous >= now)
        {
            return Ok(None);
        }
        let mut error = None;
        let minute = if self
            .pending_minute
            .first()
            .is_some_and(|pending| pending.unix_seconds / 60 != now / 60)
        {
            let minute = average(&self.pending_minute);
            let (minute, persistence) = self.prepare_sample(false, minute);
            if let Err(failure) = persistence {
                error = Some(failure);
            }
            self.pending_minute.clear();
            Some(minute)
        } else {
            None
        };
        self.pending_minute.push(sample.clone());
        let (recent, persistence) = self.prepare_sample(true, sample);
        if let Err(failure) = persistence {
            error = Some(failure);
        }
        // Preserve the old minute's byte gate even when maintenance succeeds
        // before publication. A maintenance-only failure leaves this gate due.
        let prune_bytes = self.last_maintenance_minute != now / 60;
        if error.is_some() {
            self.directory = None;
        }
        if prune_bytes && let Err(failure) = self.maintenance(now) {
            error = Some(failure);
        }
        self.last_recorded_seconds = Some(now);
        Ok(Some(PreparedRecord {
            minute,
            recent,
            prune_bytes,
            persistence: error.map_or(Ok(()), Err),
        }))
    }

    fn prepare_sample(&self, recent: bool, sample: Sample) -> (ChargedSample, io::Result<()>) {
        let encoded = json_codec::to_vec(&sample);
        let encoded_bytes = memory_charge(&encoded);
        let persistence = self.append_file(recent, sample.unix_seconds, encoded);
        (
            ChargedSample {
                sample,
                encoded_bytes,
            },
            persistence,
        )
    }
}

impl HistoryStore {
    fn publish(&mut self, prepared: PreparedRecord) -> io::Result<()> {
        let PreparedRecord {
            minute,
            recent,
            prune_bytes,
            persistence,
        } = prepared;
        let now = recent.sample.unix_seconds;
        if let Some(minute) = minute {
            self.minutes.push_charged(minute);
        }
        self.recent.push_charged(recent);
        self.prune(now, prune_bytes);
        persistence
    }

    pub(super) fn samples(&self, from: i64, to: i64, interval: i64) -> Vec<Snapshot> {
        let mut source = BTreeMap::<i64, Sample>::new();
        // Recent full samples replace their minute aggregate; they are never
        // averaged twice merely because two cache resolutions cover one period.
        let recent_minutes = self
            .recent
            .iter()
            .filter(|sample| sample.unix_seconds >= from && sample.unix_seconds <= to)
            .map(|sample| sample.unix_seconds / 60)
            .collect::<std::collections::BTreeSet<_>>();
        for sample in self.minutes.iter() {
            if sample.unix_seconds >= from
                && sample.unix_seconds <= to
                && !recent_minutes.contains(&(sample.unix_seconds / 60))
            {
                source.insert(sample.unix_seconds, sample.clone());
            }
        }
        for sample in self.recent.iter() {
            if sample.unix_seconds >= from && sample.unix_seconds <= to {
                source.insert(sample.unix_seconds, sample.clone());
            }
        }
        let mut buckets = BTreeMap::<i64, Vec<Sample>>::new();
        for sample in source.into_values() {
            buckets
                .entry((sample.unix_seconds - from) / interval)
                .or_default()
                .push(sample);
        }
        buckets
            .into_values()
            .map(|samples| average(&samples).status)
            .collect()
    }

    fn prune(&mut self, now: i64, prune_bytes: bool) {
        while self
            .recent
            .front()
            .is_some_and(|sample| sample.unix_seconds < now - RECENT_SECONDS)
            || self.recent.len() > MAX_RECENT_POINTS
        {
            self.recent.pop_front();
        }
        while self
            .minutes
            .front()
            .is_some_and(|sample| sample.unix_seconds < now - RETENTION_SECONDS)
            || self.minutes.len() > MAX_MINUTE_POINTS
        {
            self.minutes.pop_front();
        }
        // Check memory admission only once per minute; the point-count bound is
        // enforced on every observation. 3-drive installations retain 31 days.
        if prune_bytes {
            while self.minutes.encoded_bytes + self.recent.encoded_bytes > MAX_MEMORY_BYTES
                && !self.minutes.is_empty()
            {
                self.minutes.pop_front();
            }
            while self.minutes.encoded_bytes + self.recent.encoded_bytes > MAX_MEMORY_BYTES
                && self.recent.len() > 1
            {
                self.recent.pop_front();
            }
        }
    }
}

impl HistoryWriter {
    fn append_file(
        &self,
        recent: bool,
        unix_seconds: i64,
        encoded: json_codec::Result<Vec<u8>>,
    ) -> io::Result<()> {
        let Some(directory) = &self.directory else {
            return Ok(());
        };
        let bytes = encoded?;
        if bytes.len() > MAX_CACHE_LINE {
            return Err(io::Error::other("telemetry sample exceeds cache admission"));
        }
        let bucket = unix_seconds / if recent { 3600 } else { 86400 };
        let name = format!(
            "{}-{bucket}.jsonl",
            if recent { "recent" } else { "minute" }
        );
        let path = directory.join(name);
        let mut options = OpenOptions::new();
        options.append(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
            #[cfg(target_os = "linux")]
            options.custom_flags(0x20000 | 0x800); // O_NOFOLLOW | O_NONBLOCK
        }
        let mut file = options.open(&path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() + bytes.len() as u64 + 1 > MAX_CACHE_FILE {
            return Err(io::Error::other("telemetry cache file exceeds admission"));
        }
        file.write_all(&bytes)?;
        file.write_all(b"\n")?;
        Ok(())
    }

    fn maintenance(&mut self, now: i64) -> io::Result<()> {
        let Some(directory) = &self.directory else {
            self.last_maintenance_minute = now / 60;
            return Ok(());
        };
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let Some((recent, bucket)) = cache_name(&entry.file_name().to_string_lossy()) else {
                continue;
            };
            let metadata = entry.path().symlink_metadata()?;
            if metadata.is_file()
                && !metadata.file_type().is_symlink()
                && !file_in_window(recent, bucket, now)
            {
                fs::remove_file(entry.path())?;
            }
        }
        self.last_maintenance_minute = now / 60;
        Ok(())
    }
}

fn cache_name(name: &str) -> Option<(bool, i64)> {
    let (kind, bucket) = name.strip_suffix(".jsonl")?.split_once('-')?;
    let recent = match kind {
        "recent" => true,
        "minute" => false,
        _ => return None,
    };
    if bucket.is_empty() || !bucket.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let parsed = bucket.parse::<i64>().ok()?;
    if parsed.to_string() != bucket {
        return None;
    }
    Some((recent, parsed))
}

fn file_in_window(recent: bool, bucket: i64, now: i64) -> bool {
    let seconds = if recent { 3600 } else { 86400 };
    let retention = if recent {
        RECENT_SECONDS
    } else {
        RETENTION_SECONDS
    };
    ((now - retention) / seconds..=now / seconds).contains(&bucket)
}

fn ensure_private_directory(directory: &Path) -> io::Result<()> {
    match directory.symlink_metadata() {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "telemetry cache must not be a symbolic link",
            ));
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "telemetry cache directory is occupied",
            ));
        }
        Ok(_) => (),
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(directory)?,
        Err(error) => return Err(error),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn read_bounded_file(path: &Path) -> io::Result<Vec<u8>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x20000 | 0x800);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Ok(Vec::new());
    }
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(file, MAX_CACHE_FILE + 1),
        &mut bytes,
    )?;
    if bytes.len() as u64 > MAX_CACHE_FILE {
        return Ok(Vec::new());
    }
    Ok(bytes)
}

pub(super) fn average(samples: &[Sample]) -> Sample {
    let mut result = samples
        .last()
        .expect("nonempty telemetry aggregate")
        .clone();
    result.observations = samples.iter().map(|sample| sample.observations).sum();
    let mut statistics = Statistics::default();
    let mut disks = BTreeMap::new();
    for sample in samples {
        statistics.merge(
            &sample
                .statistics
                .clone()
                .unwrap_or_else(|| Statistics::observation(&sample.status)),
        );
        for disk in &sample.status.disks {
            disks.insert(disk.id.clone(), disk.clone());
        }
    }
    result.status.disks = disks.into_values().collect();
    statistics.apply(&mut result.status);
    result.statistics = Some(statistics);
    result
}

#[cfg(test)]
#[path = "../../../../test/server/unit/telemetry/history/tests.rs"]
mod tests;
