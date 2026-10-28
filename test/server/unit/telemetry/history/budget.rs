use super::*;

// This oracle deliberately serializes whole queues, independently of the stored
// charges and their update methods. It is not part of the production hot path.
fn full_encoding_bytes(queue: &SampleQueue) -> usize {
    let mut total = 0;
    for entry in &queue.entries {
        let bytes = json_codec::to_vec(&entry.sample).unwrap().len();
        assert_eq!(entry.encoded_bytes, bytes);
        total += bytes;
    }
    assert_eq!(queue.encoded_bytes, total);
    total
}

fn assert_budget(cache: &HistoryStore) -> usize {
    full_encoding_bytes(&cache.minutes) + full_encoding_bytes(&cache.recent)
}

fn oracle_prune(cache: &mut HistoryStore, now: i64) {
    let mut recent = cache.recent.iter().cloned().collect::<VecDeque<_>>();
    let mut minutes = cache.minutes.iter().cloned().collect::<VecDeque<_>>();
    while recent
        .front()
        .is_some_and(|sample| sample.unix_seconds < now - RECENT_SECONDS)
        || recent.len() > MAX_RECENT_POINTS
    {
        recent.pop_front();
    }
    while minutes
        .front()
        .is_some_and(|sample| sample.unix_seconds < now - RETENTION_SECONDS)
        || minutes.len() > MAX_MINUTE_POINTS
    {
        minutes.pop_front();
    }
    if cache.last_maintenance_minute != now / 60 {
        let encoded_bytes = |sample: &Sample| json_codec::to_vec(sample).unwrap().len();
        let mut total = minutes
            .iter()
            .chain(&recent)
            .map(encoded_bytes)
            .sum::<usize>();
        while total > MAX_MEMORY_BYTES && !minutes.is_empty() {
            total -= encoded_bytes(&minutes.pop_front().unwrap());
        }
        while total > MAX_MEMORY_BYTES && recent.len() > 1 {
            total -= encoded_bytes(&recent.pop_front().unwrap());
        }
    }
    cache.prune(now);
    assert_eq!(
        cache
            .recent
            .iter()
            .map(|sample| sample.unix_seconds)
            .collect::<Vec<_>>(),
        recent
            .iter()
            .map(|sample| sample.unix_seconds)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        cache
            .minutes
            .iter()
            .map(|sample| sample.unix_seconds)
            .collect::<Vec<_>>(),
        minutes
            .iter()
            .map(|sample| sample.unix_seconds)
            .collect::<Vec<_>>()
    );
    assert_budget(cache);
}

fn sized_sample(at: i64, encoded_bytes: usize) -> Sample {
    let mut point = sample(at, None);
    point.status.disks.push(crate::telemetry::DiskStatus {
        id: "P01".into(),
        name: String::new(),
        online: false,
        read_only: false,
        simulation: true,
        total_bytes: None,
        used_bytes: None,
        available_bytes: None,
        usage_percent: None,
        power_watts: None,
        power_unavailable_reason: Some(crate::PowerUnavailable::SimulatedDisk),
    });
    let base = json_codec::to_vec(&point).unwrap().len();
    let padding = encoded_bytes.checked_sub(base).unwrap();
    // JSON escapes each NUL to six bytes, keeping the actual 128 MiB budget
    // regression's resident fixture smaller without inventing a test-only limit.
    point.status.disks[0].name = "\0".repeat(padding / 6) + &"x".repeat(padding % 6);
    assert_eq!(json_codec::to_vec(&point).unwrap().len(), encoded_bytes);
    assert!(point.valid_at(at));
    point
}

#[test]
fn accounting_tracks_minutes_overlap_and_rejected_observations() {
    let now = 1_800_000_120;
    let mut cache = HistoryStore::memory(now);
    for offset in (0..130).step_by(5) {
        cache
            .record(sample(now + offset, (offset % 10 == 0).then_some(10.0)))
            .unwrap();
        assert_budget(&cache);
    }
    assert_eq!(cache.minutes.len(), 2);
    assert_eq!(cache.recent.len(), 26);
    // Both retained resolutions are charged, even though queries avoid counting
    // overlapping observations twice; the pending-minute clone is not charged.
    assert_eq!(cache.samples(now, now + 125, 60).len(), 3);
    assert_eq!(cache.pending_minute.len(), 2);
    let before = assert_budget(&cache);
    let pending = json_codec::to_vec(&cache.pending_minute).unwrap();
    let maintenance = cache.last_maintenance_minute;
    cache.record(sample(now + 125, Some(99.0))).unwrap();
    cache.record(sample(now - 60, Some(99.0))).unwrap();
    assert_eq!(assert_budget(&cache), before);
    assert_eq!(json_codec::to_vec(&cache.pending_minute).unwrap(), pending);
    assert_eq!(cache.last_maintenance_minute, maintenance);
    let error = cache.record(sample(now + 130, Some(f64::NAN))).unwrap_err();
    assert_eq!(error.to_string(), "invalid telemetry sample");
    assert_eq!(assert_budget(&cache), before);
    oracle_prune(&mut cache, now - 60);
    oracle_prune(&mut cache, now + RECENT_SECONDS + 180);
    assert_eq!(cache.recent.encoded_bytes, 0);
}

#[test]
fn recovery_charges_canonical_survivors_after_duplicate_replacement() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("cache");
    let now = 1_800_000_120;
    let cache = HistoryStore::open(&path, now).unwrap();
    let recent = [
        sample(now - 115, None),
        sample(now - 115, Some(30.0)),
        sample(now - 110, None),
        sample(now - 55, Some(40.0)),
        sample(now, None),
    ];
    // Whitespace is admitted from disk but is not part of the compact envelope
    // budget. Earlier duplicate lines also still consume the recovery read cap.
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(path.join(format!("recent-{}.jsonl", now / 3600)))
        .unwrap();
    for point in &recent {
        writeln!(file, " \t{}  ", json_codec::to_string(point).unwrap()).unwrap();
    }
    drop(file);
    cache.append_file(false, &sample(now - 59, None)).unwrap();
    cache
        .append_file(false, &sample(now - 54, Some(50.0)))
        .unwrap();
    drop(cache);
    let restored = HistoryStore::open(&path, now).unwrap();
    assert_eq!(restored.recent.len(), 4);
    assert_eq!(restored.minutes.len(), 2);
    assert_eq!(restored.pending_minute.len(), 1);
    assert_eq!(
        restored.recent.front().unwrap().status.cpu.power_watts,
        Some(30.0)
    );
    assert_eq!(restored.minutes.front().unwrap().unix_seconds, now - 110);
    assert_eq!(restored.minutes.back().unwrap().unix_seconds, now - 54);
    assert_eq!(
        restored.minutes.back().unwrap().status.cpu.power_watts,
        Some(50.0)
    );
    let bytes = assert_budget(&restored);
    let query = json_codec::to_vec(&restored.samples(now - 120, now, 60)).unwrap();
    drop(restored);
    let reloaded = HistoryStore::open(&path, now).unwrap();
    assert_eq!(assert_budget(&reloaded), bytes);
    assert_eq!(
        json_codec::to_vec(&reloaded.samples(now - 120, now, 60)).unwrap(),
        query
    );
}

#[test]
fn point_and_age_trimming_subtracts_each_removed_envelope() {
    let now = 1_800_000_120;
    let mut cache = HistoryStore::memory(now);
    cache
        .recent
        .push_back(sample(now - RECENT_SECONDS - 1, None));
    for index in 0..MAX_RECENT_POINTS + 3 {
        cache.recent.push_back(sample(
            now - MAX_RECENT_POINTS as i64 - 3 + index as i64,
            Some(0.0),
        ));
    }
    cache
        .minutes
        .push_back(sample(now - RETENTION_SECONDS - 1, None));
    cache
        .minutes
        .push_back(sample(now - RETENTION_SECONDS, Some(1.0)));
    assert_budget(&cache);
    oracle_prune(&mut cache, now);
    assert_eq!(cache.recent.len(), MAX_RECENT_POINTS);
    assert_eq!(cache.minutes.len(), 1);
    assert_eq!(
        cache.minutes.front().unwrap().unix_seconds,
        now - RETENTION_SECONDS
    );
    oracle_prune(&mut cache, now + RETENTION_SECONDS + 1);
    assert_eq!(cache.minutes.encoded_bytes, 0);
    assert_eq!(cache.recent.encoded_bytes, 0);
}

#[test]
fn exact_byte_boundary_keeps_equal_budget_and_evicts_only_above_it() {
    let now = 1_800_000_120;
    let points = MAX_MEMORY_BYTES / MAX_CACHE_LINE;
    for extra in [0, 1] {
        let mut cache = HistoryStore::memory(now);
        for index in 0..points - 1 {
            cache.minutes.push_back(sized_sample(
                now - (points - index) as i64 * 60,
                MAX_CACHE_LINE,
            ));
        }
        cache
            .recent
            .push_back(sized_sample(now, MAX_CACHE_LINE + extra));
        assert_eq!(assert_budget(&cache), MAX_MEMORY_BYTES + extra);
        // The existing once-per-minute policy permits overshoot until the next
        // maintenance minute; point/age admission still runs immediately.
        oracle_prune(&mut cache, now);
        assert_eq!(cache.minutes.len(), points - 1);
        oracle_prune(&mut cache, now + 60);
        assert_eq!(cache.minutes.len(), points - 1 - extra);
        assert_eq!(cache.recent.len(), 1);
        assert_eq!(
            cache.minutes.encoded_bytes + cache.recent.encoded_bytes,
            MAX_MEMORY_BYTES + extra - extra * MAX_CACHE_LINE
        );
    }
}

#[test]
fn budget_eviction_removes_minutes_before_recent_and_keeps_last_recent() {
    let now = 1_800_000_120;
    let mut cache = HistoryStore::memory(now);
    cache.minutes.push_back(sample(now - 60, None));
    cache.recent.push_back(sample(now - 5, None));
    cache
        .recent
        .push_back(sized_sample(now, MAX_MEMORY_BYTES + 1));
    oracle_prune(&mut cache, now + 60);
    assert!(cache.minutes.is_empty());
    assert_eq!(cache.minutes.encoded_bytes, 0);
    assert_eq!(cache.recent.len(), 1);
    assert_eq!(cache.recent.front().unwrap().unix_seconds, now);
    assert_eq!(cache.recent.encoded_bytes, MAX_MEMORY_BYTES + 1);
    oracle_prune(&mut cache, now + RECENT_SECONDS + 1);
    assert_eq!(cache.recent.encoded_bytes, 0);
}

#[test]
fn encoding_failure_retains_the_existing_conservative_charge() {
    struct FailsEncoding;
    impl Serialize for FailsEncoding {
        fn serialize<S: data_serialization::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(data_serialization::ser::Error::custom(
                "deliberate encoding failure",
            ))
        }
    }
    // The current Sample envelope has no fallible custom serializer. Exercise
    // the actual codec's failure result without a production fault-injection hook.
    let failure = json_codec::to_vec(&FailsEncoding);
    assert!(
        failure
            .as_ref()
            .unwrap_err()
            .to_string()
            .contains("deliberate encoding failure")
    );
    assert_eq!(memory_charge(failure), MAX_CACHE_LINE);
    assert_eq!(
        memory_charge(json_codec::to_vec(&sample(1_800_000_120, None))),
        json_codec::to_vec(&sample(1_800_000_120, None))
            .unwrap()
            .len()
    );
}

#[test]
fn real_write_failures_keep_recent_error_priority_and_memory_accounting() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("cache");
    let now = 1_800_000_120;
    let mut cache = HistoryStore::open(&path, now).unwrap();
    // The minute append will fail at open (a directory occupies its filename).
    fs::create_dir(path.join(format!("minute-{}.jsonl", now / 86400))).unwrap();
    cache.record(sample(now, None)).unwrap();
    // The later recent append fails its distinct size admission; that remains
    // the returned error, and both observations remain in bounded memory.
    let oversized = sized_sample(now + 60, MAX_CACHE_LINE + 1);
    let error = cache.record(oversized).unwrap_err();
    assert_eq!(
        error.to_string(),
        "telemetry sample exceeds cache admission"
    );
    assert!(!cache.is_persistent());
    assert_eq!(cache.minutes.len(), 1);
    assert_eq!(cache.recent.len(), 2);
    assert_budget(&cache);
    cache.record(sample(now + 65, None)).unwrap();
    assert_budget(&cache);

    let path = root.path().join("unwritable-cache-file");
    let mut cache = HistoryStore::open(&path, now).unwrap();
    fs::create_dir(path.join(format!("recent-{}.jsonl", now / 3600))).unwrap();
    let error = cache.record(sample(now, None)).unwrap_err();
    assert!(error.raw_os_error().is_some());
    assert!(!cache.is_persistent());
    assert_eq!(cache.recent.len(), 1);
    assert_budget(&cache);
    cache.record(sample(now + 5, None)).unwrap();
    assert_budget(&cache);
}
