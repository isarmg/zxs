//! Fixed 500-file local RPC fixture; 22/220 seed commits selected explicitly.
//! One branch; timed event output fixed at22; full220-event correctness outside timing.
use std::{io::Read, path::Path, time::Instant};
use storage_contract::{Entry, Event, OperationId};
use storage_engine::{Engine, NamespaceTree};
fn commits() -> usize {
    let count: usize = std::env::var("ZXS_RPC_COMMITS").unwrap().parse().unwrap();
    assert!(matches!(count, 22 | 220));
    count
}
fn edits() -> usize { commits() - 2 }
const TIMED_EVENT_LIMIT: usize = 22;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn counter(text: &str, name: &str) -> Result<u64> {
    let raw = text
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| *key == name)
        .and_then(|(_, value)| value.split_whitespace().next())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("missing /proc counter {name}"),
            )
        })?;
    Ok(raw.parse()?)
}
fn counters() -> Result<[u64; 4]> {
    let text = std::fs::read_to_string("/proc/self/io")?;
    Ok([
        counter(&text, "rchar")?,
        counter(&text, "wchar")?,
        counter(&text, "read_bytes")?,
        counter(&text, "write_bytes")?,
    ])
}
fn path_for(index: usize) -> String {
    format!("dir-{:03}/file-{index:06}.txt", index % 97)
}
fn save(
    engine: &mut Engine,
    branch: &str,
    files: &[(String, Vec<u8>)],
    message: &str,
) -> Result<OperationId> {
    let workspace = engine.create_workspace(branch, "bench")?;
    for (path, bytes) in files {
        engine.stage_bytes(&workspace.workspace_id, path, bytes)?;
    }
    let operation_id = OperationId::generate();
    engine.commit_workspace(storage_contract::CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id,
        principal: "bench".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: message.into(),
        request_digest: None,
    })?;
    Ok(operation_id)
}
fn report(
    phase: &str,
    files: usize,
    started: Instant,
    before: [u64; 4],
    extra: json_codec::Value,
) -> Result<()> {
    let elapsed = started.elapsed().as_nanos();
    let after = counters()?;
    let status = std::fs::read_to_string("/proc/self/status")?;
    println!(
        "{}",
        json_codec::json!({
            "phase":phase,"files":files,"wall_ns":elapsed,
            "rchar":after[0]-before[0],"wchar":after[1]-before[1],
            "read_bytes":after[2]-before[2],"write_bytes":after[3]-before[3],
            "vmhwm_kb":counter(&status,"VmHWM")?,"vmrss_kb":counter(&status,"VmRSS")?,"extra":extra
        })
    );
    Ok(())
}
// The four functions below are the only representation-specific adapters.
fn lookup<'a>(tree: &NamespaceTree<'a>, path: &str) -> Result<&'a Entry> {
    Ok(tree.get(path).expect("invariant: generated file exists"))
}
fn directory_page(tree: &NamespaceTree<'_>) -> Result<Vec<Entry>> {
    Ok(tree
        .entries()
        .iter()
        .filter(|entry| {
            entry
                .path()
                .strip_prefix("dir-001/")
                .is_some_and(|tail| !tail.contains('/'))
        })
        .take(200)
        .cloned()
        .collect())
}
fn event_page(engine: &Engine) -> Result<Vec<Event>> {
    Ok(engine.events_after(0).take(TIMED_EVENT_LIMIT).cloned().collect())
}

fn all_events(engine: &Engine) -> Result<Vec<Event>> {
    Ok(engine.events_after(0).cloned().collect())
}

fn all_entries(tree: &NamespaceTree<'_>) -> Result<Vec<Entry>> {
    Ok(tree.entries().to_vec())
}

fn read_once(engine: &Engine, branch: &str, files: usize, label: &str) -> Result<()> {
    let io = counters()?;
    let start = Instant::now();
    let tree = engine.namespace_tree(branch)?;
    for round in 0..1000 {
        let path = path_for((round * 101) % files);
        let entry = lookup(&tree, &path)?;
        assert_eq!(
            entry.path(),
            path,
            "lookup must return the requested identity"
        );
    }
    let page = directory_page(&tree)?;
    let expected: Vec<_> = (0..files)
        .filter(|index| index % 97 == 1)
        .map(path_for)
        .take(200)
        .collect();
    assert_eq!(
        page.iter().map(|entry| entry.path()).collect::<Vec<_>>(),
        expected
    );
    let events = event_page(engine)?;
    assert_eq!(events.len(), TIMED_EVENT_LIMIT);
    assert!(
        events
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
    report(
        label,
        files,
        start,
        io,
        json_codec::json!({"lookups":1000,"page_len":page.len(),"events":events.len()}),
    )
}
fn progress(milestone: &str, completed: usize) -> Result<()> {
    use std::io::Write;
    let mut output = std::io::stderr().lock();
    writeln!(output, "verify {milestone} {completed}")?;
    output.flush()?;
    Ok(())
}
fn verify(engine: &Engine, branch: &str, files: usize, root: &Path) -> Result<()> {
    use std::collections::BTreeMap;
    use storage_contract::{CommitState, NamespaceEntryKind, ProtectionAtPublish};
    progress("namespace_start", 0)?;
    let entries = all_entries(&engine.namespace_tree(branch)?)?;
    let mut expected_paths: Vec<_> = (0..files).map(path_for).collect();
    expected_paths.extend((0..97).map(|index| format!("dir-{index:03}")));
    expected_paths.sort();
    assert_eq!(
        entries.iter().map(|entry| entry.path()).collect::<Vec<_>>(),
        expected_paths
    );
    let mut current: BTreeMap<String, Entry> = BTreeMap::new();
    let mut versions: BTreeMap<String, (Entry, String)> = BTreeMap::new();
    let ids = std::fs::read_to_string(root.join("operations"))?;
    let ids: Vec<OperationId> = ids
        .lines()
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()?;
    assert_eq!(ids.len(), commits());
    assert_eq!(
        ids.iter().collect::<std::collections::BTreeSet<_>>().len(),
        commits()
    );
    let events = all_events(engine)?;
    assert_eq!(events.len(), commits());
    assert_eq!(engine.state().sequence, commits() as u64);
    let branch_record = &engine.state().branches[branch];
    let mut commits = std::collections::BTreeSet::new();
    let mut previous_commit: Option<String> = None;
    for (index, (id, event)) in ids.iter().zip(&events).enumerate() {
        let record = engine.operation_record(*id)?;
        assert_eq!(record.owner(), Some("bench"));
        let receipt = record.result();
        let kind = if index == 0 {
            "repository.create"
        } else {
            "workspace.commit"
        };
        assert_eq!(receipt.operation_id, *id);
        assert_eq!(receipt.kind, kind);
        assert_eq!(receipt.sequence, index as u64 + 1);
        assert_eq!(receipt.commit_state, CommitState::Published);
        assert_eq!(
            receipt.protection_at_publish,
            if index == 0 {
                ProtectionAtPublish::NotApplicable
            } else {
                ProtectionAtPublish::TargetMet
            }
        );
        assert_eq!(receipt.branch_id.as_deref(), Some(branch));
        assert_eq!(
            receipt.history_tree_id.as_deref(),
            Some(branch_record.history_tree_id.as_str())
        );
        let commit_id = receipt
            .commit_id
            .as_ref()
            .expect("invariant: every benchmark operation creates a commit");
        assert!(commits.insert(commit_id.clone()));
        let commit = &engine.state().commits[commit_id];
        assert_eq!(commit.operation_id, *id);
        assert_eq!(commit.sequence, receipt.sequence);
        assert_eq!(commit.history_tree_id, branch_record.history_tree_id);
        assert_eq!(
            commit.parent_commit_ids,
            previous_commit.iter().cloned().collect::<Vec<_>>()
        );
        assert_eq!(event.operation_id, *id);
        assert_eq!(event.kind, kind);
        assert_eq!(event.sequence, receipt.sequence);
        assert_eq!(&event.object_id, commit_id);
        let historical = all_entries(&engine.namespace_tree_at(branch, commit_id)?)?;
        if index == commits() - 1 {
            assert_eq!(historical, entries);
        }
        if index == 0 {
            assert!(historical.is_empty());
        } else {
            assert_eq!(
                historical
                    .iter()
                    .map(|entry| entry.path())
                    .collect::<Vec<_>>(),
                expected_paths
            );
            let changed = (index >= 2).then(|| ((index - 2) * 37) % files);
            for entry in historical {
                let is_file = entry.path().contains('/');
                assert_eq!(
                    entry.kind,
                    if is_file {
                        NamespaceEntryKind::File
                    } else {
                        NamespaceEntryKind::Directory
                    }
                );
                if !is_file {
                    continue;
                }
                let file_index: usize = entry
                    .name()
                    .strip_prefix("file-")
                    .expect("invariant: generated name")
                    .strip_suffix(".txt")
                    .expect("invariant: generated suffix")
                    .parse()?;
                let expected = if index == 1 {
                    format!("initial body {file_index}")
                } else if changed == Some(file_index) {
                    let previous = &current[entry.path()];
                    assert_eq!(entry.file_id, previous.file_id);
                    assert_ne!(entry.version_id, previous.version_id);
                    format!("edited body {file_index} round {}", index - 2)
                } else {
                    assert_eq!(&entry, &current[entry.path()]);
                    continue;
                };
                assert_eq!(entry.size, expected.len() as u64);
                assert!(
                    versions
                        .insert(entry.version_id.clone(), (entry.clone(), expected))
                        .is_none()
                );
                current.insert(entry.path().to_owned(), entry);
            }
        }
        progress("receipt_and_namespace", index + 1)?;
        previous_commit = Some(commit_id.clone());
    }
    assert_eq!(
        previous_commit.as_deref(),
        Some(branch_record.head_commit_id.as_str())
    );
    assert_eq!(versions.len(), files + edits());
    assert_eq!(engine.state().versions.len(), versions.len());
    assert_eq!(engine.state().placements.len(), versions.len());
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.kind == NamespaceEntryKind::File)
            .collect::<Vec<_>>(),
        current.values().collect::<Vec<_>>()
    );
    progress("unique_body_start", 0)?;
    for (index, (version_id, (entry, expected))) in versions.iter().enumerate() {
        let version = &engine.state().versions[version_id];
        assert_eq!(&version.version_id, version_id);
        assert_eq!(version.file_id, entry.file_id);
        assert_eq!(version.history_tree_id, branch_record.history_tree_id);
        assert_eq!(version.creation_primary_slot_id, "P01");
        assert_eq!(version.logical_size, expected.len() as u64);
        let placement = &engine.state().placements[version_id];
        assert_eq!(&placement.version_id, version_id);
        assert_eq!(placement.primary_slot_id, "P01");
        assert_eq!(placement.target_copies, 1);
        assert_eq!(placement.route_slot_ids, ["P01"]);
        let mut content = engine.open_content(version_id)?;
        assert_eq!(content.size, expected.len() as u64);
        assert_eq!(content.content_hash, version.content_hash);
        let mut actual = Vec::new();
        content
            .file
            .by_ref()
            .take(expected.len() as u64 + 1)
            .read_to_end(&mut actual)?;
        assert_eq!(actual, expected.as_bytes());
        if (index + 1) % 100 == 0 || index + 1 == versions.len() {
            progress("unique_bodies_verified", index + 1)?;
        }
    }
    println!(
        "{}",
        json_codec::json!({"phase":"verify","files":files,
        "verified_current_files":files,"verified_bodies":versions.len(),
        "verified_operations":ids.len(),"verified_commit_namespaces":commits(),
        "receipt_api":"operation_record","full_health_api_calls":0})
    );
    Ok(())
}

fn verify_rpc(engine: &Engine, branch: &str, root: &Path) -> Result<()> {
    use storage_contract::{CommitState, ProtectionAtPublish};
    let receipts: Vec<json_codec::Value> = json_codec::from_slice(&std::fs::read(root.join("rpc-receipts.json"))?)?;
    assert_eq!(receipts.len(), 20);
    assert_eq!(engine.state().sequence, (commits() + 20) as u64);
    let tree = engine.namespace_tree(branch)?;
    let entries = all_entries(&tree)?;
    assert_eq!(entries.len(), 597);
    let mut operations = std::collections::BTreeSet::new();
    for (round, wire) in receipts.iter().enumerate() {
        let id: OperationId = wire["operation_id"].as_str().unwrap().parse()?;
        assert!(operations.insert(id));
        let record = engine.operation_record(id)?;
        let receipt = record.result();
        assert_eq!(record.owner(), Some("bench"));
        assert_eq!(receipt.commit_state, CommitState::Published);
        assert_eq!(receipt.protection_at_publish, ProtectionAtPublish::TargetMet);
        assert_eq!(receipt.sequence, (commits() + round + 1) as u64);
        assert_eq!(receipt.kind, "workspace.commit");
        let commit = receipt.commit_id.as_ref().unwrap();
        let history = engine.namespace_tree_at(branch, commit)?;
        assert_eq!(all_entries(&history)?.len(), 597);
        let index = (round * 37) % 500;
        let entry = lookup(&history, &path_for(index))?;
        let expected = format!("RPC saved body {index} round {round}");
        let mut actual = Vec::new();
        engine.open_content(&entry.version_id)?.file.take(4097).read_to_end(&mut actual)?;
        assert_eq!(actual, expected.as_bytes());
    }
    for index in 0..500 {
        let mut expected = format!("initial body {index}");
        for round in 0..edits() {
            if (round * 37) % 500 == index {
                expected = format!("edited body {index} round {round}");
            }
        }
        for round in 0..20 {
            if (round * 37) % 500 == index {
                expected = format!("RPC saved body {index} round {round}");
            }
        }
        let entry = lookup(&tree, &path_for(index))?;
        let mut actual = Vec::new();
        engine.open_content(&entry.version_id)?.file.take(4097).read_to_end(&mut actual)?;
        assert_eq!(actual, expected.as_bytes());
    }
    println!("{}", json_codec::json!({"phase":"rpc-verify","current_files":500,
        "saved_receipts":20,"saved_commit_namespaces":20,"saved_historical_bodies":20,
        "sequence":engine.state().sequence,"reopen":"normal independent Engine::open"}));
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let [phase, files, root] = args.as_slice() else {
        return Err("usage: task1_adoption <build|edit|read|open|verify> <files> <root>".into());
    };
    let files = files.parse::<std::num::NonZeroUsize>()?.get();
    if files != 500 {
        return Err("unsupported measurement size".into());
    }
    let root = Path::new(root);
    let state = root.join("state");
    match phase.as_str() {
        "build" => {
            std::fs::create_dir_all(root)?;
            let mut engine = Engine::initialize(
                &state,
                storage_contract::Config::development([root.join("d1")]),
            )?;
            let create_id = OperationId::generate();
            let repo = engine.create_repository(
                &storage_contract::RepositoryName::parse("Bench")?,
                "P01",
                1,
                "bench",
                create_id,
            )?;
            std::fs::write(root.join("branch"), &repo.main_branch_id)?;
            let io = counters()?;
            let start = Instant::now();
            let batch: Vec<_> = (0..files)
                .map(|index| {
                    (
                        path_for(index),
                        format!("initial body {index}").into_bytes(),
                    )
                })
                .collect();
            let save_id = save(&mut engine, &repo.main_branch_id, &batch, "bench initial")?;
            report("build", files, start, io, json_codec::json!({}))?;
            std::fs::write(root.join("operations"), format!("{create_id}\n{save_id}\n"))?;
        }
        "edit" => {
            let mut engine = Engine::open(&state)?;
            let branch = std::fs::read_to_string(root.join("branch"))?;
            let mut durations = Vec::with_capacity(edits());
            let mut ids = Vec::with_capacity(edits());
            let io = counters()?;
            let start = Instant::now();
            for round in 0..edits() {
                let index = (round * 37) % files;
                let batch = [(
                    path_for(index),
                    format!("edited body {index} round {round}").into_bytes(),
                )];
                let one = Instant::now();
                ids.push(save(&mut engine, &branch, &batch, "bench small edit")?);
                durations.push(one.elapsed().as_nanos());
            }
            report(
                "edit",
                files,
                start,
                io,
                json_codec::json!({"edits":edits(),"operation_ns":durations}),
            )?;
            use std::io::Write;
            let mut output = std::fs::OpenOptions::new()
                .append(true)
                .open(root.join("operations"))?;
            for id in ids {
                writeln!(output, "{id}")?;
            }
        }
        "read" => {
            let engine = Engine::open(&state)?;
            let branch = std::fs::read_to_string(root.join("branch"))?;
            read_once(&engine, &branch, files, "read_after_open")?;
            read_once(&engine, &branch, files, "read_warm_repeat")?;
        }
        "open" => {
            let io = counters()?;
            let start = Instant::now();
            let engine = Engine::open(&state)?;
            assert_eq!(engine.state().commits.len(), commits());
            report(
                "open",
                files,
                start,
                io,
                json_codec::json!({"commits":engine.state().commits.len()}),
            )?;
        }
        "verify" => {
            progress("engine_open_start", 0)?;
            let engine = Engine::open(&state)?;
            progress("engine_open_complete", 0)?;
            let branch = std::fs::read_to_string(root.join("branch"))?;
            verify(&engine, &branch, files, root)?;
        }
        "rpc-verify" => {
            let engine = Engine::open(&state)?;
            let branch = std::fs::read_to_string(root.join("branch"))?;
            verify_rpc(&engine, &branch, root)?;
        }
        _ => return Err("unknown phase".into()),
    }
    Ok(())
}


