use super::*;
use std::sync::{Arc, mpsc};

#[test]
fn invalid_duplicate_and_backward_samples_fail_before_time_rejection() {
    let now = 1_800_000_120;
    let mut cache = HistoryFixture::memory(now);
    cache.record(sample(now, Some(10.0))).unwrap();
    let before = json_codec::to_vec(&cache.samples(now - 60, now + 60, 5)).unwrap();
    for at in [now, now - 60] {
        let error = cache.record(sample(at, Some(f64::NAN))).unwrap_err();
        assert_eq!(error.to_string(), "invalid telemetry sample");
        cache.record(sample(at, Some(20.0))).unwrap();
        assert_eq!(cache.writer.last_recorded_seconds, Some(now));
        assert_eq!(cache.writer.pending_minute.len(), 1);
        assert_eq!(cache.writer.last_maintenance_minute, now / 60);
        assert_eq!(
            json_codec::to_vec(&cache.samples(now - 60, now + 60, 5)).unwrap(),
            before
        );
    }
}

#[test]
fn recovered_writer_uses_latest_second_and_pending_minute_without_duplicate_appends() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("cache");
    let now = 1_800_000_120;
    let mut cache = HistoryFixture::open(&path, now).unwrap();
    cache.record(sample(now, Some(10.0))).unwrap();
    cache.record(sample(now + 5, Some(20.0))).unwrap();
    drop(cache);

    let mut cache = HistoryFixture::open(&path, now + 5).unwrap();
    assert_eq!(cache.writer.last_recorded_seconds, Some(now + 5));
    assert_eq!(cache.writer.pending_minute.len(), 2);
    let recent_path = path.join(format!("recent-{}.jsonl", now / 3600));
    let before = fs::read(&recent_path).unwrap();
    for at in [now + 5, now] {
        cache.record(sample(at, Some(99.0))).unwrap();
    }
    assert_eq!(fs::read(&recent_path).unwrap(), before);
    assert_eq!(cache.recent.len(), 2);
    cache.record(sample(now + 60, Some(30.0))).unwrap();
    let minute = cache.minutes.back().unwrap();
    assert_eq!(minute.observations, 2);
    assert_eq!(minute.status.cpu.power_watts, Some(15.0));
    assert_eq!(cache.writer.pending_minute.len(), 1);
    budget::assert_budget(&cache);
}

#[test]
fn minute_and_recent_append_results_preserve_attempt_order_and_memory_publication() {
    let now = 1_800_000_120;
    for minute_fails in [false, true] {
        for recent_fails in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("cache");
            let mut cache = HistoryFixture::open(&path, now).unwrap();
            cache.record(sample(now, Some(10.0))).unwrap();
            let minute_path = path.join(format!("minute-{}.jsonl", now / 86400));
            let recent_path = path.join(format!("recent-{}.jsonl", now / 3600));
            if minute_fails {
                fs::create_dir(&minute_path).unwrap();
            }
            let next = if recent_fails {
                budget::sized_sample(now + 60, MAX_CACHE_LINE + 1)
            } else {
                sample(now + 60, Some(20.0))
            };
            let result = cache.record(next);
            if recent_fails {
                assert_eq!(
                    result.unwrap_err().to_string(),
                    "telemetry sample exceeds cache admission"
                );
            } else if minute_fails {
                assert!(result.unwrap_err().raw_os_error().is_some());
            } else {
                result.unwrap();
            }
            assert_eq!(cache.is_persistent(), !minute_fails && !recent_fails);
            assert_eq!(cache.minutes.len(), 1);
            assert_eq!(cache.recent.len(), 2);
            assert_eq!(cache.writer.pending_minute.len(), 1);
            assert_eq!(cache.writer.last_recorded_seconds, Some(now + 60));
            budget::assert_budget(&cache);
            let recent_bytes = fs::read(&recent_path).unwrap();
            assert_eq!(
                recent_bytes.iter().filter(|byte| **byte == b'\n').count(),
                if recent_fails { 1 } else { 2 }
            );
            if !minute_fails {
                let bytes = fs::read(&minute_path).unwrap();
                let mut expected =
                    json_codec::to_vec(&average(&[sample(now, Some(10.0))])).unwrap();
                expected.push(b'\n');
                assert_eq!(bytes, expected);
            }
            if minute_fails || recent_fails {
                cache.record(sample(now + 65, Some(30.0))).unwrap();
                assert_eq!(fs::read(&recent_path).unwrap(), recent_bytes);
                assert_eq!(cache.recent.len(), 3);
                budget::assert_budget(&cache);
            }
        }
    }
}

#[test]
fn preparation_keeps_old_reader_view_until_both_points_and_pruning_publish() {
    let now = 1_800_000_120;
    let root = tempfile::tempdir().unwrap();
    let (mut store, mut writer) = HistoryWriter::open(&root.path().join("cache"), now).unwrap();
    store
        .recent
        .push_back(sample(now - RECENT_SECONDS, Some(1.0)));
    let history = Arc::new(Mutex::new(store));
    writer.record(sample(now, Some(10.0)), &history).unwrap();
    let (prepared_tx, prepared_rx) = mpsc::channel();
    let (publish_tx, publish_rx) = mpsc::channel();
    let worker_history = Arc::clone(&history);
    // Holding the reader guard while preparation finishes also checks that
    // input validation, encoding, append and maintenance require no history lock.
    let reader = history.lock().unwrap();
    let worker = std::thread::spawn(move || {
        let prepared = writer
            .prepare(sample(now + 60, Some(20.0)))
            .unwrap()
            .unwrap();
        prepared_tx.send(()).unwrap();
        publish_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        worker_history.lock().unwrap().publish(prepared).unwrap();
    });
    prepared_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    assert!(reader.minutes.is_empty());
    assert_eq!(reader.recent.len(), 2);
    assert_eq!(
        reader.recent.front().unwrap().unix_seconds,
        now - RECENT_SECONDS
    );
    assert_eq!(reader.recent.back().unwrap().unix_seconds, now);
    assert_eq!(reader.samples(now - RECENT_SECONDS, now + 60, 5).len(), 2);
    assert_eq!(
        reader
            .samples(now - RECENT_SECONDS, now + 60, 5)
            .iter()
            .map(|point| point.cpu.power_watts)
            .collect::<Vec<_>>(),
        [Some(1.0), Some(10.0)]
    );
    publish_tx.send(()).unwrap();
    // Publication cannot expose only the minute while this old view is held.
    assert!(reader.minutes.is_empty());
    drop(reader);
    worker.join().unwrap();
    let reader = history.lock().unwrap();
    assert_eq!(reader.minutes.len(), 1);
    assert_eq!(reader.recent.len(), 2);
    assert_eq!(reader.recent.front().unwrap().unix_seconds, now);
    assert_eq!(reader.recent.back().unwrap().unix_seconds, now + 60);
    assert_eq!(reader.samples(now - RECENT_SECONDS, now + 60, 5).len(), 2);
    assert_eq!(
        reader
            .samples(now - RECENT_SECONDS, now + 60, 5)
            .iter()
            .map(|point| point.cpu.power_watts)
            .collect::<Vec<_>>(),
        [Some(10.0), Some(20.0)]
    );
    budget::assert_budget(&reader);
}

#[cfg(unix)]
#[test]
fn maintenance_failure_retries_without_downgrade_and_preserves_old_byte_gate() {
    use std::os::unix::fs::PermissionsExt;
    // The normal workspace CI runs as the unprivileged runner. Root bypasses
    // this real permission boundary and must not turn the test into a silent pass.
    assert_ne!(
        rustix::process::geteuid().as_raw(),
        0,
        "permission failure regression requires a non-root runner"
    );
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("cache");
    let now = 1_800_000_120;
    let mut cache = HistoryFixture::open(&path, now).unwrap();
    cache.record(sample(now, None)).unwrap();
    // Existing files remain writable, but unlinking the expired cache is denied.
    fs::write(path.join(format!("minute-{}.jsonl", now / 86400)), []).unwrap();
    let expired = path.join("minute-1.jsonl");
    fs::write(&expired, b"expired").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o500)).unwrap();
    for at in [now + 60, now + 65] {
        let prepared = cache.writer.prepare(sample(at, None)).unwrap().unwrap();
        assert!(prepared.prune_bytes);
        assert!(cache.is_persistent());
        assert_eq!(cache.writer.last_maintenance_minute, now / 60);
        let error = cache.store.publish(prepared).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(cache.recent.back().unwrap().unix_seconds, at);
        assert!(expired.is_file());
        assert_eq!(fs::read(&expired).unwrap(), b"expired");
        budget::assert_budget(&cache);
    }
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    let prepared = cache
        .writer
        .prepare(sample(now + 70, None))
        .unwrap()
        .unwrap();
    assert!(
        prepared.prune_bytes,
        "successful maintenance must not reset this record's old-minute gate"
    );
    assert_eq!(cache.writer.last_maintenance_minute, (now + 70) / 60);
    cache.store.publish(prepared).unwrap();
    assert!(!expired.exists());
    assert!(cache.is_persistent());
    let prepared = cache
        .writer
        .prepare(sample(now + 75, None))
        .unwrap()
        .unwrap();
    assert!(!prepared.prune_bytes);
    cache.store.publish(prepared).unwrap();
    budget::assert_budget(&cache);
}

#[test]
fn encoded_error_is_charged_conservatively_but_only_persistent_append_reports_it() {
    struct FailsEncoding;
    impl Serialize for FailsEncoding {
        fn serialize<S: data_serialization::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(data_serialization::ser::Error::custom(
                "deliberate append encoding failure",
            ))
        }
    }
    let root = tempfile::tempdir().unwrap();
    let now = 1_800_000_120;
    let (_, persistent) = HistoryWriter::open(&root.path().join("cache"), now).unwrap();
    let (_, memory) = HistoryWriter::memory(now);
    for writer in [persistent, memory] {
        let encoded = json_codec::to_vec(&FailsEncoding);
        assert_eq!(memory_charge(&encoded), MAX_CACHE_LINE);
        let result = writer.append_file(true, now, encoded);
        if writer.directory.is_some() {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("deliberate append encoding failure")
            );
        } else {
            result.unwrap();
        }
    }
}
