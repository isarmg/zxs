//! Bounded, rebuildable monitoring cache; never part of storage publication.
use super::{Snapshot, statistics::Statistics};
use data_serialization::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
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

pub(super) struct HistoryStore {
    directory: Option<PathBuf>,
    recent: VecDeque<Sample>,
    minutes: VecDeque<Sample>,
    pending_minute: Vec<Sample>,
    last_maintenance_minute: i64,
}

impl HistoryStore {
    pub(super) fn memory(now: i64) -> Self {
        Self {
            directory: None,
            recent: VecDeque::new(),
            minutes: VecDeque::new(),
            pending_minute: Vec::new(),
            last_maintenance_minute: now / 60,
        }
    }

    #[cfg(test)]
    pub(super) fn is_persistent(&self) -> bool {
        self.directory.is_some()
    }

    pub(super) fn open(directory: &Path, now: i64) -> io::Result<Self> {
        ensure_private_directory(directory)?;
        let mut result = Self {
            directory: Some(directory.into()),
            recent: VecDeque::new(),
            minutes: VecDeque::new(),
            pending_minute: Vec::new(),
            last_maintenance_minute: -1,
        };
        result.maintenance(now)?;
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
            let Ok(bytes) = read_bounded_file(&entry.path()) else {
                continue;
            };
            for line in bytes.split(|byte| *byte == b'\n') {
                if line.is_empty()
                    || line.len() > MAX_CACHE_LINE
                    || recovery_bytes.saturating_add(line.len()) > MAX_MEMORY_BYTES
                {
                    continue;
                }
                let Ok(sample) = json_codec::from_slice::<Sample>(line) else {
                    continue;
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
        for sample in &result.recent {
            recovered
                .entry(sample.unix_seconds / 60)
                .or_default()
                .push(sample.clone());
        }
        for (minute, samples) in recovered {
            if minute == now / 60 {
                result.pending_minute = samples;
            } else if !result
                .minutes
                .iter()
                .any(|sample| sample.unix_seconds / 60 == minute)
            {
                let sample = average(&samples);
                if let Err(error) = result.append_file(false, &sample) {
                    tracing::warn!(error = ?error, "monitoring cache recovery is read-only; using bounded memory history");
                    result.directory = None;
                }
                result.minutes.push_back(sample);
            }
        }
        result
            .minutes
            .make_contiguous()
            .sort_by_key(|sample| sample.unix_seconds);
        result.last_maintenance_minute = -1;
        result.prune(now);
        result.last_maintenance_minute = now / 60;
        Ok(result)
    }

    pub(super) fn record(&mut self, sample: Sample) -> io::Result<()> {
        let now = sample.unix_seconds;
        if !sample.valid_at(now) {
            return Err(io::Error::other("invalid telemetry sample"));
        }
        // Repeated observations in the same second never multiply history points.
        if self
            .recent
            .back()
            .is_some_and(|previous| previous.unix_seconds >= now)
        {
            return Ok(());
        }
        let mut error = None;
        if self
            .pending_minute
            .first()
            .is_some_and(|pending| pending.unix_seconds / 60 != now / 60)
        {
            let minute = average(&self.pending_minute);
            if let Err(failure) = self.append_file(false, &minute) {
                error = Some(failure);
            }
            self.minutes.push_back(minute);
            self.pending_minute.clear();
        }
        self.pending_minute.push(sample.clone());
        if let Err(failure) = self.append_file(true, &sample) {
            error = Some(failure);
        }
        self.recent.push_back(sample);
        self.prune(now);
        if error.is_some() {
            self.directory = None;
        }
        if self.last_maintenance_minute != now / 60
            && let Err(failure) = self.maintenance(now)
        {
            error = Some(failure);
        }
        match error {
            Some(error) => Err(error),
            None => Ok(()),
        }
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
        for sample in &self.minutes {
            if sample.unix_seconds >= from
                && sample.unix_seconds <= to
                && !recent_minutes.contains(&(sample.unix_seconds / 60))
            {
                source.insert(sample.unix_seconds, sample.clone());
            }
        }
        for sample in &self.recent {
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

    fn prune(&mut self, now: i64) {
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
        let approximate = |points: &VecDeque<Sample>| {
            points
                .iter()
                .map(|sample| {
                    json_codec::to_vec(sample).map_or(MAX_CACHE_LINE, |bytes| bytes.len())
                })
                .sum::<usize>()
        };
        // Check memory admission only once per minute; the point-count bound is
        // enforced on every observation. 3-drive installations retain 31 days.
        if self.last_maintenance_minute != now / 60 {
            let mut bytes = approximate(&self.minutes) + approximate(&self.recent);
            while bytes > MAX_MEMORY_BYTES && !self.minutes.is_empty() {
                if let Some(sample) = self.minutes.pop_front() {
                    bytes = bytes.saturating_sub(
                        json_codec::to_vec(&sample).map_or(MAX_CACHE_LINE, |value| value.len()),
                    );
                }
            }
            while bytes > MAX_MEMORY_BYTES && self.recent.len() > 1 {
                if let Some(sample) = self.recent.pop_front() {
                    bytes = bytes.saturating_sub(
                        json_codec::to_vec(&sample).map_or(MAX_CACHE_LINE, |value| value.len()),
                    );
                }
            }
        }
    }

    fn append_file(&self, recent: bool, sample: &Sample) -> io::Result<()> {
        let Some(directory) = &self.directory else {
            return Ok(());
        };
        let bytes = json_codec::to_vec(sample)?;
        if bytes.len() > MAX_CACHE_LINE {
            return Err(io::Error::other("telemetry sample exceeds cache admission"));
        }
        let bucket = sample.unix_seconds / if recent { 3600 } else { 86400 };
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
