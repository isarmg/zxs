//! Recovery-cost carrier; the timed task1 build/edit/read/open load is unchanged.
use std::{io::Read, path::Path, time::Instant};
use storage_contract::{Entry, Event, OperationId};
use storage_engine::{Engine, NamespaceTree};
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
fn lookup(tree: &NamespaceTree<'_>, path: &str) -> Result<Entry> {
    Ok(tree.get(path)?.expect("invariant: generated file exists"))
}
fn directory_page(tree: &NamespaceTree<'_>) -> Result<Vec<Entry>> {
    Ok(tree
        .directory_page("dir-001", None, 200)?
        .expect("invariant: generated directory exists"))
}
fn event_page(engine: &Engine) -> Result<Vec<Event>> {
    Ok(engine.events_page(0, 200)?)
}

fn all_entries(engine: &Engine, branch: &str) -> Result<Vec<Entry>> {
    Ok(engine.namespace_tree(branch)?.entries()?)
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
    assert_eq!(events.len(), 22);
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
fn verify(engine: &Engine, branch: &str, files: usize, root: &Path) -> Result<()> {
    let entries = all_entries(engine, branch)?;
    let mut expected_paths: Vec<_> = (0..files).map(path_for).collect();
    expected_paths.extend((0..97).map(|index| format!("dir-{index:03}")));
    expected_paths.sort();
    assert_eq!(
        entries.iter().map(|entry| entry.path()).collect::<Vec<_>>(),
        expected_paths
    );
    let tree = engine.namespace_tree(branch)?;
    for index in 0..files {
        let entry = lookup(&tree, &path_for(index))?;
        let expected = match (0..20).rev().find(|round| (round * 37) % files == index) {
            Some(round) => format!("edited body {index} round {round}"),
            None => format!("initial body {index}"),
        };
        assert_eq!(entry.size, expected.len() as u64);
        let mut content = engine.open_content(&entry.version_id)?;
        let mut actual = Vec::new();
        content
            .file
            .by_ref()
            .take(expected.len() as u64 + 1)
            .read_to_end(&mut actual)?;
        assert_eq!(actual, expected.as_bytes());
    }
    use storage_contract::{CommitState, NamespaceEntryKind, ProtectionAtPublish};
    for entry in &entries {
        let expected_kind = if entry.path().contains('/') {
            NamespaceEntryKind::File
        } else {
            NamespaceEntryKind::Directory
        };
        assert_eq!(entry.kind, expected_kind);
    }
    let ids = std::fs::read_to_string(root.join("operations"))?;
    let ids: Vec<OperationId> = ids
        .lines()
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()?;
    assert_eq!(ids.len(), 22);
    assert_eq!(
        ids.iter().collect::<std::collections::BTreeSet<_>>().len(),
        22
    );
    let events = event_page(engine)?;
    assert_eq!(events.len(), 22);
    assert_eq!(engine.state().sequence, 22);
    let branch_record = &engine.state().branches[branch];
    let mut commits = std::collections::BTreeSet::new();
    let mut previous_commit: Option<String> = None;
    for (index, (id, event)) in ids.iter().zip(&events).enumerate() {
        let receipt = engine.published_receipt(*id)?.expect("invariant: retained receipt exists");
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
        previous_commit = Some(commit_id.clone());
    }
    assert_eq!(
        previous_commit.as_deref(),
        Some(branch_record.head_commit_id.as_str())
    );
    let expected_bodies: std::collections::BTreeSet<_> = (0..files)
        .map(|index| format!("initial body {index}").into_bytes())
        .chain((0..20).map(|round| {
            let index = (round * 37) % files;
            format!("edited body {index} round {round}").into_bytes()
        }))
        .collect();
    assert_eq!(engine.state().versions.len(), files + 20);
    let mut actual_bodies = std::collections::BTreeSet::new();
    for version in engine.state().versions.values() {
        let mut content = engine.open_content(&version.version_id)?;
        let mut bytes = Vec::new();
        content.file.by_ref().take(version.logical_size + 1).read_to_end(&mut bytes)?;
        assert_eq!(bytes.len() as u64, version.logical_size);
        assert!(actual_bodies.insert(bytes), "each retained generated body is unique");
    }
    assert_eq!(actual_bodies, expected_bodies);
    let count = ids.len();
    println!(
        "{}",
        json_codec::json!({"phase":"verify","files":files,"verified_current_bodies":files,"verified_retained_bodies":files+20,"verified_operations":count})
    );
    Ok(())
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let [phase, files, root] = args.as_slice() else {
        return Err("usage: task1_adoption <build|edit|read|open|verify> <files> <root>".into());
    };
    let files = files.parse::<std::num::NonZeroUsize>()?.get();
    if ![500, 1500, 3000].contains(&files) {
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
            let mut durations = Vec::with_capacity(20);
            let mut ids = Vec::with_capacity(20);
            let io = counters()?;
            let start = Instant::now();
            for round in 0..20 {
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
                json_codec::json!({"edits":20,"operation_ns":durations}),
            )?;
            use std::io::Write;
            let mut output = std::fs::OpenOptions::new()
                .append(true)
                .open(root.join("operations"))?;
            for id in ids {
                writeln!(output, "{id}")?;
            }
            std::fs::write(root.join("expected-state.json"), json_codec::to_vec(engine.state())?)?;
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
            assert_eq!(engine.state().commits.len(), 22);
            report(
                "open",
                files,
                start,
                io,
                json_codec::json!({"commits":engine.state().commits.len()}),
            )?;
            // Exact State comparison is outside the Engine::open timing/counters.
            let expected: storage_contract::State = json_codec::from_slice(
                &std::fs::read(root.join("expected-state.json"))?,
            )?;
            assert_eq!(engine.state(), &expected);
        }
        "verify" => {
            let engine = Engine::open(&state)?;
            let branch = std::fs::read_to_string(root.join("branch"))?;
            verify(&engine, &branch, files, root)?;
        }
        _ => return Err("unknown phase".into()),
    }
    Ok(())
}
