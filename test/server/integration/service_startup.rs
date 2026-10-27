use std::process::{Command, Output};
fn daemon(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_zxs"))
        .args(args)
        .env("TOKIO_WORKER_THREADS", "0")
        .env_remove("RUST_LOG")
        .output()
        .unwrap()
}
#[test]
fn direct_daemon_help_and_version_do_not_start_workers() {
    for args in [&["--help"][..], &["--version"]] {
        let output = daemon(args);
        assert!(output.status.success(), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
    }
    let help = String::from_utf8(daemon(&["--help"]).stdout).unwrap();
    assert!(help.contains("ZXS storage and Web management daemon"));
    assert!(!help.contains("Commands:"));
    assert!(help.contains("--management-root"));
}
#[test]
fn daemon_does_not_dispatch_management_or_rescue_subcommands() {
    for name in [
        "serve",
        "init",
        "account",
        "restore-system",
        "offline-export",
        "put",
        "get",
    ] {
        let output = daemon(&[name]);
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(!String::from_utf8_lossy(&output.stderr).contains("TOKIO_WORKER_THREADS"));
    }
}
#[test]
fn no_subcommand_is_required_to_start_daemon_runtime() {
    let output = daemon(&[]);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("TOKIO_WORKER_THREADS"),
        "{output:?}"
    );
}

#[test]
fn malformed_setup_identifiers_fail_before_creating_state() {
    for (flag, id) in [("--setup-disk", "P.01"), ("--setup-config", "layout.prod")] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("pool");
        let management = directory.path().join("management");
        let output = Command::new(env!("CARGO_BIN_EXE_zxs"))
            .arg("--root")
            .arg(&root)
            .arg("--management-root")
            .arg(&management)
            .args([
                flag,
                &format!("{id}={}", directory.path().join("missing").display()),
            ])
            .env("TOKIO_WORKER_THREADS", "1")
            .env_remove("RUST_LOG")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains(&format!(
            "{flag} ID must contain 1–64 ASCII letters, digits, '_' or '-'"
        )));
        assert!(!root.exists());
        assert!(!management.exists());
    }
}

struct ChildGuard(std::process::Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}
fn response(port: u16, path: &str) -> std::io::Result<String> {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port))?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    )?;
    let mut value = String::new();
    stream.read_to_string(&mut value)?;
    Ok(value)
}
#[test]
fn daemon_serves_web_and_liveness_with_missing_or_corrupt_pool() {
    use std::os::unix::fs::DirBuilderExt;
    for (corrupt, auxiliary_failure) in [(false, false), (true, false), (true, true)] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("pool");
        let management = directory.path().join("management");
        let web = directory.path().join("web");
        let runtime = directory.path().join("runtime");
        let socket = runtime.join("smb.sock");
        for path in [&root, &management, &web] {
            std::fs::DirBuilder::new().mode(0o700).create(path).unwrap();
        }
        let preset = directory.path().join("preset.json");
        if corrupt {
            std::fs::write(root.join("config.json"), "not a valid pool configuration").unwrap();
            std::fs::write(&preset, "not a valid setup preset").unwrap();
        }
        if auxiliary_failure {
            std::fs::write(management.join("api-spool"), "keep this evidence").unwrap();
            std::fs::write(management.join("smb-spool"), "keep SMB evidence").unwrap();
            // Empty assets must not advertise a working Web UI.
        } else {
            std::fs::write(web.join("index.html"), "<h1>management-available</h1>").unwrap();
        }
        let reservation = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = reservation.local_addr().unwrap().port();
        drop(reservation);
        let child = Command::new(env!("CARGO_BIN_EXE_zxs"))
            .arg("--root")
            .arg(&root)
            .arg("--management-root")
            .arg(&management)
            .arg("--web-dir")
            .arg(&web)
            .arg("--smb-socket")
            .arg(&socket)
            .args([
                "--smb-peer-uid",
                &smb_protocol::protocol::effective_uid().to_string(),
                "--smb-user",
                "samba-owner",
            ])
            .args(["--setup-config", &format!("broken={}", preset.display())])
            .args([
                "--setup-disk",
                &format!(
                    "missing={}",
                    directory.path().join("missing-disk").display()
                ),
            ])
            .args(["--listen", &format!("127.0.0.1:{port}")])
            .env("TOKIO_WORKER_THREADS", "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut child = ChildGuard(child);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "daemon exited on unavailable storage"
            );
            if response(port, "/healthz").is_ok_and(|reply| reply.starts_with("HTTP/1.1 200")) {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "HTTP did not start");
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        // The optional bridge prepares its own paths asynchronously. Its spool
        // follows management_root, never the disposable socket parent.
        while !runtime.exists() || (!auxiliary_failure && !socket.exists()) {
            assert!(child.0.try_wait().unwrap().is_none());
            assert!(
                std::time::Instant::now() < deadline,
                "SMB did not initialize"
            );
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(!runtime.join("smb-spool").exists());
        if auxiliary_failure {
            assert!(!socket.exists());
            assert_eq!(
                std::fs::read(management.join("smb-spool")).unwrap(),
                b"keep SMB evidence"
            );
        } else {
            assert!(management.join("smb-spool/empty-directory").is_dir());
        }
        let page = response(port, "/").unwrap();
        if auxiliary_failure {
            assert!(page.starts_with("HTTP/1.1 503"), "{page}");
            assert!(page.contains("WEB_ASSETS_UNAVAILABLE"));
            // Health/Web admission no longer waits for optional spool inventory.
            let status = loop {
                let status = response(port, "/api/management/status").unwrap();
                assert!(status.starts_with("HTTP/1.1 200"), "{status}");
                if !status.contains("\"http_uploads\":\"pending\"") {
                    break status;
                }
                assert!(child.0.try_wait().unwrap().is_none());
                assert!(
                    std::time::Instant::now() < deadline,
                    "HTTP spool inventory did not finish"
                );
                std::thread::sleep(std::time::Duration::from_millis(25));
            };
            assert!(status.contains("\"http_uploads\":\"unavailable\""));
            assert!(status.contains("\"web_assets\":\"unavailable\""));
            assert_eq!(
                std::fs::read(management.join("api-spool")).unwrap(),
                b"keep this evidence"
            );
        } else {
            assert!(page.starts_with("HTTP/1.1 200"), "{page}");
            assert!(page.contains("management-available"));
        }
        let readiness = response(port, "/readyz").unwrap();
        assert!(readiness.starts_with("HTTP/1.1 503"), "{readiness}");
        assert!(
            !root.join("auth.json").exists(),
            "must not initialize on storage failure"
        );
    }
}

#[test]
fn default_state_directory_remains_private_for_later_web_setup() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let reservation = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = reservation.local_addr().unwrap().port();
    drop(reservation);
    let child = Command::new(env!("CARGO_BIN_EXE_zxs"))
        .current_dir(directory.path())
        .args(["--listen", &format!("127.0.0.1:{port}")])
        .env("TOKIO_WORKER_THREADS", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut child = ChildGuard(child);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        assert!(child.0.try_wait().unwrap().is_none());
        if response(port, "/healthz").is_ok_and(|reply| reply.starts_with("HTTP/1.1 200")) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    for path in ["zxs-data", "zxs-data/management"] {
        let permissions = std::fs::metadata(directory.path().join(path))
            .unwrap()
            .permissions();
        assert_eq!(permissions.mode() & 0o777, 0o700, "{path}");
    }
    assert!(!directory.path().join("zxs-data/config.json").exists());
}

#[test]
fn invalid_retained_workspace_flags_do_not_create_state() {
    for flags in [
        vec!["--max-retained-workspaces", "2"],
        vec![
            "--max-retained-workspaces",
            "0",
            "--max-retained-workspaces-per-principal",
            "1",
            "--max-retained-workspace-metadata-bytes",
            "1",
            "--max-retained-workspace-metadata-bytes-per-principal",
            "1",
        ],
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("pool");
        let management = directory.path().join("management");
        let output = Command::new(env!("CARGO_BIN_EXE_zxs"))
            .arg("--root")
            .arg(&root)
            .arg("--management-root")
            .arg(&management)
            .env("TOKIO_WORKER_THREADS", "0")
            .args(flags)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("TOKIO_WORKER_THREADS"));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}

// Native-host regression: this exercises daemon processes and OS signals, not
// systemd, physical disks, host reboots, or abrupt loss of power.
#[test]
fn repeated_normal_daemon_stops_release_same_address_and_socket() {
    use rustix::process::{Pid, Signal, kill_process};
    use std::{
        os::unix::net::UnixStream,
        time::{Duration, Instant},
    };
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("pool");
    let management = directory.path().join("management");
    let socket = directory.path().join("runtime/smb.sock");
    drop(
        storage_engine::Engine::initialize(
            &root,
            storage_contract::Config::development([directory.path().join("disk")]),
        )
        .unwrap(),
    );
    let configuration = std::fs::read(root.join("config.json")).unwrap();
    let reservation = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = reservation.local_addr().unwrap();
    drop(reservation);
    // SIGINT is the shipped systemd unit's normal stop signal. Also retain
    // SIGTERM support for supervisors using their default stop signal.
    for (cycle, signal) in [Signal::INT, Signal::INT, Signal::INT, Signal::TERM]
        .into_iter()
        .enumerate()
    {
        let child = Command::new(env!("CARGO_BIN_EXE_zxs"))
            .arg("--root")
            .arg(&root)
            .arg("--management-root")
            .arg(&management)
            .arg("--smb-socket")
            .arg(&socket)
            .args([
                "--smb-peer-uid",
                &smb_protocol::protocol::effective_uid().to_string(),
            ])
            .args([
                "--smb-user",
                "cycle-owner",
                "--listen",
                &address.to_string(),
            ])
            .env("TOKIO_WORKER_THREADS", "1")
            .env_remove("RUST_LOG")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        // This guard is failure cleanup only. Passing requires a successful,
        // already-reaped normal exit below, never Child::kill().
        let mut child = ChildGuard(child);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "cycle {cycle}: early exit"
            );
            if response(address.port(), "/healthz")
                .is_ok_and(|reply| reply.starts_with("HTTP/1.1 200"))
                && socket.exists()
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "cycle {cycle}: startup timed out"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let status = response(address.port(), "/api/management/status").unwrap();
        assert!(
            status.starts_with("HTTP/1.1 200"),
            "cycle {cycle}: {status}"
        );
        // ShutdownSignal is installed before startup I/O, so health readiness
        // cannot precede registration of the normal-stop signal handlers.
        let pid = Pid::from_raw(child.0.id().try_into().unwrap()).unwrap();
        kill_process(pid, signal).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                assert!(
                    status.success(),
                    "cycle {cycle}: normal stop returned {status}"
                );
                break;
            }
            assert!(
                Instant::now() < deadline,
                "cycle {cycle}: normal stop timed out"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!socket.exists(), "cycle {cycle}: stale SMB socket");
        assert!(UnixStream::connect(&socket).is_err());
        assert!(std::net::TcpStream::connect_timeout(&address, Duration::from_secs(1)).is_err());
        assert_eq!(
            std::fs::read(root.join("config.json")).unwrap(),
            configuration,
            "cycle {cycle}: restart must preserve installation configuration"
        );
        assert!(
            !root.join("setup-intent.json").exists(),
            "cycle {cycle}: must not reinitialize"
        );
    }
}

#[test]
fn incoherent_scrub_host_policy_fails_before_creating_management_state() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("system");
    let output = Command::new(env!("CARGO_BIN_EXE_zxs"))
        .arg("--root")
        .arg(&root)
        .args([
            "--scrub-interval-seconds",
            "2",
            "--scrub-max-age-seconds",
            "1",
        ])
        .env_remove("TOKIO_WORKER_THREADS")
        .env_remove("RUST_LOG")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    // The daemon reports its typed outer error with Debug, preserving the
    // invalid values rather than this error's Display description.
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert!(diagnostic.contains("InvalidScrubPolicy"), "{output:?}");
    assert!(diagnostic.contains("interval_seconds: 2"), "{output:?}");
    assert!(diagnostic.contains("max_age_seconds: 1"), "{output:?}");
    assert!(!root.exists());
}
