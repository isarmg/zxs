use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct CountingReader {
    inner: std::io::Cursor<Vec<u8>>,
    bytes: Arc<AtomicUsize>,
}
impl std::io::Read for CountingReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let n = std::io::Read::read(&mut self.inner, buffer)?;
        self.bytes.fetch_add(n, Ordering::SeqCst);
        Ok(n)
    }
}
impl Seek for CountingReader {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(position)
    }
}
fn counted(bytes: Vec<u8>, count: Arc<AtomicUsize>) -> CountingReader {
    CountingReader {
        inner: std::io::Cursor::new(bytes),
        bytes: count,
    }
}
fn manifest(bytes: &[u8]) -> storage_contract::ChecksumManifest {
    storage_contract::ChecksumManifest {
        chunk_bytes: 65536,
        logical_size: bytes.len() as u64,
        chunks: bytes
            .chunks(65536)
            .map(|chunk| *blake3::hash(chunk).as_bytes())
            .collect(),
    }
}
#[test]
fn verified_ranges_read_only_touched_chunks_and_fallback_without_exposing_corruption() {
    let bytes = vec![42; 65536 * 4 + 7];
    let count = Arc::new(AtomicUsize::new(0));
    let mut reader = VerifiedRange::new(
        counted(bytes.clone(), count.clone()),
        vec![],
        manifest(&bytes),
        65535,
        2,
    )
    .unwrap();
    assert_eq!(reader.next_chunk().unwrap().unwrap(), [42]);
    assert_eq!(reader.next_chunk().unwrap().unwrap(), [42]);
    assert!(reader.next_chunk().unwrap().is_none());
    assert_eq!(count.load(Ordering::SeqCst), 2 * 65536);
    let mut corrupt = bytes.clone();
    corrupt[65536] = 0;
    let bad_count = Arc::new(AtomicUsize::new(0));
    let good_count = Arc::new(AtomicUsize::new(0));
    let mut reader = VerifiedRange::new(
        counted(corrupt.clone(), bad_count.clone()),
        vec![counted(bytes.clone(), good_count.clone())],
        manifest(&bytes),
        65537,
        1,
    )
    .unwrap();
    assert_eq!(reader.next_chunk().unwrap().unwrap(), [42]);
    assert_eq!(bad_count.load(Ordering::SeqCst), 65536);
    assert_eq!(good_count.load(Ordering::SeqCst), 65536);
    let mut reader = VerifiedRange::new(
        counted(corrupt, Arc::default()),
        vec![],
        manifest(&bytes),
        65537,
        1,
    )
    .unwrap();
    assert_eq!(
        reader.next_chunk().unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );
    // A short tail checks only actual remaining bytes, not a padded chunk.
    let tail_count = Arc::new(AtomicUsize::new(0));
    let mut reader = VerifiedRange::new(
        counted(bytes.clone(), tail_count.clone()),
        vec![],
        manifest(&bytes),
        (bytes.len() - 1) as u64,
        1,
    )
    .unwrap();
    assert_eq!(reader.next_chunk().unwrap().unwrap(), [42]);
    assert_eq!(tail_count.load(Ordering::SeqCst), 7);
}

#[test]
fn verified_ranges_prefer_last_verified_replica_without_discarding_others() {
    let bytes = vec![42; 65536 * 3];
    let mut first = bytes.clone();
    first[0] = 0;
    let mut second = bytes.clone();
    second[65536 * 2] = 0;
    let first_reads = Arc::new(AtomicUsize::new(0));
    let second_reads = Arc::new(AtomicUsize::new(0));
    let mut reader = VerifiedRange::new(
        counted(first, first_reads.clone()),
        vec![counted(second, second_reads.clone())],
        manifest(&bytes),
        0,
        bytes.len() as u64,
    )
    .unwrap();
    assert_eq!(reader.next_chunk().unwrap().unwrap(), bytes[..65536]);
    assert_eq!(first_reads.load(Ordering::SeqCst), 65536);
    assert_eq!(second_reads.load(Ordering::SeqCst), 65536);
    // The successful source remains preferred for the next authenticated block.
    assert_eq!(reader.next_chunk().unwrap().unwrap(), bytes[65536..131072]);
    assert_eq!(first_reads.load(Ordering::SeqCst), 65536);
    assert_eq!(second_reads.load(Ordering::SeqCst), 131072);
    // Corruption is per block: the original source must remain eligible.
    assert_eq!(reader.next_chunk().unwrap().unwrap(), bytes[131072..]);
    assert_eq!(first_reads.load(Ordering::SeqCst), 131072);
    assert_eq!(second_reads.load(Ordering::SeqCst), 196608);
    assert!(reader.next_chunk().unwrap().is_none());
}

#[test]
fn verified_ranges_promote_later_fallback_after_corrupt_and_short_replicas() {
    let bytes = vec![42; 65536 * 2];
    let bad_reads = Arc::new(AtomicUsize::new(0));
    let short_reads = Arc::new(AtomicUsize::new(0));
    let good_reads = Arc::new(AtomicUsize::new(0));
    let mut reader = VerifiedRange::new(
        counted(vec![0; bytes.len()], bad_reads.clone()),
        vec![
            counted(vec![42; 7], short_reads.clone()),
            counted(bytes.clone(), good_reads.clone()),
        ],
        manifest(&bytes),
        0,
        bytes.len() as u64,
    )
    .unwrap();
    for expected in bytes.chunks(65536) {
        assert_eq!(reader.next_chunk().unwrap().unwrap(), expected);
    }
    assert!(reader.next_chunk().unwrap().is_none());
    assert_eq!(bad_reads.load(Ordering::SeqCst), 65536);
    assert_eq!(short_reads.load(Ordering::SeqCst), 7);
    assert_eq!(good_reads.load(Ordering::SeqCst), bytes.len());
}

struct ReadLease(Arc<AtomicUsize>);
impl Drop for ReadLease {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
fn content(lease: Arc<AtomicUsize>) -> Content {
    let mut file = tempfile::tempfile().unwrap();
    std::io::Write::write_all(&mut file, b"0123456789").unwrap();
    Content {
        checksum_manifest: None,
        fallback_files: Vec::new(),
        file,
        size: 10,
        hash: "hash".into(),
        lease: Box::new(ReadLease(lease)),
        download_name: None,
    }
}

#[tokio::test]
async fn verified_stream_preserves_lease_and_admission_until_consumption_or_drop() {
    for consume in [true, false] {
        let slots = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = slots.clone().try_acquire_owned().unwrap();
        let dropped = Arc::new(AtomicUsize::new(0));
        let mut content = content(dropped.clone());
        content.checksum_manifest = Some(manifest(b"0123456789"));
        content.lease = Box::new((content.lease, permit));
        let response = content_response(content, Method::GET, HeaderMap::new()).unwrap();
        assert_eq!(dropped.load(Ordering::SeqCst), 0);
        assert_eq!(slots.available_permits(), 0);
        if consume {
            assert_eq!(
                http_routing::body::to_bytes(response.into_body(), 8192)
                    .await
                    .unwrap(),
                "0123456789"
            );
        } else {
            drop(response);
        }
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        assert_eq!(slots.available_permits(), 1);
    }
}

#[tokio::test]
async fn verification_failure_after_a_chunk_stays_a_stream_error() {
    let original = vec![42; 65536 * 2];
    let expected = manifest(&original);
    let mut corrupted = original.clone();
    corrupted[65536] = 0;
    let mut file = tempfile::tempfile().unwrap();
    std::io::Write::write_all(&mut file, &corrupted).unwrap();
    let dropped = Arc::new(AtomicUsize::new(0));
    let content = Content {
        checksum_manifest: Some(expected),
        fallback_files: Vec::new(),
        file,
        size: original.len() as u64,
        hash: blake3::hash(&original).to_hex().to_string(),
        lease: Box::new(ReadLease(dropped.clone())),
        download_name: None,
    };
    let response = content_response(content, Method::GET, HeaderMap::new()).unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/octet-stream"
    );
    let mut stream = response.into_body().into_data_stream();
    assert_eq!(stream.next().await.unwrap().unwrap(), &original[..65536]);
    assert!(stream.next().await.unwrap().is_err());
    // No JSON envelope is appended to the partial binary representation.
    assert!(stream.next().await.is_none());
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn ranges_obey_boundaries_and_reject_non_decimal_syntax() {
    assert_eq!(parse_range("bytes=2-8", 5).unwrap(), (2, 4));
    assert_eq!(parse_range("bytes=-2", 5).unwrap(), (3, 4));
    assert_eq!(parse_range("bytes=3-", 5).unwrap(), (3, 4));
    assert_eq!(
        parse_range("bytes=-18446744073709551615", 5).unwrap(),
        (0, 4)
    );
    assert_eq!(
        parse_range("bytes=0-", u64::MAX).unwrap(),
        (0, u64::MAX - 1)
    );
    for invalid in [
        "bytes=5-",
        "bytes=0-1,3-4",
        "bytes=-0",
        "bytes=missing",
        "bytes=+1-2",
        "bytes=1-+2",
        "bytes=18446744073709551616-",
        "bytes= 1-2",
    ] {
        assert_eq!(
            parse_range(invalid, 5).unwrap_err().status,
            416,
            "{invalid}"
        );
    }
    assert_eq!(parse_range("bytes=0-0", 0).unwrap_err().status, 416);
}

#[tokio::test]
async fn malformed_duplicate_and_unsatisfiable_ranges_include_size_and_head_is_empty() {
    for method in [Method::GET, Method::HEAD] {
        for invalid in ["bytes=missing", "bytes=+1-2", "bytes=20-"] {
            let mut headers = HeaderMap::new();
            headers.insert(header::RANGE, HeaderValue::from_str(invalid).unwrap());
            let response =
                content_response(content(Arc::default()), method.clone(), headers).unwrap();
            assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
            assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes */10");
            assert_eq!(response.headers()[header::ACCEPT_RANGES], "bytes");
            let body = http_routing::body::to_bytes(response.into_body(), 8192)
                .await
                .unwrap();
            assert_eq!(body.is_empty(), method == Method::HEAD);
        }
        let mut headers = HeaderMap::new();
        headers.append(header::RANGE, HeaderValue::from_static("bytes=0-1"));
        headers.append(header::RANGE, HeaderValue::from_static("bytes=2-3"));
        let response = content_response(content(Arc::default()), method.clone(), headers).unwrap();
        assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes */10");
    }
}

#[tokio::test]
async fn download_lease_survives_until_body_consumption_or_cancellation() {
    for consume in [true, false] {
        let dropped = Arc::new(AtomicUsize::new(0));
        let response =
            content_response(content(dropped.clone()), Method::GET, HeaderMap::new()).unwrap();
        assert_eq!(dropped.load(Ordering::SeqCst), 0);
        if consume {
            assert_eq!(
                http_routing::body::to_bytes(response.into_body(), 8192)
                    .await
                    .unwrap(),
                "0123456789"
            );
        } else {
            drop(response);
        }
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn stale_if_range_returns_complete_content_and_head_retains_length() {
    let mut headers = HeaderMap::new();
    headers.insert(header::RANGE, HeaderValue::from_static("bytes=2-3"));
    headers.insert(header::IF_RANGE, HeaderValue::from_static("\"stale\""));
    let response = content_response(content(Arc::default()), Method::GET, headers).unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        http_routing::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap(),
        "0123456789"
    );
    let dropped = Arc::new(AtomicUsize::new(0));
    let mut headers = HeaderMap::new();
    headers.insert(header::RANGE, HeaderValue::from_static("bytes=2-3"));
    let response = content_response(content(dropped.clone()), Method::HEAD, headers).unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()[header::CONTENT_LENGTH], "2");
    assert!(
        http_routing::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

struct RetainedGenerationBackend;
impl crate::Backend for RetainedGenerationBackend {
    fn call(
        &self,
        action: crate::BackendAction,
        _identity: &Identity,
        _input: Value,
    ) -> Result<Value, ApiError> {
        match action {
            crate::BackendAction::UploadAuthorize => Ok(json!({"authorized":true})),
            _ => Err(ApiError::unsupported()),
        }
    }

    fn upload(
        &self,
        _identity: &Identity,
        _workspace: &str,
        _item: &str,
        _path: &str,
        _source: &crate::UploadSource,
    ) -> Result<Value, ApiError> {
        Ok(json!({"accepted":true}))
    }

    fn content(&self, _identity: &Identity, _version: &str) -> Result<Content, ApiError> {
        Err(ApiError::unsupported())
    }
}

#[test]
fn cancelled_generation_retention_keeps_item_claim_and_upload_slot_until_worker_finishes() {
    use service_stack::ServiceExt;
    use std::{
        fs,
        sync::{Condvar, Mutex},
        time::Duration,
    };

    struct Release(Arc<(Mutex<bool>, Condvar)>);
    impl Drop for Release {
        fn drop(&mut self) {
            uploads::RETAIN_GENERATION_HOOK.lock().unwrap().take();
            *self.0.0.lock().unwrap() = true;
            self.0.1.notify_all();
        }
    }
    let runtime = || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    };
    let request = |token: &str, item: &str, generation: u64, body: Body| {
        Request::builder()
            .method("PUT")
            .uri(format!("/api/workspaces/ws/files/{item}/content?path=file"))
            .header("authorization", format!("Bearer {token}"))
            .header("x-zxs-logical-size", "3")
            .header("x-zxs-upload-generation", generation.to_string())
            .header(
                "x-zxs-content-hash",
                blake3::hash(if generation == 1 { b"old" } else { b"new" })
                    .to_hex()
                    .to_string(),
            )
            .body(body)
            .unwrap()
    };
    let root = crate::tests::private_tempdir();
    let mut auth =
        crate::AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
    let token = auth.login("admin", "a-long-test-password").unwrap().token;
    let mut config = crate::ServerConfig::loopback(root.path());
    config.http_transfer_limits.slots = 2;
    config.http_spool_limits.recovery_reserve_bytes = 1;
    let spool = config.spool_dir.clone();
    let first_runtime = runtime();
    let (app, previous) = first_runtime.block_on(async {
        let app = crate::router(Arc::new(RetainedGenerationBackend), auth, config).unwrap();
        let first = app
            .clone()
            .oneshot(request(&token, "item", 1, Body::from("old")))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let previous = uploads::load(&spool, "ws", "item").unwrap().unwrap();
        let (started, started_rx) = tokio::sync::oneshot::channel();
        let started = Mutex::new(Some(started));
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let release = Release(gate.clone());
        *uploads::RETAIN_GENERATION_HOOK.lock().unwrap() = Some(uploads::RetainGenerationHook {
            root: spool.clone(),
            after_publish: Arc::new(move || {
                let Some(started) = started.lock().unwrap().take() else {
                    return;
                };
                started.send(()).unwrap();
                let mut ready = gate.0.lock().unwrap();
                while !*ready {
                    ready = gate.1.wait(ready).unwrap();
                }
            }),
        });
        let pending =
            tokio::spawn(
                app.clone()
                    .oneshot(request(&token, "item", 2, Body::from("new"))),
            );
        // The old generation's retained record is already installed, but its
        // blocking retention job has not completed its final sync or released owners.
        tokio::time::timeout(Duration::from_secs(5), started_rx)
            .await
            .unwrap()
            .unwrap();
        pending.abort();
        assert!(pending.await.unwrap_err().is_cancelled());
        let retry = app
            .clone()
            .oneshot(request(&token, "item", 2, Body::from("new")))
            .await
            .unwrap();
        assert_eq!(retry.status(), StatusCode::CONFLICT);
        let error: Value = json_codec::from_slice(
            &http_routing::body::to_bytes(retry.into_body(), 8192)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(error["error"]["code"], "FILE_WRITE_BUSY");

        let (reading, reading_rx) = tokio::sync::oneshot::channel();
        let mut reading = Some(reading);
        let body = Body::from_stream(futures::stream::poll_fn(move |_| {
            if let Some(reading) = reading.take() {
                reading.send(()).unwrap();
            }
            std::task::Poll::Pending::<Option<Result<bytes::Bytes, std::io::Error>>>
        }));
        let other = tokio::spawn(app.clone().oneshot(request(&token, "other", 1, body)));
        tokio::time::timeout(Duration::from_secs(5), reading_rx)
            .await
            .unwrap()
            .unwrap();
        let overflow = app
            .clone()
            .oneshot(request(&token, "third", 1, Body::from("old")))
            .await
            .unwrap();
        assert_eq!(overflow.status(), StatusCode::TOO_MANY_REQUESTS);
        other.abort();
        assert!(other.await.unwrap_err().is_cancelled());
        assert_eq!(
            uploads::load(&spool, "ws", "item")
                .unwrap()
                .unwrap()
                .generation,
            1
        );
        drop(release);
        (app, previous)
    });
    // Runtime shutdown joins the released blocking worker. Do not infer its
    // completion from elapsed time or a notification sent before owner destruction.
    drop(first_runtime);

    runtime().block_on(async {
        let retry = app
            .oneshot(request(&token, "item", 2, Body::from("new")))
            .await
            .unwrap();
        assert_eq!(retry.status(), StatusCode::OK);
    });
    let current = uploads::load(&spool, "ws", "item").unwrap().unwrap();
    assert_eq!(current.generation, 2);
    assert_eq!(current.state, "complete");
    assert_eq!(fs::read(spool.join(&current.data_file)).unwrap(), b"new");
    assert_eq!(fs::read(spool.join(&previous.data_file)).unwrap(), b"old");
    let retained = fs::read_dir(&spool)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("retained-")
        })
        .collect::<Vec<_>>();
    assert_eq!(retained.len(), 1);
    let record: uploads::UploadRecord =
        storage_contract::decode_strict(&fs::read(&retained[0]).unwrap()).unwrap();
    assert_eq!(
        json_codec::to_vec(&record).unwrap(),
        json_codec::to_vec(&previous).unwrap()
    );
}
