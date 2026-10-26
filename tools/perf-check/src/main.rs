//! Disposable Development-mode architecture probe, not a physical-storage benchmark.
use json_codec::{Value, json};
use sqlite_access::{Executor, Row};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Seek, SeekFrom},
    path::Path,
    time::Instant,
};
use storage_engine::{CommitRequest, Config, Engine, new_id};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn save(engine: &mut Engine, branch: &str, bytes: &[u8]) -> Result<()> {
    let w = engine.create_workspace(branch, "admin")?;
    engine.stage_bytes(&w.workspace_id, "sample.bin", bytes)?;
    engine.commit_workspace(CommitRequest {
        workspace_id: w.workspace_id,
        operation_id: new_id("op"),
        principal: "admin".into(),
        expected_head: w.base_head,
        fencing_token: w.fencing_token,
        message: "Architecture workload".into(),
        request_digest: None,
    })?;
    Ok(())
}
fn version(engine: &Engine, branch: &str) -> String {
    engine
        .list_tree(branch)
        .unwrap()
        .into_iter()
        .find(|e| e.path == "sample.bin")
        .unwrap()
        .version_id
}
fn sql(path: &Path, install: bool) -> Result<Value> {
    platform_io::sqlite::block_on_sqlite_connection(async {
        let mut conn = platform_io::sqlite::open_index_connection(path, false, None).await?;
        if install {
            // Count only typed history rows, not generic mirrors, the fixed
            // current_state marker, or these instrumentation updates.
            conn.execute("CREATE TABLE IF NOT EXISTS perf_history_writes(inserts INTEGER,updates INTEGER,deletes INTEGER);DELETE FROM perf_history_writes;INSERT INTO perf_history_writes VALUES(0,0,0);CREATE TRIGGER IF NOT EXISTS perf_history_insert AFTER INSERT ON file_history_versions BEGIN UPDATE perf_history_writes SET inserts=inserts+1;END;CREATE TRIGGER IF NOT EXISTS perf_history_update AFTER UPDATE ON file_history_versions BEGIN UPDATE perf_history_writes SET updates=updates+1;END;CREATE TRIGGER IF NOT EXISTS perf_history_delete AFTER DELETE ON file_history_versions BEGIN UPDATE perf_history_writes SET deletes=deletes+1;END;").await?;
        }
        let r = sqlite_access::query("SELECT inserts,updates,deletes FROM perf_history_writes")
            .fetch_one(&mut conn)
            .await?;
        Ok::<Value, Box<dyn std::error::Error>>(
            json!({"inserts":r.get::<i64,_>(0),"updates":r.get::<i64,_>(1),"deletes":r.get::<i64,_>(2)}),
        )
    })
}
// Accept only the two persisted object layouts, never report a shard prefix
// as an object kind or count malformed evidence as a successful benchmark.
fn metadata_object_kind(path: &Path) -> Result<Option<String>> {
    let parts = path
        .iter()
        .map(|p| p.to_str())
        .collect::<Option<Vec<_>>>()
        .ok_or("non-UTF-8 benchmark path")?;
    let Some(start) = parts
        .windows(2)
        .position(|pair| pair == ["metadata", "objects"])
    else {
        return Ok(None);
    };
    let tail = &parts[start + 2..];
    if !(tail.len() == 2 || tail.len() == 3) {
        return Err("invalid metadata object layout".into());
    }
    let kind = tail[0];
    let hash = tail
        .last()
        .and_then(|name| name.strip_suffix(".json"))
        .ok_or("invalid metadata object filename")?;
    if kind.is_empty()
        || !kind.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
        || hash.len() != 64
        || !hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || (tail.len() == 3 && tail[1] != &hash[..2])
    {
        return Err("invalid metadata object address".into());
    }
    Ok(Some(kind.to_owned()))
}
fn disk_bytes(root: &Path) -> Result<Value> {
    fn walk(
        root: &Path,
        counts: &mut [u64; 4],
        kinds: &mut BTreeMap<String, (u64, u64)>,
    ) -> Result<()> {
        for e in fs::read_dir(root)? {
            let e = e?;
            let meta = e.file_type()?;
            if meta.is_dir() {
                walk(&e.path(), counts, kinds)?;
            } else if meta.is_file() {
                let p = e.path();
                let name = p.to_string_lossy();
                let n = e.metadata()?.len();
                counts[0] += n;
                counts[1] += 1;
                if name.contains("/metadata/application-log/") {
                    counts[2] += n;
                }
                if let Some(kind) = metadata_object_kind(&p)? {
                    counts[3] += n;
                    let entry = kinds.entry(kind).or_default();
                    entry.0 += n;
                    entry.1 += 1;
                }
            }
        }
        Ok(())
    }
    let mut c = [0; 4];
    let mut kinds = BTreeMap::new();
    walk(root, &mut c, &mut kinds)?;
    let kinds: BTreeMap<_, _> = kinds
        .into_iter()
        .map(|(kind, (bytes, files))| (kind, json!({"bytes": bytes, "files": files})))
        .collect();
    Ok(
        json!({"file_bytes":c[0],"files":c[1],"application_log_bytes_all_members":c[2],"metadata_object_bytes_all_members":c[3],"metadata_objects_by_kind_all_members":kinds}),
    )
}
fn range(engine: &Engine, id: &str, expected: &[u8], repetitions: usize) -> Result<u128> {
    let now = Instant::now();
    for i in 0..repetitions {
        let mut content = engine.open_content(id)?;
        let offset = (i * 997) % (expected.len() - 64);
        content.file.seek(SeekFrom::Start(offset as u64))?;
        let mut b = [0u8; 64];
        content.file.read_exact(&mut b)?;
        assert_eq!(&b, &expected[offset..offset + 64]);
    }
    Ok(now.elapsed().as_micros())
}
fn main() -> Result<()> {
    let history: usize = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "64".into())
        .parse()?;
    if !(4..=4096).contains(&history) {
        return Err("history must be 4..=4096".into());
    }
    let temp = tempfile::Builder::new()
        .prefix("zxs-perf-development-")
        .tempdir()?;
    let root = temp.path().join("system");
    let config = Config::development([
        temp.path().join("primary"),
        temp.path().join("backup"),
        temp.path().join("tertiary"),
    ]);
    let mut engine = Engine::initialize(&root, config)?;
    let repo = engine.create_repository("Architecture probe", "P01", 3, "admin", &new_id("op"))?;
    let branch = repo.main_branch_id;
    let mut checkpoints = vec![1, history / 4, history / 2, history];
    checkpoints.sort_unstable();
    checkpoints.dedup();
    let mut samples = Vec::new();
    let mut expected = vec![0u8; 1024];
    for n in 1..=history {
        expected.fill((n % 251) as u8);
        expected[..8].copy_from_slice(&(n as u64).to_le_bytes());
        let checkpoint = checkpoints.contains(&n);
        if checkpoint {
            sql(&root.join("index.sqlite"), true)?;
        }
        let start = Instant::now();
        save(&mut engine, &branch, &expected)?;
        let save_us = start.elapsed().as_micros();
        if checkpoint {
            let writes = sql(&root.join("index.sqlite"), false)?;
            let id = version(&engine, &branch);
            let before = json_codec::to_value(engine.state())?;
            drop(engine);
            let start = Instant::now();
            engine = Engine::open(&root)?;
            let restart_us = start.elapsed().as_micros();
            assert_eq!(
                json_codec::to_value(engine.state())?,
                before,
                "restart must recover identical state"
            );
            let read_us = range(&engine, &id, &expected, 8)?;
            // Main database apparent length after reopen; excludes journal/WAL
            // sidecars and includes the probe's own counter table.
            let index_sqlite_bytes = fs::metadata(root.join("index.sqlite"))?.len();
            samples.push(json!({"history_versions":n,"save_us":save_us,"restart_us":restart_us,"range_open_read_8_us":read_us,"sqlite_history_writes":writes,"index_sqlite_bytes":index_sqlite_bytes,"disk":disk_bytes(temp.path())?}));
        }
    }
    // Larger immutable version: open/verify cost is intentionally included in every range request.
    let big: Vec<u8> = (0..4 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
    save(&mut engine, &branch, &big)?;
    let old_id = version(&engine, &branch);
    let cold_us = range(&engine, &old_id, &big, 1)?;
    let warm_us = range(&engine, &old_id, &big, 16)?;
    // The owned open descriptor and lease are used while the sole writer saves a new version.
    // This exercises read-under-write without claiming parallel Engine calls/server scheduling.
    let mut opened = engine.open_content(&old_id)?;
    let reader_expected = big.clone();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let reader_barrier = barrier.clone();
    let reader = std::thread::spawn(move || {
        reader_barrier.wait();
        for _ in 0..4 {
            opened.file.seek(SeekFrom::Start(0)).unwrap();
            let mut b = Vec::new();
            opened.file.read_to_end(&mut b).unwrap();
            assert_eq!(b, reader_expected);
        }
        drop(opened);
    });
    barrier.wait();
    save(&mut engine, &branch, &expected)?;
    reader.join().expect("concurrent held read failed");
    range(&engine, &old_id, &big, 1)?;
    assert!(engine.check()?.issues.is_empty(), "integrity check failed");
    println!(
        "{}",
        json_codec::to_string_pretty(
            &json!({"mode":"Development","physical_redundancy_evidence":false,"timings_are_gates":false,"history_limit":history,"small_file_bytes":1024,"copies":3,"samples":samples,"large_range":{"file_bytes":big.len(),"range_bytes":64,"first_open_read_us":cold_us,"repeated_open_read_16_us":warm_us},"checks":{"restart_state_equality":true,"range_bytes_correct":true,"held_read_during_save":true,"historical_version_after_save":true,"integrity_check":true}})
        )?
    );
    Ok(())
}

#[cfg(test)]
#[path = "../../../test/tools/perf-check/unit/main_tests.rs"]
mod tests;
