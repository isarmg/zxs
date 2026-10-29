//! 任务一基准测量入口（显式 example 测量程序）。
//!
//! 对同一逻辑负载分阶段测量墙钟、/proc/self/io 字节与 VmHWM 峰值内存，
//! 以便与基线实现（独立 worktree）在相同档位下逐项比较。每个阶段以独立
//! 进程运行，使 VmHWM 反映该阶段自身的峰值；输出一行 JSON。
//!
//! 用法：
//! `cargo run --locked --offline --release -p storage-engine --example task1_bench -- <phase> <files> <state-dir>`
//! phase ∈ build | edit | read | open；files 为初始发布的文件数档位。
use std::time::Instant;

fn counter(text: &str, key: &str) -> std::io::Result<u64> {
    let value = text
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| *name == key)
        .and_then(|(_, value)| value.split_whitespace().next())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("missing process counter {key}"),
            )
        })?;
    value
        .parse()
        .map_err(|source| std::io::Error::new(std::io::ErrorKind::InvalidData, source))
}

fn proc_status(key: &str) -> std::io::Result<u64> {
    counter(&std::fs::read_to_string("/proc/self/status")?, key)
}

#[derive(Default, Clone, Copy)]
struct Io {
    rchar: u64,
    wchar: u64,
    read_bytes: u64,
    write_bytes: u64,
}

fn proc_io() -> std::io::Result<Io> {
    let text = std::fs::read_to_string("/proc/self/io")?;
    Ok(Io {
        rchar: counter(&text, "rchar")?,
        wchar: counter(&text, "wchar")?,
        read_bytes: counter(&text, "read_bytes")?,
        write_bytes: counter(&text, "write_bytes")?,
    })
}

fn path_for(index: usize) -> String {
    format!("dir-{:03}/file-{index:06}.txt", index % 97)
}

fn save(
    engine: &mut storage_engine::Engine,
    branch: &str,
    files: &[(String, Vec<u8>)],
    message: &str,
) {
    let workspace = engine.create_workspace(branch, "bench").unwrap();
    for (path, bytes) in files {
        engine
            .stage_bytes(&workspace.workspace_id, path, bytes)
            .unwrap();
    }
    engine
        .commit_workspace(storage_contract::CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "bench".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: message.into(),
            request_digest: None,
        })
        .unwrap();
}

fn report(
    phase: &str,
    files: usize,
    wall_ms: u128,
    io_before: Io,
    extra: &str,
) -> std::io::Result<()> {
    let io = proc_io()?;
    println!(
        "{{\"impl\":\"paged\",\"phase\":\"{phase}\",\"files\":{files},\"wall_ms\":{wall_ms},\
\"rchar\":{},\"wchar\":{},\"read_bytes\":{},\"write_bytes\":{},\"vmhwm_kb\":{}{extra}}}",
        io.rchar - io_before.rchar,
        io.wchar - io_before.wchar,
        io.read_bytes - io_before.read_bytes,
        io.write_bytes - io_before.write_bytes,
        proc_status("VmHWM")?,
    );
    Ok(())
}

fn main() -> std::io::Result<()> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [phase, files, root] = arguments.as_slice() else {
        eprintln!("usage: task1_bench <build|edit|read|open> <files> <state-dir>");
        std::process::exit(2);
    };
    let files = files
        .parse::<std::num::NonZeroUsize>()
        .map_err(|source| std::io::Error::new(std::io::ErrorKind::InvalidInput, source))?
        .get();
    let root = std::path::PathBuf::from(root);
    let state = root.join("state");
    let config = storage_contract::Config::development([root.join("d1")]);
    match phase.as_str() {
        "build" => {
            std::fs::create_dir_all(&root).unwrap();
            let mut engine = storage_engine::Engine::initialize(&state, config).unwrap();
            let repo = engine
                .create_repository(
                    &storage_contract::RepositoryName::parse("Bench").unwrap(),
                    "P01",
                    1,
                    "bench",
                    storage_contract::OperationId::generate(),
                )
                .unwrap();
            std::fs::write(root.join("branch"), &repo.main_branch_id).unwrap();
            let io = proc_io()?;
            let start = Instant::now();
            let batch: Vec<_> = (0..files)
                .map(|index| {
                    (
                        path_for(index),
                        format!("initial body {index}").into_bytes(),
                    )
                })
                .collect();
            save(&mut engine, &repo.main_branch_id, &batch, "bench initial");
            report("build", files, start.elapsed().as_millis(), io, "")?;
        }
        "edit" => {
            let mut engine = storage_engine::Engine::open(&state).unwrap();
            let branch = std::fs::read_to_string(root.join("branch")).unwrap();
            let edits = 20usize;
            let io = proc_io()?;
            let start = Instant::now();
            for round in 0..edits {
                let index = (round * 37) % files;
                let batch = vec![(
                    path_for(index),
                    format!("edited body {index} round {round}").into_bytes(),
                )];
                save(&mut engine, &branch, &batch, "bench small edit");
            }
            let wall = start.elapsed().as_millis();
            report("edit", files, wall, io, &format!(",\"edits\":{edits}"))?;
        }
        "read" => {
            let engine = storage_engine::Engine::open(&state).unwrap();
            let branch = std::fs::read_to_string(root.join("branch")).unwrap();
            let io = proc_io()?;
            let start = Instant::now();
            let tree = engine.namespace_tree(&branch).unwrap();
            let lookups = 1000usize;
            let mut found = 0usize;
            for round in 0..lookups {
                let index = (round * 101) % files;
                if tree.get(&path_for(index)).unwrap().is_some() {
                    found += 1;
                }
            }
            let page = tree
                .directory_page("dir-001", None, 200)
                .unwrap()
                .unwrap_or_default();
            let events = engine.events_page(0, 200).unwrap();
            let wall = start.elapsed().as_millis();
            report(
                "read",
                files,
                wall,
                io,
                &format!(
                    ",\"lookups\":{lookups},\"found\":{found},\"page_len\":{},\"events\":{}",
                    page.len(),
                    events.len()
                ),
            )?;
        }
        "open" => {
            let io = proc_io()?;
            let start = Instant::now();
            let engine = storage_engine::Engine::open(&state).unwrap();
            let wall = start.elapsed().as_millis();
            let commits = engine.state().commits.len();
            report(
                "open",
                files,
                wall,
                io,
                &format!(
                    ",\"commits\":{commits},\"vmrss_kb\":{}",
                    proc_status("VmRSS")?
                ),
            )?;
        }
        other => {
            eprintln!("unknown phase {other}");
            std::process::exit(2);
        }
    }
    Ok(())
}
