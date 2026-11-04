//! Real engine gates through the HTTP content adapter and response body owner.
use super::*;
use crate::{Backend, engine_backend::EngineBackend};
use std::os::unix::fs::MetadataExt;
use storage_engine::{Config, DiskQuiesceFault, Engine, OperationId};

struct ManagedFixture {
    _root: tempfile::TempDir,
    backend: EngineBackend,
    identity: Identity,
    version: String,
    slots: Vec<String>,
    payload: Vec<u8>,
}

impl ManagedFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut config = Config::development([
            root.path().join("primary"),
            root.path().join("backup-one"),
            root.path().join("backup-two"),
        ])
        .unwrap();
        config.checksum_chunk_bytes = 65_536;
        let mut engine = Engine::initialize(&root.path().join("system"), config).unwrap();
        let repository = engine
            .create_repository(
                &storage_contract::RepositoryName::parse("HTTP disk cancellation").unwrap(),
                "P01",
                3,
                "owner",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        let workspace = engine
            .create_workspace(&repository.main_branch_id, "owner")
            .unwrap();
        let payload = vec![42; 65_536 * 3];
        engine
            .stage_bytes(&workspace.workspace_id, "body", &payload)
            .unwrap();
        engine
            .commit_workspace(storage_engine::CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: OperationId::generate().unwrap(),
                principal: "owner".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "HTTP disk cancellation fixture".into(),
                request_digest: None,
            })
            .unwrap();
        let version = engine.list_tree(&repository.main_branch_id).unwrap()[0]
            .version_id
            .clone();
        let slots = engine.state().placements[&version].route_slot_ids.clone();
        let auth = crate::auth::AuthStore::initialize(
            &root.path().join("auth"),
            "owner",
            "http-disk-cancellation-test-password",
        )
        .unwrap();
        let identity = auth.sole_identity().unwrap();
        engine
            .persist_auth_state(
                auth.snapshot().unwrap(),
                "system_auth",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        Self {
            _root: root,
            backend: EngineBackend::new(engine),
            identity,
            version,
            slots,
            payload,
        }
    }

    fn content(&self) -> Content {
        self.backend
            .range_content(
                &self.identity,
                crate::ReadContent {
                    version: &self.version,
                    context: None,
                },
            )
            .unwrap()
    }

    fn quiesce(&self, slot: &str) -> DiskQuiesceFault {
        let mut fault = self
            .backend
            .test_with_engine_locked(|engine| engine.inject_disk_quiesce(slot).unwrap());
        assert!(
            !fault.is_drained().unwrap(),
            "the control responsibility is still held"
        );
        fault.release_control();
        fault
    }
}

fn identities(content: &Content) -> Vec<(u64, u64)> {
    std::iter::once(&content.file)
        .chain(&content.fallback_files)
        .map(|file| {
            let metadata = file.metadata().unwrap();
            (metadata.dev(), metadata.ino())
        })
        .collect()
}

fn matching_descriptors(identity: (u64, u64)) -> usize {
    std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter(|entry| {
            let entry = entry.as_ref().unwrap();
            match std::fs::metadata(entry.path()) {
                Ok(metadata) => (metadata.dev(), metadata.ino()) == identity,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => panic!("inspect test descriptor: {error}"),
            }
        })
        .count()
}

#[tokio::test]
async fn an_unpolled_http_body_releases_real_disk_descriptors_on_quiesce() {
    let fixture = ManagedFixture::new();
    let content = fixture.content();
    let files = identities(&content);
    assert!(
        files
            .iter()
            .all(|identity| matching_descriptors(*identity) > 0)
    );
    let response = content_response(content, Method::GET, HeaderMap::new()).unwrap();
    // Keep the actual response body alive without polling or expiring its idle
    // timer. Quiescing must close its disk resources without consumer progress.
    for slot in &fixture.slots {
        let fault = fixture.quiesce(slot);
        assert_eq!(fault.ordinary_count().unwrap(), 0);
        assert!(fault.is_drained().unwrap());
    }
    assert!(
        files
            .iter()
            .all(|identity| matching_descriptors(*identity) == 0)
    );
    let mut stream = response.into_body().into_data_stream();
    assert!(stream.next().await.unwrap().is_err());
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn http_backpressure_keeps_no_cancelled_payload_open_between_chunks() {
    let fixture = ManagedFixture::new();
    let content = fixture.content();
    let files = identities(&content);
    let mut stream = content_response(content, Method::GET, HeaderMap::new())
        .unwrap()
        .into_body()
        .into_data_stream();
    assert_eq!(
        stream.next().await.unwrap().unwrap().as_ref(),
        &fixture.payload[..65_536]
    );
    for slot in &fixture.slots {
        assert!(fixture.quiesce(slot).is_drained().unwrap());
    }
    assert!(
        files
            .iter()
            .all(|identity| matching_descriptors(*identity) == 0)
    );
    assert!(stream.next().await.unwrap().is_err());
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn an_open_http_stream_moves_to_an_independently_admitted_fallback() {
    let fixture = ManagedFixture::new();
    let content = fixture.content();
    let files = identities(&content);
    let mut stream = content_response(content, Method::GET, HeaderMap::new())
        .unwrap()
        .into_body()
        .into_data_stream();
    assert_eq!(stream.next().await.unwrap().unwrap().len(), 65_536);
    assert!(fixture.quiesce(&fixture.slots[0]).is_drained().unwrap());
    assert_eq!(matching_descriptors(files[0]), 0);
    assert!(
        files[1..]
            .iter()
            .all(|identity| matching_descriptors(*identity) > 0)
    );
    assert_eq!(
        stream.next().await.unwrap().unwrap().as_ref(),
        &fixture.payload[65_536..131_072]
    );
    for slot in &fixture.slots[1..] {
        assert!(fixture.quiesce(slot).is_drained().unwrap());
    }
    assert!(
        files
            .iter()
            .all(|identity| matching_descriptors(*identity) == 0)
    );
    assert!(stream.next().await.unwrap().is_err());
}

#[test]
fn checksum_backend_reads_skip_cancelled_members_and_reject_all_cancelled() {
    let fixture = ManagedFixture::new();
    let checks = fixture
        .backend
        .checksums(&fixture.identity, &fixture.version)
        .unwrap();
    assert!(fixture.quiesce(&fixture.slots[0]).is_drained().unwrap());
    assert_eq!(
        fixture
            .backend
            .checksums(&fixture.identity, &fixture.version)
            .unwrap(),
        checks
    );
    for slot in &fixture.slots[1..] {
        assert!(fixture.quiesce(slot).is_drained().unwrap());
    }
    let error = fixture
        .backend
        .checksums(&fixture.identity, &fixture.version)
        .unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::ContentUnavailable);
}
