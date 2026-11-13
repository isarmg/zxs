//! Cross-revision executable wrapper. The Engine always comes from its own checkout.
use std::{env, fs, path::Path, os::unix::fs::PermissionsExt};
use std::io::Read;
use storage_engine::{Config, Engine, OperationId};
use platform_io::fs_safety::PrivateDirectory;

const BODY: &[u8] = b"cross-revision format fence retained content";

fn private(path: &Path) {
    fs::create_dir(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn seed(root: &Path) {
    let system = root.join("system");
    let disk = root.join("disk");
    private(&system);
    private(&disk);
    let config = Config::development(vec![disk]).unwrap();
    let mut engine = Engine::initialize_storage(&system, config, OperationId::generate().unwrap()).unwrap();
    let repository = engine.create_repository(
        &storage_contract::RepositoryName::parse("format fence").unwrap(),
        "P01", 1, "admin", OperationId::generate().unwrap(),
    ).unwrap();
    let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "preserved.txt", BODY).unwrap();
    engine.commit_workspace(storage_engine::CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate().unwrap(), principal: "admin".into(),
        expected_head: workspace.base_head, fencing_token: workspace.fencing_token,
        message: "real format evidence".into(), request_digest: None,
    }).unwrap();
    fs::write(root.join("branch"), repository.main_branch_id).unwrap();
    println!("seeded real storage and immutable content");
}

fn open(root: &Path) -> i32 {
    match Engine::open(&root.join("system")) {
        Ok(engine) => {
            let branch = fs::read_to_string(root.join("branch")).unwrap();
            let entries = engine.list_tree(&branch).unwrap();
            let entry = entries.iter().find(|entry| entry.path() == "preserved.txt").unwrap();
            let mut content = engine.open_content(&entry.version_id).unwrap();
            let mut bytes = Vec::new();
            content.file.read_to_end(&mut bytes).unwrap();
            assert_eq!(content.size, BODY.len() as u64);
            assert_eq!(bytes, BODY);
            println!("opened exact retained file through actual Engine");
            0
        }
        Err(error) => { println!("explicit Engine refusal: {error:?}"); 73 }
    }
}

fn admit(root: &Path) -> i32 {
    match Engine::admit_storage_recovery(vec![PrivateDirectory::open_existing(&root.join("disk")).unwrap()]) {
        Ok(_) => { println!("actual Engine source admission succeeded"); 0 }
        Err(error) => { println!("explicit source admission refusal: {error:?}"); 73 }
    }
}

// CURRENT_RECOVERY_IMPLEMENTATION

fn main() {
    let args = env::args().collect::<Vec<_>>();
    assert_eq!(args.len(), 3);
    let root = Path::new(&args[2]);
    let code = match args[1].as_str() {
        "seed" => { seed(root); 0 }
        "open" => open(root),
        "admit" => admit(root),
        // CURRENT_RECOVERY_COMMAND
        _ => panic!("unknown explicit fence operation"),
    };
    std::process::exit(code);
}
