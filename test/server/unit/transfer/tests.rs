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
            http::StatusCode::RANGE_NOT_SATISFIABLE,
            "{invalid}"
        );
    }
    assert_eq!(
        parse_range("bytes=0-0", 0).unwrap_err().status,
        http::StatusCode::RANGE_NOT_SATISFIABLE
    );
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
    fn operation(
        &self,
        _: &crate::auth::Identity,
        _: storage_contract::OperationId,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        unreachable!("test does not query operations")
    }
    fn commit_workspace(
        &self,
        _: &crate::auth::Identity,
        _: crate::CommitWorkspace,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        unreachable!("test does not commit workspaces")
    }

    fn snapshot_at(
        &self,
        _: &crate::auth::Identity,
        _: &str,
        _: &str,
    ) -> Result<std::sync::Arc<storage_contract::Commit>, crate::ApiError> {
        unreachable!("this test backend does not serve snapshots")
    }

    fn authorize_current(
        &self,
        _identity: &crate::auth::Identity,
        _branch: &str,
    ) -> Result<(), crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }

    fn read_directory(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadDirectory<'_>,
    ) -> Result<crate::DirectorySnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }

    fn authorize_workspace_selection(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
        _path: &name_policy::PortablePath,
    ) -> Result<(), crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }

    fn create_workspace(
        &self,
        _identity: &crate::auth::Identity,
        _command: crate::CreateWorkspace,
    ) -> Result<storage_contract::Workspace, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn abort_workspace(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
    ) -> Result<(), crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }

    fn workspace(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
    ) -> Result<storage_contract::Workspace, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn published_upload_status(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
        _item: &str,
    ) -> Result<Option<crate::PublishedUploadStatus>, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn seal_workspace(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
    ) -> Result<storage_contract::SealedWorkspace, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn commit_workspace_public(
        &self,
        _identity: &crate::auth::Identity,
        _command: storage_contract::PublicCommitEnvelope,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn version_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _version: &str,
    ) -> Result<crate::VersionSnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn authorize_version(
        &self,
        _identity: &crate::auth::Identity,
        _version: &str,
    ) -> Result<(), crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn repository_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<storage_contract::Repository, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn authorize_repository(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<(), crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn capabilities(&self) -> Result<crate::Capabilities, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn create_write_session(
        &self,
        _identity: &crate::auth::Identity,
        _branch: &str,
    ) -> Result<crate::WorkspaceWriteSession, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn stage_namespace(
        &self,
        _identity: &crate::auth::Identity,
        _command: crate::StageNamespace,
    ) -> Result<storage_contract::Workspace, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn authorize_upload(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
        _logical_size: Option<u64>,
    ) -> Result<(), crate::ApiError> {
        Ok(())
    }

    fn lookup_current(
        &self,
        _identity: &crate::auth::Identity,
        _branch: &str,
        _path: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }

    fn lookup_at(
        &self,
        _: &crate::auth::Identity,
        _: &str,
        _: &str,
        _: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, crate::ApiError> {
        unreachable!("this test backend does not serve snapshot lookups")
    }

    fn call(
        &self,
        _action: crate::BackendAction,
        _identity: &Identity,
        _input: Value,
    ) -> Result<Value, ApiError> {
        Err(ApiError::unsupported())
    }

    fn upload(
        &self,
        _identity: &Identity,
        _workspace: &str,
        _item: &str,
        _path: &name_policy::PortablePath,
        _source: &crate::UploadSource,
    ) -> Result<storage_contract::StagedFile, ApiError> {
        Ok(storage_contract::StagedFile {
            item_id: _item.into(),
            path: _path.clone(),
            source_path: "/private/upload-fixture".into(),
            size: _source.size,
            content_hash: _source.hash,
            dirty_generation: 1,
            base_version_id: None,
        })
    }
    fn upload_generation(
        &self,
        identity: &crate::auth::Identity,
        workspace: &str,
        item: &str,
        path: &name_policy::PortablePath,
        source: &crate::UploadSource,
        generation: u64,
    ) -> Result<storage_contract::StagedFile, crate::ApiError> {
        let mut staged = self.upload(identity, workspace, item, path, source)?;
        staged.dirty_generation = generation;
        Ok(staged)
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
            .oneshot(request(
                &token,
                "entry_00000000000000000000000000000001",
                1,
                Body::from("old"),
            ))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let previous = uploads::load(&spool, "ws", "entry_00000000000000000000000000000001")
            .unwrap()
            .unwrap();
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
        let pending = tokio::spawn(app.clone().oneshot(request(
            &token,
            "entry_00000000000000000000000000000001",
            2,
            Body::from("new"),
        )));
        // The old generation's retained record is already installed, but its
        // blocking retention job has not returned or released its owners.
        tokio::time::timeout(Duration::from_secs(5), started_rx)
            .await
            .unwrap()
            .unwrap();
        pending.abort();
        assert!(pending.await.unwrap_err().is_cancelled());
        let retry = app
            .clone()
            .oneshot(request(
                &token,
                "entry_00000000000000000000000000000001",
                2,
                Body::from("new"),
            ))
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
        let other = tokio::spawn(app.clone().oneshot(request(
            &token,
            "entry_00000000000000000000000000000002",
            1,
            body,
        )));
        tokio::time::timeout(Duration::from_secs(5), reading_rx)
            .await
            .unwrap()
            .unwrap();
        let overflow = app
            .clone()
            .oneshot(request(
                &token,
                "entry_00000000000000000000000000000003",
                1,
                Body::from("old"),
            ))
            .await
            .unwrap();
        assert_eq!(overflow.status(), StatusCode::TOO_MANY_REQUESTS);
        other.abort();
        assert!(other.await.unwrap_err().is_cancelled());
        assert_eq!(
            uploads::load(&spool, "ws", "entry_00000000000000000000000000000001")
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
            .oneshot(request(
                &token,
                "entry_00000000000000000000000000000001",
                2,
                Body::from("new"),
            ))
            .await
            .unwrap();
        assert_eq!(retry.status(), StatusCode::OK);
    });
    let current = uploads::load(&spool, "ws", "entry_00000000000000000000000000000001")
        .unwrap()
        .unwrap();
    assert_eq!(current.generation, 2);
    assert_eq!(current.state, crate::uploads::UploadState::Complete);
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

fn ready_body(frames: Vec<bytes::Bytes>) -> futures::stream::Peekable<BodyDataStream> {
    Body::from_stream(futures::stream::iter(
        frames.into_iter().map(Ok::<_, std::io::Error>),
    ))
    .into_data_stream()
    .peekable()
}

#[tokio::test]
async fn ready_upload_batches_bound_bytes_frames_and_checkpoint_overshoot() {
    let future = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    for (frame_size, frames, checkpoint, expected) in [
        (16384, 64, 1048576, 262144),
        (1, 128, 1048576, 64),
        (6, 10, 10, 12),
    ] {
        let mut stream = ready_body(vec![bytes::Bytes::from(vec![7; frame_size]); frames]);
        let first = stream.next().await.unwrap().unwrap();
        let batch = ready_upload_batch(&mut stream, first, 1048576, checkpoint, future);
        assert_eq!(batch.len(), expected);
        assert!(batch.iter().all(|byte| *byte == 7));
        assert_eq!(stream.next().await.unwrap().unwrap().len(), frame_size);
    }
    let first = bytes::Bytes::from(vec![3; 2 * 1024 * 1024]);
    let original = first.as_ptr();
    let mut stream = ready_body(vec![bytes::Bytes::from_static(b"tail")]);
    let batch = ready_upload_batch(&mut stream, first, 3 * 1024 * 1024, 1048576, future);
    assert_eq!(
        batch.as_ptr(),
        original,
        "large frames need no coalescing allocation"
    );
    assert_eq!(stream.next().await.unwrap().unwrap(), "tail");
}

#[tokio::test]
async fn ready_upload_batch_preserves_pending_errors_empty_and_overflow_frames() {
    let future = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut pending = Body::from_stream(futures::stream::pending::<
        Result<bytes::Bytes, std::io::Error>,
    >())
    .into_data_stream()
    .peekable();
    assert_eq!(
        ready_upload_batch(
            &mut pending,
            bytes::Bytes::from_static(b"abc"),
            10,
            1048576,
            future
        ),
        "abc"
    );
    for next in [
        Ok(bytes::Bytes::new()),
        Err(std::io::Error::other("interrupted")),
        Ok(bytes::Bytes::from_static(b"too long")),
    ] {
        let mut stream = Body::from_stream(futures::stream::iter([next]))
            .into_data_stream()
            .peekable();
        assert_eq!(
            ready_upload_batch(
                &mut stream,
                bytes::Bytes::from_static(b"abc"),
                4,
                1048576,
                future
            ),
            "abc"
        );
        assert!(
            stream.next().await.is_some(),
            "terminal/control frame remains for the HTTP boundary"
        );
    }
    let mut stream = ready_body(vec![bytes::Bytes::from_static(b"next")]);
    assert_eq!(
        ready_upload_batch(
            &mut stream,
            bytes::Bytes::from_static(b"abc"),
            10,
            1048576,
            tokio::time::Instant::now()
        ),
        "abc"
    );
    assert_eq!(stream.next().await.unwrap().unwrap(), "next");
}

#[tokio::test]
async fn fragmented_ready_upload_uses_one_batch_per_256k() {
    let future = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut stream = ready_body(vec![bytes::Bytes::from(vec![42; 16 * 1024]); 256]);
    let mut batches = 0;
    let mut length = 0;
    while let Some(first) = stream.next().await {
        let batch = ready_upload_batch(
            &mut stream,
            first.unwrap(),
            4 * 1024 * 1024 - length,
            1048576,
            future,
        );
        length += batch.len() as u64;
        batches += 1;
    }
    assert_eq!(length, 4 * 1024 * 1024);
    assert_eq!(
        batches, 16,
        "ready-stream operation count, not a real HTTP speedup claim"
    );
}

#[tokio::test]
async fn paced_upload_frames_are_not_held_for_coalescing() {
    let stream = futures::stream::unfold(0, |index| async move {
        if index == 4 {
            return None;
        }
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        Some((
            Ok::<_, std::io::Error>(bytes::Bytes::from(vec![42; 16384])),
            index + 1,
        ))
    });
    let mut stream = Body::from_stream(stream).into_data_stream().peekable();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut batches = 0;
    while let Some(first) = stream.next().await {
        let batch = ready_upload_batch(&mut stream, first.unwrap(), 65536, 1048576, deadline);
        assert_eq!(batch.len(), 16384);
        batches += 1;
    }
    assert_eq!(
        batches, 4,
        "paced input still needs one write task per frame"
    );
}

#[tokio::test]
async fn fragmented_upload_checkpoints_before_pending_and_verifies_retry_tail() {
    use service_stack::ServiceExt;
    let root = crate::tests::private_tempdir();
    let mut auth =
        crate::AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
    let token = auth.login("admin", "a-long-test-password").unwrap().token;
    let mut config = crate::ServerConfig::loopback(root.path());
    config.http_spool_limits.recovery_reserve_bytes = 1;
    let spool = config.spool_dir.clone();
    let app = crate::router(Arc::new(RetainedGenerationBackend), auth, config).unwrap();
    let bytes = vec![42; 2 * 1024 * 1024];
    let request = |body| {
        Request::builder()
            .method("PUT")
            .uri(
                "/api/workspaces/ws/files/entry_00000000000000000000000000000009/content?path=file",
            )
            .header("authorization", format!("Bearer {token}"))
            .header("x-zxs-logical-size", bytes.len().to_string())
            .header(
                "x-zxs-content-hash",
                blake3::hash(&bytes).to_hex().to_string(),
            )
            .body(body)
            .unwrap()
    };
    let frame = bytes::Bytes::from(vec![42; 16384]);
    let frames = futures::stream::iter((0..65).map({
        let frame = frame.clone();
        move |_| Ok::<_, std::io::Error>(frame.clone())
    }));
    let body = Body::from_stream(frames.chain(futures::stream::pending()));
    let upload = tokio::spawn(app.clone().oneshot(request(body)));
    let record = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Some(record) =
                uploads::load(&spool, "ws", "entry_00000000000000000000000000000009").unwrap()
                && std::fs::metadata(spool.join(&record.data_file))
                    .unwrap()
                    .len()
                    == 65 * 16384
            {
                break record;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(record.persisted_bytes, 1048576);
    assert_eq!(
        record.persisted_hash,
        blake3::hash(&bytes[..1048576]).to_hex().as_str()
    );
    upload.abort();
    assert!(upload.await.unwrap_err().is_cancelled());
    let retry = Body::from_stream(futures::stream::iter(
        (0..128).map(move |_| Ok::<_, std::io::Error>(frame.clone())),
    ));
    let response = app.clone().oneshot(request(retry)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .oneshot(request(Body::from(bytes.clone())))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let current = uploads::load(&spool, "ws", "entry_00000000000000000000000000000009")
        .unwrap()
        .unwrap();
    assert_eq!(current.generation, record.generation);
    assert_eq!(current.content_hash, record.content_hash);
    assert_eq!(current.data_file, record.data_file);
    assert_eq!(current.persisted_bytes, bytes.len() as u64);
    assert_eq!(std::fs::read(spool.join(current.data_file)).unwrap(), bytes);
}

// Explicit measurement: no timing assertion and no claim about network framing.
#[tokio::test]
#[ignore = "manual upload batching throughput measurement"]
async fn upload_batching_ready_and_paced_measurement() {
    use service_stack::ServiceExt;
    use std::time::{Duration, Instant};
    let root = crate::tests::private_tempdir();
    let mut auth =
        crate::AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
    let token = auth.login("admin", "a-long-test-password").unwrap().token;
    let mut config = crate::ServerConfig::loopback(root.path());
    config.http_spool_limits.recovery_reserve_bytes = 1;
    let spool = config.spool_dir.clone();
    let app = crate::router(Arc::new(RetainedGenerationBackend), auth, config).unwrap();
    let frame = bytes::Bytes::from(vec![42; 16384]);
    let bytes = vec![42; 4 * 1024 * 1024];
    let hash = blake3::hash(&bytes).to_hex().to_string();
    for paced in [false, true] {
        for run in 0..3 {
            let frame = frame.clone();
            let stream = futures::stream::unfold((0, frame), move |(index, frame)| async move {
                if index == 256 {
                    return None;
                }
                if paced {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
                Some((Ok::<_, std::io::Error>(frame.clone()), (index + 1, frame)))
            });
            let item = format!("measurement-{paced}-{run}");
            let request = Request::builder()
                .method("PUT")
                .uri(format!("/api/workspaces/ws/files/{item}/content?path=file"))
                .header("authorization", format!("Bearer {token}"))
                .header("x-zxs-logical-size", bytes.len().to_string())
                .header("x-zxs-content-hash", &hash)
                .body(Body::from_stream(stream))
                .unwrap();
            let start = Instant::now();
            let response = app.clone().oneshot(request).await.unwrap();
            let elapsed = start.elapsed();
            assert_eq!(response.status(), StatusCode::OK);
            let record = uploads::load(&spool, "ws", &item).unwrap().unwrap();
            assert_eq!(std::fs::read(spool.join(record.data_file)).unwrap(), bytes);
            println!(
                "upload batching: paced={paced}, run={run}, bytes={}, elapsed_us={}",
                bytes.len(),
                elapsed.as_micros()
            );
        }
    }
}

#[test]
fn public_backend_content_hash_is_validated_at_the_header_boundary() {
    use std::error::Error as _;
    let dropped = Arc::new(AtomicUsize::new(0));
    let mut content = content(dropped.clone());
    content.hash = "untrusted\r\nheader".into();
    let error = content_response(content, Method::HEAD, HeaderMap::new()).unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);
    assert!(
        error
            .source()
            .unwrap()
            .source()
            .unwrap()
            .is::<http::header::InvalidHeaderValue>()
    );
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn published_upload_wire_preserves_large_integers_empty_ranges_and_sealed_manifest() {
    for (size, state, manifest) in [
        (0, crate::PublishedUploadState::Complete, None),
        (
            u64::MAX,
            crate::PublishedUploadState::Sealed {
                manifest_hash: "manifest".into(),
            },
            Some("manifest"),
        ),
    ] {
        let hash = blake3::hash(b"").to_hex().to_string();
        let response = super::published_upload_response(
            "workspace",
            "item",
            crate::PublishedUploadStatus {
                generation: u64::MAX,
                logical_size: size,
                content_hash: hash.parse().unwrap(),
                state,
            },
        );
        assert_eq!(
            response,
            json!({"workspace_id":"workspace","item_id":"item","upload_generation":u64::MAX.to_string(),"logical_size":size.to_string(),"content_hash":hash,"state":if manifest.is_some(){"SEALED"}else{"COMPLETE"},"durable_ranges":if size>0{vec![json!({"offset":"0","length":size.to_string(),"content_hash":hash})]}else{vec![]},"sealed_manifest_hash":manifest})
        );
    }
}

#[tokio::test]
async fn typed_content_hash_header_rejects_noncanonical_values_before_staging() {
    use service_stack::ServiceExt;
    let root = crate::tests::private_tempdir();
    let mut auth =
        crate::AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
    let token = auth.login("admin", "a-long-test-password").unwrap().token;
    let mut config = crate::ServerConfig::loopback(root.path());
    config.http_spool_limits.recovery_reserve_bytes = 1;
    let spool = config.spool_dir.clone();
    let app = crate::router(Arc::new(RetainedGenerationBackend), auth, config).unwrap();
    let hash = blake3::hash(b"abc").to_hex().to_string();
    let uri = "/api/workspaces/ws_00000000000000000000000000000000/files/entry_00000000000000000000000000000000/content?path=file";
    let initial: Vec<_> = std::fs::read_dir(&spool)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    for values in [
        vec![],
        vec![String::new()],
        vec![hash.to_uppercase()],
        vec!["0".repeat(63)],
        vec!["g".repeat(64)],
        vec![hash.clone(), hash.clone()],
    ] {
        let mut request = Request::builder()
            .method("PUT")
            .uri(uri)
            .header("authorization", format!("Bearer {token}"))
            .header("x-zxs-logical-size", "3");
        for value in values {
            request = request.header("x-zxs-content-hash", value);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::from("abc")).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let current: Vec<_> = std::fs::read_dir(&spool)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(
            current, initial,
            "invalid hash must not create staged data or receipt"
        );
    }
    let request = Request::builder()
        .method("PUT")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("x-zxs-logical-size", "3")
        .header("x-zxs-content-hash", &hash)
        .body(Body::from("abc"))
        .unwrap();
    assert_eq!(app.oneshot(request).await.unwrap().status(), StatusCode::OK);
    let record = uploads::load(
        &spool,
        "ws_00000000000000000000000000000000",
        "entry_00000000000000000000000000000000",
    )
    .unwrap()
    .unwrap();
    assert_eq!(record.content_hash.unwrap().to_string(), hash);
    assert_eq!(std::fs::read(spool.join(record.data_file)).unwrap(), b"abc");
}
