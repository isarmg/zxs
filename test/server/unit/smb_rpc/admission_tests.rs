//! Shared capacity while authenticated connections remain idle.
use super::*;
use std::{net::Shutdown, os::unix::net::UnixStream, thread::JoinHandle, time::Duration};

struct Connection {
    wire: FrameStream,
    cancel: UnixStream,
    worker: JoinHandle<io::Result<()>>,
}

fn connect(shared: Arc<Shared>, username: &str, repository: &str) -> (Connection, Response) {
    let (client, server) = UnixStream::pair().unwrap();
    let cancel = server.try_clone().unwrap();
    let worker =
        std::thread::spawn(move || run_connection(FrameStream::new(server).unwrap(), shared));
    let mut wire = FrameStream::with_timeout(client, Duration::from_secs(2)).unwrap();
    wire.send(
        &Request::Hello {
            protocol: PROTOCOL.into(),
            capabilities: REQUIRED_CAPABILITIES
                .iter()
                .map(|value| (*value).into())
                .collect(),
            username: username.into(),
            repository_id: repository.into(),
        },
        None,
    )
    .unwrap();
    let (response, descriptor) = wire.receive::<Response>().unwrap();
    assert!(descriptor.is_none());
    (
        Connection {
            wire,
            cancel,
            worker,
        },
        response,
    )
}

fn disconnect(connection: Connection, cancel: bool) {
    if cancel {
        // Same cancellation mechanism as the listening service's SocketGuard.
        connection.cancel.shutdown(Shutdown::Both).unwrap();
    }
    drop(connection.wire);
    connection.worker.join().unwrap().unwrap();
}

#[test]
fn session_cap_is_shared_across_repositories_and_rejects_other_samba_names() {
    let (_temp, shared, _backend, _branch) = fixture();
    let repository = repository(&shared);
    let other_repository = shared
        .backend
        .call(
            crate::BackendAction::CreateRepository,
            &identity(&shared),
            json!({"name":"second-share","primary_slot_id":"P01","target_copies":3,"operation_id":new_id("op")}),
        )
        .unwrap()["repository_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut idle = Vec::new();
    for index in 0..MAX_AUTHENTICATED_SESSIONS {
        let target = if index % 2 == 0 {
            &repository
        } else {
            &other_repository
        };
        let (connection, response) = connect(shared.clone(), "alice", target);
        assert!(matches!(response, Response::Ok { .. }));
        idle.push(connection);
    }
    let (rejected, response) = connect(shared.clone(), "alice", &repository);
    assert!(
        matches!(response, Response::Error { code, .. } if code == storage_contract::ErrorCode::ResourceLimitExceeded)
    );
    disconnect(rejected, false);
    for username in ["alias-alice", "bob", "", "anonymous", "guest"] {
        let (rejected, response) = connect(shared.clone(), username, &repository);
        assert!(
            matches!(response, Response::Error { code, .. } if code == storage_contract::ErrorCode::Forbidden)
        );
        disconnect(rejected, false);
    }
    assert_eq!(
        *shared.authenticated_sessions.lock().unwrap(),
        MAX_AUTHENTICATED_SESSIONS
    );
    disconnect(idle.pop().unwrap(), false);
    let (replacement, response) = connect(shared.clone(), "alice", &other_repository);
    assert!(matches!(response, Response::Ok { .. }));
    idle.push(replacement);
    for connection in idle {
        disconnect(connection, true);
    }
    assert_eq!(*shared.authenticated_sessions.lock().unwrap(), 0);
}

#[test]
fn failed_repository_admission_does_not_leak_session_slots() {
    let (_temp, shared, _backend, _branch) = fixture();
    for _ in 0..=MAX_AUTHENTICATED_SESSIONS {
        let (rejected, response) = connect(shared.clone(), "alice", &new_id("repo"));
        assert!(
            matches!(response, Response::Error { code, .. } if code != storage_contract::ErrorCode::ResourceLimitExceeded)
        );
        disconnect(rejected, false);
        assert_eq!(*shared.authenticated_sessions.lock().unwrap(), 0);
    }
}

#[test]
fn configured_samba_account_binds_the_current_single_authority_and_fails_closed_during_recovery() {
    let (temp, mut shared, _backend, _branch) = fixture();
    let repository = repository(&shared);
    Arc::get_mut(&mut shared).unwrap().username = "samba-owner".into();
    let (wrong, response) = connect(shared.clone(), "alice", &repository);
    assert!(
        matches!(response, Response::Error { code, .. } if code == storage_contract::ErrorCode::Forbidden)
    );
    disconnect(wrong, false);
    let (mut live, response) = connect(shared.clone(), "samba-owner", &repository);
    assert!(matches!(response, Response::Ok { result } if result["principal"] == "alice"));

    *shared.auth.lock().unwrap() = AuthStore::unavailable(&temp.path().join("recovering"));
    live.wire
        .send(
            &Request::Stat {
                path: "report.txt".into(),
            },
            None,
        )
        .unwrap();
    assert!(
        matches!(live.wire.receive::<Response>().unwrap().0, Response::Error { code, .. } if code == storage_contract::ErrorCode::RecoveryRequired)
    );
    let (rejected, response) = connect(shared.clone(), "samba-owner", &repository);
    assert!(
        matches!(response, Response::Error { code, .. } if code == storage_contract::ErrorCode::RecoveryRequired)
    );
    disconnect(rejected, false);
    disconnect(live, false);
    assert_eq!(*shared.authenticated_sessions.lock().unwrap(), 0);

    *shared.auth.lock().unwrap() = AuthStore::initialize(
        &temp.path().join("restored-auth"),
        "restored-owner",
        "long-test-password",
    )
    .unwrap();
    let (restored, response) = connect(shared.clone(), "samba-owner", &repository);
    assert!(matches!(response, Response::Ok { result } if result["principal"] == "restored-owner"));
    disconnect(restored, false);
}
