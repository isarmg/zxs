//! Publication can succeed before either workspace manifest is durable.
use std::{fs, os::unix::fs::PermissionsExt, path::Path};
use storage_engine::{Config, Engine, new_id};

fn corrupt_retained_bytes(path: &Path) {
    // Published generations are read-only. Deliberate out-of-band corruption
    // must not rely on the test runner having root's permission bypass.
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(path, b"oops").unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o400)).unwrap();
}

#[test]
fn upload_retry_recovers_published_bytes_after_manifest_failure() {
    for primary_failure in [true, false] {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("system");
        let primary = temporary.path().join("primary");
        let mut config = Config::development([
            primary.clone(),
            temporary.path().join("backup"),
            temporary.path().join("tertiary"),
        ]);
        config.max_file_bytes = 4;
        config.max_workspace_bytes = 4;
        let mut engine = Engine::initialize(&root, config).unwrap();
        let repo = engine
            .create_repository("Retry uploads", "P01", 1, "admin", &new_id("op"))
            .unwrap();
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        let item = new_id("entry");
        let source = temporary.path().join("source");
        fs::write(&source, b"data").unwrap();
        let data = primary
            .join("staging/workspaces")
            .join(&workspace.workspace_id);
        let manifest = if primary_failure {
            data.join("workspace.json")
        } else {
            root.join("workspaces")
                .join(&workspace.workspace_id)
                .join("workspace.json")
        };
        let original_manifest = fs::read(&manifest).unwrap();
        fs::remove_file(&manifest).unwrap();
        fs::create_dir(&manifest).unwrap();
        assert!(
            engine
                .stage_upload(&workspace.workspace_id, &item, "a", &source, 1)
                .is_err()
        );
        let published = data.join(format!("{item}-1.full"));
        assert_eq!(fs::read(&published).unwrap(), b"data");
        fs::remove_dir(&manifest).unwrap();
        fs::write(&manifest, original_manifest).unwrap();
        // A possibly durable mirror write fences this process even if
        // the failing path was repaired externally.
        assert_eq!(
            engine
                .stage_upload(&workspace.workspace_id, &item, "a", &source, 1)
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::RecoveryRequired
        );
        assert_eq!(fs::read(&published).unwrap(), b"data");
        drop(engine);
        if !primary_failure {
            // The primary advanced, but the restored system manifest is
            // valid-old. No publication receipt proves an upload-only
            // transition, so neither valid mirror may silently win.
            let data_manifest = data.join("workspace.json");
            let retained_data = fs::read(&data_manifest).unwrap();
            let retained_system = fs::read(&manifest).unwrap();
            assert_eq!(
                Engine::open(&root).err().unwrap().code(),
                storage_contract::ErrorCode::RecoveryRequired
            );
            assert_eq!(fs::read(data_manifest).unwrap(), retained_data);
            assert_eq!(fs::read(manifest).unwrap(), retained_system);
            assert_eq!(fs::read(&published).unwrap(), b"data");
            continue;
        }
        engine = Engine::open(&root).unwrap();
        // A retry must never overwrite an already published generation.
        fs::write(&source, b"oops").unwrap();
        assert_eq!(
            engine
                .stage_upload(&workspace.workspace_id, &item, "a", &source, 1)
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::UploadGenerationConflict
        );
        assert_eq!(fs::read(&published).unwrap(), b"data");
        fs::write(&source, b"data").unwrap();
        let staged = engine
            .stage_upload(&workspace.workspace_id, &item, "a", &source, 1)
            .unwrap();
        assert_eq!(staged.source_path, published);
        assert_eq!(staged.size, 4);
        assert_eq!(
            engine.workspace(&workspace.workspace_id).unwrap().files[&item].content_hash,
            staged.content_hash
        );
        assert_eq!(
            engine
                .stage_upload(&workspace.workspace_id, &new_id("entry"), "b", &source, 1)
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::ResourceLimit
        );
        drop(engine);
        let engine = Engine::open(&root).unwrap();
        assert_eq!(
            engine.workspace(&workspace.workspace_id).unwrap().files[&item].content_hash,
            staged.content_hash
        );
    }
}

#[test]
fn same_generation_retry_rejects_missing_or_corrupt_retained_bytes() {
    for missing in [false, true] {
        for reopen in [false, true] {
            let temporary = tempfile::tempdir().unwrap();
            let root = temporary.path().join("system");
            let mut engine = Engine::initialize(
                &root,
                Config::development([
                    temporary.path().join("primary"),
                    temporary.path().join("backup"),
                    temporary.path().join("tertiary"),
                ]),
            )
            .unwrap();
            let repo = engine
                .create_repository("Retained upload", "P01", 1, "admin", &new_id("op"))
                .unwrap();
            let workspace = engine
                .create_workspace(&repo.main_branch_id, "admin")
                .unwrap();
            let item = new_id("entry");
            let source = temporary.path().join("source");
            fs::write(&source, b"data").unwrap();
            let staged = engine
                .stage_upload(&workspace.workspace_id, &item, "a", &source, 1)
                .unwrap();
            if missing {
                fs::remove_file(&staged.source_path).unwrap();
            } else {
                corrupt_retained_bytes(&staged.source_path);
            }
            if reopen {
                drop(engine);
                engine = Engine::open(&root).unwrap();
            }
            assert!(
                engine
                    .stage_upload(&workspace.workspace_id, &item, "a", &source, 1)
                    .is_err()
            );
            let retained = &engine.workspace(&workspace.workspace_id).unwrap().files[&item];
            assert_eq!(retained.content_hash, staged.content_hash);
            assert_eq!(retained.dirty_generation, 1);
            if missing {
                assert!(!staged.source_path.exists());
            } else {
                assert_eq!(fs::read(&staged.source_path).unwrap(), b"oops");
            }
        }
    }
}

#[test]
fn same_generation_retry_revalidates_held_target_before_acknowledging() {
    for replace in [false, true] {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("system");
        let mut engine = Engine::initialize(
            &root,
            Config::development([
                temporary.path().join("primary"),
                temporary.path().join("backup"),
                temporary.path().join("tertiary"),
            ]),
        )
        .unwrap();
        let repo = engine
            .create_repository("Retained upload", "P01", 1, "admin", &new_id("op"))
            .unwrap();
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        let item = new_id("entry");
        let source = temporary.path().join("source");
        fs::write(&source, b"data").unwrap();
        let staged = engine
            .stage_upload(&workspace.workspace_id, &item, "a", &source, 1)
            .unwrap();
        let prepared = engine
            .prepare_upload(&workspace.workspace_id, &item, "a", &source, 1)
            .unwrap()
            .execute()
            .unwrap();
        if replace {
            fs::rename(
                &staged.source_path,
                temporary.path().join("held-generation"),
            )
            .unwrap();
            // Even equal replacement bytes are not the inode already verified.
            fs::write(&staged.source_path, b"data").unwrap();
        } else {
            corrupt_retained_bytes(&staged.source_path);
        }
        assert_eq!(
            engine.finish_upload(prepared).unwrap_err().code(),
            storage_contract::ErrorCode::ChecksumMismatch
        );
        assert_eq!(
            engine.workspace(&workspace.workspace_id).unwrap().files[&item].content_hash,
            staged.content_hash
        );
    }
}

#[test]
fn detached_upload_keeps_writer_locks_until_cancelled() {
    for execute in [false, true] {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("system");
        let primary = temporary.path().join("primary");
        let mut engine = Engine::initialize(
            &root,
            Config::development([
                primary.clone(),
                temporary.path().join("backup"),
                temporary.path().join("tertiary"),
            ]),
        )
        .unwrap();
        let repo = engine
            .create_repository("Detached upload", "P01", 1, "admin", &new_id("op"))
            .unwrap();
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        let source = temporary.path().join("source");
        fs::write(&source, b"data").unwrap();
        let preparation = engine
            .prepare_upload(&workspace.workspace_id, &new_id("entry"), "a", &source, 1)
            .unwrap();
        drop(engine);
        if execute {
            let prepared = preparation.execute().unwrap();
            assert_eq!(
                Engine::open(&root)
                    .err()
                    .map(|error| error.code().to_owned()),
                Some(storage_contract::ErrorCode::WriterAlreadyActive),
                "prepared upload still owns scratch bytes and its workspace admission"
            );
            drop(prepared);
        } else {
            assert_eq!(
                Engine::open(&root)
                    .err()
                    .map(|error| error.code().to_owned()),
                Some(storage_contract::ErrorCode::WriterAlreadyActive),
                "detached upload can still write to this pool"
            );
            drop(preparation);
        }
        let reopened = Engine::open(&root).unwrap();
        assert!(
            reopened
                .workspace(&workspace.workspace_id)
                .unwrap()
                .files
                .is_empty()
        );
        let data = primary
            .join("staging/workspaces")
            .join(&workspace.workspace_id);
        assert!(fs::read_dir(data).unwrap().all(|entry| {
            entry
                .unwrap()
                .path()
                .extension()
                .is_none_or(|extension| extension != "prepared" && extension != "full")
        }));
    }
}

#[test]
fn first_upload_revalidates_prepared_bytes_before_acknowledging() {
    for replace in [false, true] {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("system");
        let primary = temporary.path().join("primary");
        let mut engine = Engine::initialize(&root, Config::development([primary.clone()])).unwrap();
        let repo = engine
            .create_repository("Prepared upload", "P01", 1, "admin", &new_id("op"))
            .unwrap();
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        let item = new_id("entry");
        let source = temporary.path().join("source");
        fs::write(&source, b"data").unwrap();
        let prepared = engine
            .prepare_upload(&workspace.workspace_id, &item, "a", &source, 1)
            .unwrap()
            .execute()
            .unwrap();
        let directory = primary
            .join("staging/workspaces")
            .join(&workspace.workspace_id);
        let scratch = fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "prepared")
            })
            .unwrap();
        if replace {
            fs::rename(&scratch, temporary.path().join("held-preparation")).unwrap();
            fs::write(&scratch, b"data").unwrap();
        } else {
            corrupt_retained_bytes(&scratch);
        }
        assert_eq!(
            engine
                .finish_upload(prepared)
                .err()
                .map(|error| error.code().to_owned()),
            Some(storage_contract::ErrorCode::ChecksumMismatch)
        );
        assert!(
            engine
                .workspace(&workspace.workspace_id)
                .unwrap()
                .files
                .is_empty()
        );
        assert!(!directory.join(format!("{item}-1.full")).exists());
        assert!(
            !scratch.exists(),
            "rejected preparation must release scratch quota"
        );
        let retried = engine
            .stage_upload(&workspace.workspace_id, &item, "a", &source, 1)
            .unwrap();
        assert_eq!(fs::read(retried.source_path).unwrap(), b"data");
    }
}

#[test]
fn interrupted_copy_temporaries_consume_workspace_bytes_and_generation_slots() {
    for count_limit in [false, true] {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("system");
        let primary = temporary.path().join("primary");
        let mut config = Config::development([primary.clone()]);
        config.max_file_bytes = 4;
        config.max_workspace_bytes = if count_limit { 8 } else { 4 };
        config.max_entries = if count_limit { 1 } else { 8 };
        let mut engine = Engine::initialize(&root, config).unwrap();
        let repo = engine
            .create_repository("Interrupted upload", "P01", 1, "admin", &new_id("op"))
            .unwrap();
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        let directory = primary
            .join("staging/workspaces")
            .join(&workspace.workspace_id);
        let crash = directory.join(format!(".zxs-full-{}", new_id("req")));
        fs::write(&crash, b"old!").unwrap();
        // Similar names and temporaries outside this workspace are not its
        // retained upload evidence; admission must not recursively charge them.
        let unrelated = directory.join(format!(".zxs-fullish-{}", new_id("req")));
        let outside = primary
            .join("staging")
            .join(format!(".zxs-full-{}", new_id("req")));
        fs::write(&unrelated, b"unrelated data").unwrap();
        fs::write(&outside, b"another workspace's data").unwrap();
        for reopen in [false, true] {
            if reopen {
                drop(engine);
                engine = Engine::open(&root).unwrap();
            }
            assert_eq!(
                engine
                    .stage_bytes(&workspace.workspace_id, "a", b"data")
                    .unwrap_err()
                    .code(),
                storage_contract::ErrorCode::ResourceLimit,
                "interrupted copies consume the {} budget",
                if count_limit { "generation" } else { "byte" },
            );
            assert_eq!(fs::read(&crash).unwrap(), b"old!");
            assert!(
                engine
                    .workspace(&workspace.workspace_id)
                    .unwrap()
                    .files
                    .is_empty()
            );
        }
        // Explicit fixture cleanup releases capacity; engine admission never
        // deletes retained evidence on the user's behalf.
        fs::remove_file(&crash).unwrap();
        let staged = engine
            .stage_bytes(&workspace.workspace_id, "a", b"data")
            .unwrap();
        assert_eq!(fs::read(staged.source_path).unwrap(), b"data");
        assert_eq!(fs::read(unrelated).unwrap(), b"unrelated data");
        assert_eq!(fs::read(outside).unwrap(), b"another workspace's data");
    }
}
