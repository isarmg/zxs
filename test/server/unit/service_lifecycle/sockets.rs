use super::*;
use crate::{ApiError, Backend, BackendAction, Content, auth::Identity};
use http_routing::{Router, routing::get};
use json_codec::Value;
use smb_protocol::protocol::{FrameStream, PROTOCOL, REQUIRED_CAPABILITIES, Request, Response};
use std::{
    io::Read,
    os::unix::{fs::PermissionsExt, net::UnixStream},
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Semaphore,
};

struct BackendProbe;
impl Backend for BackendProbe {
    fn operation(
        &self,
        _: &crate::auth::Identity,
        _: storage_contract::OperationId,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        unreachable!("this test backend does not query operations")
    }
    fn commit_workspace(
        &self,
        _: &crate::auth::Identity,
        _: crate::CommitWorkspace,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        unreachable!("this test backend does not commit workspaces")
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
        id: &str,
    ) -> Result<storage_contract::Repository, crate::ApiError> {
        Ok(storage_contract::Repository {
            repository_id: id.into(),
            name: "test".into(),
            history_tree_id: "test-history".into(),
            main_branch_id: "test-branch".into(),
            primary_slot_id: "P01".into(),
            target_copies: 1,
            name_policy: name_policy::NAME_POLICY.into(),
            unicode_version: name_policy::UNICODE_VERSION.into(),
            smb_enabled: true,
            smb_binding_generation: 1,
            file_manager_root: false,
        })
    }
    fn authorize_repository(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<(), crate::ApiError> {
        Ok(())
    }
    fn capabilities(&self) -> Result<crate::Capabilities, crate::ApiError> {
        Ok(crate::Capabilities::for_engine(4096))
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
        Err(crate::ApiError::unsupported())
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

    fn call(&self, _action: BackendAction, _: &Identity, _: Value) -> Result<Value, ApiError> {
        Err(ApiError::unsupported())
    }
    fn upload(
        &self,
        _: &Identity,
        _: &str,
        _: &str,
        _: &name_policy::PortablePath,
        _: &crate::UploadSource,
    ) -> Result<storage_contract::StagedFile, ApiError> {
        Err(ApiError::unsupported())
    }
    fn upload_generation(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
        _item: &str,
        _path: &name_policy::PortablePath,
        _source: &crate::UploadSource,
        _generation: u64,
    ) -> Result<storage_contract::StagedFile, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }

    fn content(&self, _: &Identity, _: &str) -> Result<Content, ApiError> {
        Err(ApiError::unsupported())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_closes_idle_smb_and_http_listener_while_http_request_is_stalled() {
    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = root.path().join("smb.sock");
    let bridge_socket = socket.clone();
    let bridge_spool = root.path().join("management/smb-spool");
    let auth = Arc::new(Mutex::new(
        crate::auth::AuthStore::initialize(
            &root.path().join("auth"),
            "alice",
            "long-test-password",
        )
        .unwrap(),
    ));
    let (stop, shutdown) = oneshot::channel();
    let stalled = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let app = Router::new().route(
        "/stall",
        get({
            let stalled = stalled.clone();
            let release = release.clone();
            move || {
                let stalled = stalled.clone();
                let release = release.clone();
                async move {
                    stalled.add_permits(1);
                    release.acquire().await.unwrap().forget();
                    "finished"
                }
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(run(
        |stopped| async move {
            http_routing::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = stopped.await;
                })
                .await
        },
        async move {
            crate::smb_rpc::serve(
                crate::SmbConfig {
                    socket: bridge_socket,
                    spool_dir: bridge_spool,
                    allowed_uid: smb_protocol::protocol::effective_uid(),
                    username: "alice".into(),
                    spool_limits: crate::SpoolLimits::default(),
                },
                auth,
                Arc::new(BackendProbe),
                4096,
            )
            .await
            .unwrap();
        },
        async move {
            let _ = shutdown.await;
        },
    ));
    let mut http = tokio::net::TcpStream::connect(address).await.unwrap();
    http.write_all(b"GET /stall HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    tokio::time::timeout(DEADLINE, stalled.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    tokio::time::timeout(DEADLINE, async {
        while !socket.exists() {
            assert!(!server.is_finished(), "bridge failed before binding");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let mut idle = tokio::task::spawn_blocking({
        let socket = socket.clone();
        move || {
            let mut wire =
                FrameStream::with_timeout(UnixStream::connect(socket).unwrap(), DEADLINE).unwrap();
            wire.send(
                &Request::<String>::Hello {
                    protocol: PROTOCOL.into(),
                    capabilities: REQUIRED_CAPABILITIES
                        .iter()
                        .map(|value| (*value).into())
                        .collect(),
                    username: "alice".into(),
                    repository_id: "test-repository".into(),
                },
                None,
            )
            .unwrap();
            let (reply, _) = wire.receive::<Response>().unwrap();
            assert!(matches!(reply, Response::Ok { .. }));
            wire.0
        }
    })
    .await
    .unwrap();
    stop.send(()).unwrap();
    tokio::task::spawn_blocking(move || {
        let mut byte = [0];
        assert_eq!(
            idle.read(&mut byte).unwrap(),
            0,
            "idle SMB session must close"
        );
    })
    .await
    .unwrap();
    tokio::time::timeout(DEADLINE, async {
        while socket.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("SMB socket must be removed before HTTP drain");
    assert!(UnixStream::connect(&socket).is_err());
    tokio::time::timeout(DEADLINE, async {
        while tokio::net::TcpStream::connect(address).await.is_ok() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        !server.is_finished(),
        "stalled admitted HTTP request must still drain"
    );
    release.add_permits(1);
    let mut response = String::new();
    tokio::time::timeout(DEADLINE, http.read_to_string(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.ends_with("finished"), "{response}");
    tokio::time::timeout(DEADLINE, server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
