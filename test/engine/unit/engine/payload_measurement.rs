//! Identical baseline/candidate I/O fixtures; all sampling stays in test code.
use super::*;
use std::{fs, io::Write, time::Instant};

const BODY_NAME: &str = "payload-measurement-body.bin";

fn fixture() -> (tempfile::TempDir, Engine, Workspace, u64) {
    let bytes = match std::env::var("ZXS_PAYLOAD_MEASUREMENT_BYTES") {
        Ok(value) => value.parse().expect("measurement byte count is an integer"),
        Err(std::env::VarError::NotPresent) => 64 * 1024 * 1024,
        Err(error) => panic!("invalid measurement byte count: {error}"),
    };
    assert!(bytes > 0);
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join(BODY_NAME);
    let mut file = File::create_new(&source).unwrap();
    let block: Vec<_> = (0..65_536).map(|index| (index % 251) as u8).collect();
    let mut remaining = bytes;
    while remaining > 0 {
        let count = remaining.min(block.len() as u64) as usize;
        file.write_all(&block[..count]).unwrap();
        remaining -= count as u64;
    }
    file.sync_all().unwrap();
    drop(file);
    let config = Config::development([
        temp.path().join("primary"),
        temp.path().join("backup"),
        temp.path().join("tertiary"),
    ]);
    let mut engine = Engine::initialize(&temp.path().join("state"), config).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("payload-measurement").unwrap(),
            "P01",
            3,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_file(&workspace.workspace_id, BODY_NAME, &source)
        .unwrap();
    (temp, engine, workspace, bytes)
}

fn request(workspace: &Workspace) -> CommitRequest {
    CommitRequest {
        workspace_id: workspace.workspace_id.clone(),
        operation_id: storage_contract::OperationId::generate(),
        principal: "admin".into(),
        expected_head: workspace.base_head.clone(),
        fencing_token: workspace.fencing_token,
        message: "payload measurement".into(),
        request_digest: None,
    }
}

fn proc_value(contents: &str, key: &str) -> u64 {
    contents
        .lines()
        .find_map(|line| line.strip_prefix(key))
        .expect("Linux proc counter is present")
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

fn marker(text: &str) {
    std::io::stderr().lock().write_all(text.as_bytes()).unwrap();
}

fn measure(operation: &str, payload_bytes: u64, run: impl FnOnce()) {
    let memory_before = fs::read_to_string("/proc/self/status").unwrap();
    let io_before = fs::read_to_string("/proc/self/io").unwrap();
    marker(&format!("ZXS_PAYLOAD_MEASURE_BEGIN {operation}\n"));
    let started = Instant::now();
    run();
    let elapsed_ns = started.elapsed().as_nanos();
    marker(&format!("ZXS_PAYLOAD_MEASURE_END {operation}\n"));
    let io_after = fs::read_to_string("/proc/self/io").unwrap();
    let memory_after = fs::read_to_string("/proc/self/status").unwrap();
    let sample = json!({
        "operation": operation,
        "payload_bytes": payload_bytes,
        "elapsed_ns": elapsed_ns,
        "rchar": proc_value(&io_after, "rchar:") - proc_value(&io_before, "rchar:"),
        "wchar": proc_value(&io_after, "wchar:") - proc_value(&io_before, "wchar:"),
        "rss_before_kib": proc_value(&memory_before, "VmRSS:"),
        "rss_after_kib": proc_value(&memory_after, "VmRSS:"),
        "process_peak_before_kib": proc_value(&memory_before, "VmHWM:"),
        "process_peak_after_kib": proc_value(&memory_after, "VmHWM:"),
    });
    marker(&format!("ZXS_PAYLOAD_MEASURE_RESULT {sample}\n"));
}

#[test]
#[ignore = "same-fixture payload I/O benchmark; invoke through payload-syscalls.py"]
fn fresh_prepare_operation() {
    let (_temp, engine, workspace, bytes) = fixture();
    let plan = engine
        .plan_workspace_commit_inner(request(&workspace), None)
        .unwrap();
    let MutationPlan::Prepared { prepared, .. } = plan.mutation else {
        panic!("fresh operation has a payload plan");
    };
    assert_eq!(prepared.len(), 1);
    let payload = &prepared[0];
    assert_eq!(payload.placement.route_slot_ids.len(), 3);
    measure("fresh_prepare", bytes, || {
        engine.prepare_payload(payload).unwrap()
    });
    for slot in &payload.placement.route_slot_ids {
        let directory = version_dir(
            engine.disk(slot).unwrap(),
            &payload.version.history_tree_id,
            &payload.version.version_id,
        );
        verify_payload(&directory, &payload.version).unwrap();
    }
}

#[test]
#[ignore = "same-fixture payload I/O benchmark; invoke through payload-syscalls.py"]
fn export_operation() {
    let (temp, mut engine, workspace, bytes) = fixture();
    engine.commit_workspace(request(&workspace)).unwrap();
    let entry = &engine.list_tree(&workspace.branch_id).unwrap()[0];
    let destination = temp.path().join("export.bin");
    measure("export", bytes, || {
        engine
            .export_version(&entry.version_id, &destination)
            .unwrap();
    });
    let (size, hash, _) = full_copy(
        &destination,
        &temp.path().join("export-validation.bin"),
        engine.config.checksum_chunk_bytes,
        bytes,
    )
    .unwrap();
    assert_eq!(size, bytes);
    assert_eq!(hash, engine.state.versions[&entry.version_id].content_hash);
}

#[test]
#[ignore = "same-fixture existing-payload admission and preparation I/O benchmark"]
fn existing_publication_operation() {
    let (_temp, engine, workspace, bytes) = fixture();
    let plan = engine
        .plan_workspace_commit_inner(request(&workspace), None)
        .unwrap();
    let MutationPlan::Prepared {
        next,
        prepared,
        namespace,
        result,
    } = plan.mutation
    else {
        panic!("fresh operation has a payload plan");
    };
    assert_eq!(prepared.len(), 1);
    assert_eq!(prepared[0].placement.route_slot_ids.len(), 3);
    engine.prepare_payload(&prepared[0]).unwrap();
    measure("existing_publication", bytes, || {
        engine
            .prepare_publication(*next, prepared, namespace, result, MAX_METADATA)
            .unwrap();
    });
}
