//! Inspect the actual persisted SQLite, not a JSON stand-in or mocked writer.
use super::*;
use sqlite_access::{Connection, Row, SqliteConnection, sqlite::SqliteConnectOptions};

const NAME: &str = ".zxs-directory.sqlite";

pub(super) fn records(path: &Path) -> BTreeMap<(String, String), json_codec::Value> {
    platform_io::sqlite::block_on_sqlite_connection(async {
        let options = SqliteConnectOptions::new().filename(path).read_only(true);
        let mut connection = SqliteConnection::connect_with(&options).await.unwrap();
        let rows = sqlite_access::query("SELECT kind,id,body FROM records ORDER BY kind,id")
            .fetch_all(&mut connection)
            .await
            .unwrap();
        let result = rows
            .into_iter()
            .map(|row| {
                let body: String = row.try_get("body").unwrap();
                (
                    (row.try_get("kind").unwrap(), row.try_get("id").unwrap()),
                    json_codec::from_str(&body).unwrap(),
                )
            })
            .collect();
        connection.close().await.unwrap();
        result
    })
}

#[test]
fn directory_database_contains_own_branches_index_and_relative_history_and_rebuilds() {
    let mut fixture = Fixture::new();
    fixture.save("outside/ignored.txt", b"outside");
    let original = fixture.save("box/file.txt", b"first");
    fixture.initialize("");
    let initialized = fixture.initialize("box");
    let directory = initialized.directory_id;
    let main = initialized.directory_branch_id.unwrap();
    let checkpoint = fixture.checkpoint("box");
    let head = checkpoint.head_tree_id.unwrap();
    let topic = fixture.create_branch("box", "topic");
    let root_head = fixture.checkpoint("").head_tree_id.unwrap();
    // Root and child are physically materialized by the real Engine entry.
    fixture
        .engine
        .reconcile_directory_layout(&fixture.repository.repository_id)
        .unwrap();
    let parent = records(&fixture.root().join(NAME));
    let parent_tree = &parent[&("tree".into(), root_head)];
    assert!(parent_tree["directory_refs"].get(&directory).is_some());
    assert!(
        !parent_tree["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["file_id"] == original.file_id)
    );
    assert!(!parent.contains_key(&("branch".into(), main.clone())));
    let path = fixture.root().join("box").join(NAME);
    let data = records(&path);
    let git = &data[&("repository".into(), directory.clone())];
    assert_eq!(git["main_branch_id"], main);
    assert_eq!(git["branches"][&main]["head_tree_id"], head);
    assert!(data.contains_key(&("branch".into(), topic)));
    assert!(data.contains_key(&("tree".into(), head)));
    assert!(
        data.values()
            .filter_map(|value| value.get("entries"))
            .flat_map(|value| value.as_array().unwrap())
            .any(|entry| entry["file_id"] == original.file_id && entry["path"] == "file.txt")
    );
    for ((kind, _), value) in &data {
        if kind == "tree" {
            assert_eq!(value["path"], "");
            assert!(
                !value["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|entry| entry["path"].as_str().unwrap().starts_with("outside/"))
            );
        }
    }
    let inode = identity(&fixture.root().join("box"));
    fixture.rename("box", "renamed");
    assert_eq!(identity(&fixture.root().join("renamed")), inode);
    assert!(!path.exists());
    let path = fixture.root().join("renamed").join(NAME);
    let data = records(&path);
    assert_eq!(
        data[&("repository".into(), directory.clone())]["main_branch_id"],
        main
    );
    let mut fixture = fixture.reopen();
    assert_eq!(records(&path).len(), data.len());
    fs::remove_file(&path).unwrap();
    fixture = fixture.reopen();
    assert_eq!(records(&path).len(), data.len());
    // Missing derived material is rebuilt from the verified current publication,
    // not adopted as an empty directory repository or a new operation.
    assert_eq!(
        fixture.engine.state().directory_git_repositories[&directory].main_branch_id,
        main
    );
}

#[test]
fn directory_database_automatically_tracks_saved_staged_and_committed_compositions() {
    let mut fixture = Fixture::new();
    fixture.save("file.txt", b"before");
    fixture.initialize("");
    fixture
        .engine
        .reconcile_directory_layout(&fixture.repository.repository_id)
        .unwrap();
    let first = records(&fixture.root().join(NAME));
    let saved = fixture.save("file.txt", b"after");
    let status = fixture.status("");
    let working = status.working_tree_id.unwrap();
    let second = records(&fixture.root().join(NAME));
    let git = &second[&(
        "repository".into(),
        fixture.repository.repository_id.clone(),
    )];
    let main = git["main_branch_id"].as_str().unwrap();
    assert_ne!(git["branches"][main]["index_tree_id"], working);
    assert!(git["branches"][main]["head_tree_id"].is_null());
    assert!(second.len() > first.len());
    assert!(
        second[&("tree".into(), working)]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["version_id"] == saved.version_id)
    );
    let status = fixture.checkpoint("");
    let head = status.head_tree_id.unwrap();
    let data = records(&fixture.root().join(NAME));
    assert!(
        data.contains_key(&(
            "commit".into(),
            fixture.engine.state().directory_trees[&head]
                .commit_id
                .clone()
        ))
    );
    let fixture = fixture.reopen();
    assert_eq!(records(&fixture.root().join(NAME)).len(), data.len());
}

#[test]
fn directory_database_reserved_names_links_and_unknown_bytes_are_preserved_and_rejected() {
    let mut collision = Fixture::new();
    collision.save(NAME, b"a user file must not be overwritten");
    collision.initialize("");
    let sequence = collision.engine.state().sequence;
    assert_eq!(
        collision
            .engine
            .reconcile_directory_layout(&collision.repository.repository_id)
            .unwrap_err()
            .code(),
        ErrorCode::NameCollision
    );
    assert_eq!(collision.engine.state().sequence, sequence);
    assert!(!collision.root().join(NAME).exists());

    let mut fixture = Fixture::new();
    fixture.save("file.txt", b"preserve body");
    fixture.initialize("");
    fixture
        .engine
        .reconcile_directory_layout(&fixture.repository.repository_id)
        .unwrap();
    let path = fixture.root().join(NAME);
    let valid = fs::read(&path).unwrap();
    fs::write(&path, b"unexpected data").unwrap();
    assert!(
        fixture
            .engine
            .reconcile_directory_layout(&fixture.repository.repository_id)
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), b"unexpected data");
    fs::remove_file(&path).unwrap();
    let victim = fixture.temporary.path().join("do-not-touch");
    fs::write(&victim, b"outside").unwrap();
    std::os::unix::fs::symlink(&victim, &path).unwrap();
    assert!(
        fixture
            .engine
            .reconcile_directory_layout(&fixture.repository.repository_id)
            .is_err()
    );
    assert_eq!(fs::read(&victim).unwrap(), b"outside");
    fs::remove_file(&path).unwrap();
    fs::write(&path, &valid).unwrap();
    platform_io::sqlite::block_on_sqlite_connection(async {
        let mut connection =
            SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path))
                .await
                .unwrap();
        sqlite_access::query("UPDATE identity SET directory_id=?")
            .bind(format!("entry_{}", "f".repeat(32)))
            .execute(&mut connection)
            .await
            .unwrap();
        connection.close().await.unwrap();
    });
    let wrong_owner = fs::read(&path).unwrap();
    assert!(
        fixture
            .engine
            .reconcile_directory_layout(&fixture.repository.repository_id)
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), wrong_owner);
    fs::write(&path, valid).unwrap();
    fixture
        .engine
        .reconcile_directory_layout(&fixture.repository.repository_id)
        .unwrap();
}

#[test]
fn directory_database_excludes_outside_and_child_pending_reservations() {
    let mut fixture = Fixture::new();
    let outside = fixture.save("outside.txt", b"outside");
    let own = fixture.save("box/own.txt", b"own");
    let child = fixture.save("box/child/nested.txt", b"child");
    fixture.initialize("");
    fixture.initialize("box");
    fixture.initialize("box/child");
    for (path, entry) in [
        ("outside.txt", &outside),
        ("box/own.txt", &own),
        ("box/child/nested.txt", &child),
    ] {
        let branch = fixture
            .engine
            .create_file_branch(
                &entry.file_id,
                &entry.version_id,
                &BranchName::parse("pending").unwrap(),
                "admin",
                op(),
            )
            .unwrap()
            .branch_id
            .unwrap();
        fixture
            .engine
            .select_file_branch_at(
                &fixture.repository.repository_id,
                &name_policy::PortablePath::parse(path).unwrap(),
                &entry.file_id,
                &branch,
                "admin",
                op(),
            )
            .unwrap();
    }
    fixture.checkpoint("box/child");
    let head = fixture.checkpoint("box").head_tree_id.unwrap();
    let commit_id = fixture.engine.state().directory_trees[&head]
        .commit_id
        .clone();
    let source = &fixture.engine.state().commits[&commit_id];
    // The authoritative namespace commit deliberately retains the outside scope;
    // the directory-local database must not copy its reservation side table.
    assert!(source.pending_file_versions.contains_key(&outside.file_id));
    assert!(source.pending_file_versions.contains_key(&own.file_id));
    fixture
        .engine
        .reconcile_directory_layout(&fixture.repository.repository_id)
        .unwrap();
    let data = records(&fixture.root().join("box").join(NAME));
    let local = &data[&("commit".into(), commit_id)];
    assert_eq!(local["pending_file_versions"][&own.file_id], own.version_id);
    assert!(
        local["pending_file_versions"]
            .get(&outside.file_id)
            .is_none()
    );
    assert!(local["pending_file_versions"].get(&child.file_id).is_none());
    for ((kind, _), value) in &data {
        if kind == "commit" {
            let entries = value["entries"].as_array().unwrap();
            if let Some(pending) = value["pending_file_versions"].as_object() {
                for (file, version) in pending {
                    assert!(entries.iter().any(|entry| {
                        entry["file_id"] == *file && entry["version_id"] == *version
                    }));
                }
            }
        }
    }
}
