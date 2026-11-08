//! Explicit, isolated LD_PRELOAD fault fixture; normal CI does not assume a C
//! shim has been built. Run the named parent after compiling the adjacent C file.
use super::*;
use std::{
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const ROOT: &str = "ZXS_BP_PROGRESS_ROOT";
const SHIM: &str = "ZXS_BP_PROGRESS_SHIM";
const CHILD: &str = "backup_preparation::progress_io::progress_gate_child";

struct ChildOwner(Child);
impl Drop for ChildOwner {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn until_file(child: &mut Child, path: &Path, duration: Duration) -> bool {
    let deadline = Instant::now() + duration;
    loop {
        if path.is_file() {
            return true;
        }
        if child.try_wait().unwrap().is_some() || Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
#[ignore = "requires adjacent descriptor-scoped progress gate compiled and ZXS_BP_PROGRESS_SHIM"]
fn status_query_does_not_wait_for_a_blocked_system_progress_write() {
    let temp = tempfile::Builder::new()
        .permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700))
        .tempdir()
        .unwrap();
    let shim = std::env::var_os(SHIM).expect("explicit compiled progress gate is required");
    let mut child = ChildOwner(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                CHILD,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("LD_PRELOAD", shim)
            .env(ROOT, temp.path())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let ready = temp.path().join("progress-write-ready");
    assert!(
        until_file(&mut child.0, &ready, Duration::from_secs(10)),
        "no real SYSTEM progress write reached the gate"
    );
    let written: u64 = fs::read_to_string(ready).unwrap().trim().parse().unwrap();
    assert!(
        written > 0,
        "gate was not calibrated by a successful actual write"
    );
    let queried = until_file(
        &mut child.0,
        &temp.path().join("status-query-finished"),
        Duration::from_secs(2),
    );
    // Always release before asserting so an old blocking implementation also
    // exits normally and exposes the exact failed responsiveness assertion.
    fs::write(temp.path().join("progress-write-release"), b"release").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "child did not finish after gate release"
        );
        std::thread::sleep(Duration::from_millis(2));
    };
    assert!(
        queried,
        "status query was blocked behind SYSTEM progress I/O"
    );
    assert!(status.success(), "progress fixture failed: {status}");
}

#[test]
#[ignore = "isolated child only; the named parent launches this with a scoped shim"]
fn progress_gate_child() {
    let root = PathBuf::from(std::env::var_os(ROOT).expect("parent supplied private root"));
    let config = Config::development((0..3).map(|n| root.join(format!("disk-{n}")))).unwrap();
    let mut engine = Engine::initialize(&root.join("system"), config).unwrap();
    let repository = engine.create_disk_root("P01", "admin", op()).unwrap();
    save(
        &mut engine,
        &repository,
        "body",
        b"actual progress gate body",
    );
    let destination = target(&root, "target");
    let id = op();
    engine
        .start_backup_preparation(
            request(&engine, vec![destination.clone()], id),
            fs::File::open(&destination).unwrap(),
        )
        .unwrap();
    let ready = root.join("progress-write-ready");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready.is_file() {
        assert!(
            Instant::now() < deadline,
            "worker never entered progress write"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(!root.join("progress-write-release").exists());
    let status = engine.backup_preparation_status().unwrap();
    assert_eq!(status.operation_id, id);
    assert_eq!(status.phase, BackupPreparationPhase::Copying);
    fs::write(
        root.join("status-query-finished"),
        b"queried while actual write is blocked",
    )
    .unwrap();
    let completed = engine.wait_backup_preparation().unwrap();
    assert_eq!(completed.phase, BackupPreparationPhase::HistoryPrepared);
    offline_check_history_package(
        &destination
            .join("histories")
            .join(&repository.history_tree_id),
    )
    .unwrap();
}
