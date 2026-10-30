//! Real HTTP backup browsing uses user prefixes and retained per-slot bodies.
use super::*;

fn listing_path(slot: &str, path: &str) -> String {
    let query = url_encoding::form::Serializer::new(String::new())
        .append_pair("path", path)
        .finish();
    format!("/api/file-manager/disks/{slot}/entries?{query}")
}
async fn listing(app: &Router, token: &str, slot: &str, path: &str) -> Value {
    success(app, token, "GET", &listing_path(slot, path), None).await
}
async fn configure(app: &Router, token: &str, repo: &str, path: &str, b: &str, c: &str) -> Value {
    let route = format!("/api/repositories/{repo}/backup-configuration");
    let query = url_encoding::form::Serializer::new(String::new())
        .append_pair("path", path)
        .finish();
    let old = success(app, token, "GET", &format!("{route}?{query}"), None).await;
    success(app,token,"PUT",&route,Some(json!({"path":path,"target_copies":3,"backup_prefixes":{"B01":b,"C01":c},
        "expected_scope_id":old["scope"]["id"],"expected_generation":old["generation"],"operation_id":new_id("op")}))).await
}
fn read_only_listing(value: &Value) {
    assert_eq!(value["backup"], true);
    assert_eq!(value["writable"], false);
    assert!(value["context"].is_null());
    assert_eq!(value["directory_branches"], json!([]));
    assert_eq!(value["directory_branches_supported"], false);
    assert_eq!(value["default_branch_scope"], false);
    for entry in value["entries"].as_array().unwrap() {
        assert_eq!(entry["read_only"], true);
        assert_eq!(entry["backup"], true);
    }
}

#[tokio::test]
async fn user_backup_prefixes_keep_fixed_paths_and_slot_versions_after_primary_rename_and_delete() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let admin = login(&app, "admin", "integration-test-password").await;
    let first = manager_upload(&app, &admin, "docs/nested/report.txt", b"first stored body").await;
    let first_version = first["result"]["versions"][0]["version_id"]
        .as_str()
        .unwrap();
    manager_upload(
        &app,
        &admin,
        "docs/nested/report.txt",
        b"second stored body",
    )
    .await;
    let primary = listing(&app, &admin, "P01", "docs/nested").await;
    let file = primary["entries"][0]["file_id"].as_str().unwrap();
    let version = primary["entries"][0]["version_id"].as_str().unwrap();
    let repo = primary["context"]["repository_id"].as_str().unwrap();
    configure(&app, &admin, repo, "docs", "WorkBackup", "冷备份/docs").await;
    let configuration = success(
        &app,
        &admin,
        "GET",
        &format!("/api/repositories/{repo}/backup-configuration?path=docs"),
        None,
    )
    .await;
    assert_eq!(
        configuration["backup_routes"],
        json!([["P01", "B01", "C01"]])
    );
    assert_eq!(configuration["backup_slot_ids"], json!(["B01", "C01"]));
    capture_wire("backup-prefix-configuration.json", &configuration);
    for (slot, prefix, leaf) in [
        ("B01", "WorkBackup", "WorkBackup/nested"),
        ("C01", "冷备份", "冷备份/docs/nested"),
    ] {
        let top = listing(&app, &admin, slot, "").await;
        read_only_listing(&top);
        assert_eq!(top["entries"].as_array().unwrap().len(), 1);
        assert_eq!(top["entries"][0]["name"], prefix);
        assert_ne!(
            top["entries"][0]["name"], "P01",
            "a configured prefix is not hidden behind a primary-slot directory"
        );
        let child = listing(&app, &admin, slot, leaf).await;
        read_only_listing(&child);
        assert_eq!(child["entries"][0]["file_id"], file);
        assert_eq!(child["entries"][0]["version_id"], version);
        assert_eq!(
            child["entries"][0]["repository_path"],
            "docs/nested/report.txt"
        );
    }
    // A public commit can explicitly choose one copy for this new version.
    // The scoped policy and existing AVAILABLE v1/v2 placements remain unchanged.
    let workspace = success(
        &app,
        &admin,
        "POST",
        "/api/file-manager/disks/P01/write-sessions",
        Some(json!({"path":"docs/nested/report.txt"})),
    )
    .await;
    let workspace_id = workspace["workspace_id"].as_str().unwrap();
    let item_id = new_id("entry");
    upload(
        &app,
        &admin,
        workspace_id,
        &item_id,
        workspace["relative_path"].as_str().unwrap(),
        b"primary body without B or C placement",
    )
    .await;
    let mut commit = success(
        &app,
        &admin,
        "POST",
        &format!("/api/workspaces/{workspace_id}/seal"),
        Some(json!({})),
    )
    .await;
    assert_eq!(commit["file_placement"][&item_id]["target_copies"], 3);
    commit["file_placement"][&item_id] = json!({"primary_slot_id":"P01","target_copies":1});
    commit["request_digest"] = request_digest(
        "POST",
        "/api/workspaces/{workspace}/commit",
        json!({"workspace":workspace_id}),
        json!({}),
        &commit,
    )
    .unwrap()
    .into();
    success(
        &app,
        &admin,
        "POST",
        &format!("/api/workspaces/{workspace_id}/commit"),
        Some(commit),
    )
    .await;
    assert_eq!(
        success(
            &app,
            &admin,
            "GET",
            &format!("/api/repositories/{repo}/backup-configuration?path=docs"),
            None,
        )
        .await,
        configuration,
        "one version's placement does not mutate scoped backup settings"
    );
    let current = listing(&app, &admin, "P01", "docs/nested").await;
    let current_version = current["entries"][0]["version_id"].as_str().unwrap();
    assert_ne!(current_version, version);
    for (body, copies, slots) in [
        (first_version, 3, json!(["P01", "B01", "C01"])),
        (version, 3, json!(["P01", "B01", "C01"])),
        (current_version, 1, json!(["P01"])),
    ] {
        let placement = success(
            &app,
            &admin,
            "GET",
            &format!("/api/versions/{body}/placement"),
            None,
        )
        .await;
        assert_eq!(placement["target_copies"], copies);
        assert_eq!(placement["route_slot_ids"], slots);
    }
    for (path, destination) in [
        ("docs", "archive"),
        ("archive/nested/report.txt", "archive/nested/renamed.txt"),
    ] {
        manager_namespace(&app, &admin, "rename", path, Some(destination)).await;
    }
    let renamed = listing(&app, &admin, "P01", "archive/nested").await;
    assert_eq!(renamed["entries"][0]["version_id"], current_version);
    assert!(
        renamed["entries"][0]["node_id"]
            .as_str()
            .unwrap()
            .starts_with("fnode_")
    );
    let backup = listing(&app, &admin, "B01", "WorkBackup/nested").await;
    read_only_listing(&backup);
    let entry = &backup["entries"][0];
    assert_eq!(entry["path"], "WorkBackup/nested/report.txt");
    assert_eq!(entry["name"], "report.txt");
    assert_eq!(entry["repository_id"], repo);
    assert_eq!(entry["repository_path"], "archive/nested/renamed.txt");
    assert_eq!(entry["historical_only"], false);
    assert_eq!(entry["version_id"], version);
    assert_eq!(entry["node_id"], version);
    let history=success(&app,&admin,"GET",&format!("/api/file-manager/files/{file}/history?repository_id={repo}&path=archive/nested/renamed.txt"),None).await;
    assert_eq!(history["context"]["content"]["version_id"], current_version);
    let versions = history_versions(
        &app,
        &admin,
        file,
        &history,
        Some((repo, "archive/nested/renamed.txt")),
    )
    .await;
    assert!(
        versions["versions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|body| body["version_id"] == version)
    );
    assert_eq!(
        version_bytes(&app, &admin, version).await,
        b"second stored body"
    );
    assert_eq!(
        version_bytes(&app, &admin, current_version).await,
        b"primary body without B or C placement"
    );
    let cold = listing(&app, &admin, "C01", "冷备份/docs/nested").await;
    assert_eq!(cold["entries"][0]["path"], "冷备份/docs/nested/report.txt");
    assert_eq!(cold["entries"][0]["version_id"], version);
    for (route, body) in [
        (
            "write-sessions",
            json!({"path":"WorkBackup/nested/report.txt"}),
        ),
        (
            "directory-branches",
            json!({"path":"WorkBackup/nested","name":"forbidden","operation_id":new_id("op")}),
        ),
        (
            "namespace-batch",
            manager_namespace_request(&app, &admin, "delete", "archive/nested/renamed.txt", None)
                .await,
        ),
    ] {
        assert_eq!(
            send(
                &app,
                Some(&admin),
                "POST",
                &format!("/api/file-manager/disks/B01/{route}"),
                Some(body)
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    manager_namespace(&app, &admin, "delete", "archive/nested/renamed.txt", None).await;
    let retained = listing(&app, &admin, "B01", "WorkBackup/nested").await;
    read_only_listing(&retained);
    assert_eq!(retained["entries"][0]["historical_only"], true);
    assert!(retained["entries"][0]["repository_path"].is_null());
    assert_eq!(retained["entries"][0]["version_id"], version);
    assert_eq!(retained["entries"][0]["node_id"], version);
    let historical = success(
        &app,
        &admin,
        "GET",
        &format!("/api/file-manager/files/{file}/history"),
        None,
    )
    .await;
    assert!(historical["identity"]["context"]["path"].is_null());
    assert_eq!(
        history_versions(&app, &admin, file, &historical, None).await["versions"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        version_bytes(&app, &admin, version).await,
        b"second stored body"
    );
    capture_wire("backup-prefix-files.json", &backup);
    capture_wire("backup-prefix-retained.json", &retained);
    drop(app);
    let engine = reopen_after_router_drop(&root.path().join("system")).await;
    assert_eq!(
        engine.state().file_backup_paths[file]["B01"],
        "WorkBackup/nested/report.txt"
    );
    assert_eq!(engine.file_history(file).unwrap().len(), 3);
}

#[tokio::test]
async fn backup_path_collisions_are_rejected_for_every_session_without_overwriting_bodies() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let admin = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &admin, "docs/report.txt", b"publicly authorized body").await;
    let primary = listing(&app, &admin, "P01", "docs").await;
    let repo = primary["context"]["repository_id"].as_str().unwrap();
    let public_version = primary["entries"][0]["version_id"].as_str().unwrap();
    configure(&app, &admin, repo, "docs", "WorkBackup/nested", "冷备份").await;
    let private=success(&app,&admin,"POST","/api/repositories",Some(json!({"name":"Private","primary_slot_id":"P01","target_copies":3,"operation_id":new_id("op")}))).await;
    let private_repo = private["repository_id"].as_str().unwrap();
    manager_upload(
        &app,
        &admin,
        "Private/report.txt",
        b"private body must not leak",
    )
    .await;
    let private_entries = listing(&app, &admin, "P01", "Private").await;
    let private_version = private_entries["entries"][0]["version_id"]
        .as_str()
        .unwrap();
    configure(
        &app,
        &admin,
        private_repo,
        "report.txt",
        "workbackup/nested",
        "PrivateOnly",
    )
    .await;
    let (status, conflict) = send(
        &app,
        Some(&admin),
        "GET",
        &listing_path("B01", "WorkBackup/nested"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict["error"]["code"], "BACKUP_PATH_CONFLICT");
    assert_eq!(
        version_bytes(&app, &admin, public_version).await,
        b"publicly authorized body"
    );
    assert_eq!(
        version_bytes(&app, &admin, private_version).await,
        b"private body must not leak"
    );
    // Every authenticated session belongs to the same administrator, so
    // colliding repository projections remain explicit rather than hidden by ACLs.
    let session = login(&app, "admin", "integration-test-password").await;
    let (status, same_conflict) = send(
        &app,
        Some(&session),
        "GET",
        &listing_path("B01", "WorkBackup/nested"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(same_conflict["error"]["code"], "BACKUP_PATH_CONFLICT");
    let cold = listing(&app, &session, "C01", "").await;
    read_only_listing(&cold);
    assert!(
        cold["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "PrivateOnly")
    );
    let other = listing(&app, &session, "C01", "PrivateOnly").await;
    assert_eq!(other["entries"][0]["version_id"], private_version);
    assert_eq!(
        version_bytes(&app, &session, private_version).await,
        b"private body must not leak"
    );
    let revoked = revoked_session(&app).await;
    for token in [None, Some(revoked.as_str())] {
        for path in [
            listing_path("B01", ""),
            listing_path("C01", "PrivateOnly"),
            format!("/api/versions/{private_version}/content"),
        ] {
            assert_eq!(
                send(&app, token, "GET", &path, None).await.0,
                StatusCode::UNAUTHORIZED
            );
        }
    }
    // A file/directory-prefix collision is equally explicit; different stable
    // containers remain independently readable after both collisions.
    configure(
        &app,
        &admin,
        private_repo,
        "report.txt",
        "WorkBackup/nested/report.txt",
        "PrivateOnly",
    )
    .await;
    let (status, conflict) = send(
        &app,
        Some(&admin),
        "GET",
        &listing_path("B01", "WorkBackup/nested"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict["error"]["code"], "BACKUP_PATH_CONFLICT");
    assert_eq!(
        version_bytes(&app, &session, public_version).await,
        b"publicly authorized body"
    );
    assert_eq!(
        version_bytes(&app, &session, private_version).await,
        b"private body must not leak"
    );
    capture_wire("backup-prefix-administrator.json", &cold);
    capture_wire("backup-prefix-conflict.json", &conflict);
}

#[tokio::test]
async fn administrator_discovers_historical_primary_routes_and_all_required_backup_prefixes() {
    let root = tempfile::tempdir().unwrap();
    let mut config = Config::development([
        root.path().join("d1"),
        root.path().join("d2"),
        root.path().join("d3"),
    ]);
    for (slot, level, parent) in [("P02", 1, None), ("B02", 2, Some("P02"))] {
        config.disks.push(storage_contract::DiskConfig {
            slot_id: slot.into(),
            disk_id: new_id("disk"),
            root: root.path().join(slot),
            level,
            upstream_slot_id: parent.map(str::to_owned),
            metadata_member: false,
            binding_generation: 1,
            capacity_bytes: None,
        });
    }
    let app = configured_application(root.path(), config);
    let admin = login(&app, "admin", "integration-test-password").await;
    let repo=success(&app,&admin,"POST","/api/repositories",Some(json!({"name":"Mixed","primary_slot_id":"P01","target_copies":1,"operation_id":new_id("op")}))).await;
    let id = repo["repository_id"].as_str().unwrap();
    manager_upload(&app, &admin, "Mixed/a.txt", b"first primary").await;
    let first = listing(&app, &admin, "P01", "Mixed").await["entries"][0]["version_id"]
        .as_str()
        .unwrap()
        .to_owned();
    success(&app,&admin,"PUT",&format!("/api/primary-placement-rules/{}",new_id("rule")),Some(json!({"repository_id":id,
        "path_prefix":"a.txt","primary_slot_id":"P02","expected_generation":"0","operation_id":new_id("op")}))).await;
    manager_upload(&app, &admin, "Mixed/a.txt", b"second primary").await;
    let second = listing(&app, &admin, "P01", "Mixed").await["entries"][0]["version_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let route = format!("/api/repositories/{id}/backup-configuration");
    let before = success(&app, &admin, "GET", &format!("{route}?path=a.txt"), None).await;
    assert_eq!(
        before["backup_routes"],
        json!([["P01", "B01", "C01"], ["P02", "B02"]])
    );
    assert_eq!(before["backup_slot_ids"], json!(["B01", "B02", "C01"]));
    assert_eq!(before["max_copies"], 2);
    let body = json!({"path":"a.txt","target_copies":2,"backup_prefixes":{"B02":"second"},"expected_scope_id":before["scope"]["id"],
        "expected_generation":"0","operation_id":new_id("op")});
    let (status, error) = send(&app, Some(&admin), "PUT", &route, Some(body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["error"]["code"], "BACKUP_PREFIX_REQUIRED");
    success(&app,&admin,"PUT",&route,Some(json!({"path":"a.txt","target_copies":2,"backup_prefixes":{"B01":"first","B02":"second"},
        "expected_scope_id":before["scope"]["id"],"expected_generation":"0","operation_id":new_id("op")}))).await;
    // Another history on the same primary contributes a distinct C01 route;
    // the sole administrator sees both histories without per-account filtering.
    success(&app,&admin,"POST","/api/repositories",Some(json!({"name":"Hidden","primary_slot_id":"P01","target_copies":3,"operation_id":new_id("op")}))).await;
    manager_upload(&app, &admin, "Hidden/secret.txt", b"not admitted").await;

    let session = login(&app, "admin", "integration-test-password").await;
    let disks = success(&app, &session, "GET", "/api/file-manager/disks", None).await;
    let visible = disks["disks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|disk| disk["slot_id"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        visible,
        ["P01", "P02", "B01", "B02", "C01"].into_iter().collect()
    );
    let old = listing(&app, &session, "B01", "first").await;
    read_only_listing(&old);
    assert_eq!(old["entries"][0]["version_id"], first);
    let new = listing(&app, &session, "B02", "second").await;
    read_only_listing(&new);
    assert_eq!(new["entries"][0]["version_id"], second);
    assert_eq!(old["entries"][0]["repository_path"], "a.txt");
    assert_eq!(new["entries"][0]["repository_path"], "a.txt");
    assert_eq!(
        version_bytes(&app, &session, &first).await,
        b"first primary"
    );
    assert_eq!(
        version_bytes(&app, &session, &second).await,
        b"second primary"
    );
    let actual = success(&app, &session, "GET", &format!("{route}?path=a.txt"), None).await;
    assert_eq!(
        actual["backup_paths"],
        json!({"B01":"first/a.txt","B02":"second/a.txt"})
    );
    success(&app, &session, "POST", "/api/auth/logout", None).await;
    for path in [
        "/api/file-manager/disks".to_owned(),
        listing_path("B01", "first"),
        listing_path("B02", "second"),
    ] {
        assert_eq!(
            send(&app, Some(&session), "GET", &path, None).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    capture_wire("backup-mixed-routes.json", &actual);
    capture_wire("backup-route-disks.json", &disks);
}

#[tokio::test]
async fn primary_and_backup_listings_keep_directory_first_unicode_order() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let admin = login(&app, "admin", "integration-test-password").await;
    // Publish in a deliberately different order from the display order. Each
    // directory contains a file so it is present in the backup projection too.
    for path in [
        "mixed/项目.txt",
        "mixed/z-folder/child.txt",
        "mixed/Éclair.txt",
        "mixed/Étage/child.txt",
        "mixed/Beta.txt",
        "mixed/目录/child.txt",
        "mixed/alpha.txt",
        "mixed/z-last.txt",
        "mixed/A-folder/child.txt",
    ] {
        manager_upload(&app, &admin, path, b"sorting fixture").await;
    }
    let primary = listing(&app, &admin, "P01", "mixed").await;
    let repo = primary["context"]["repository_id"].as_str().unwrap();
    configure(&app, &admin, repo, "mixed", "Sorted", "Sorted").await;
    let expected = json!([
        ["directory", "A-folder"],
        ["directory", "z-folder"],
        ["directory", "Étage"],
        ["directory", "目录"],
        ["file", "alpha.txt"],
        ["file", "Beta.txt"],
        ["file", "z-last.txt"],
        ["file", "Éclair.txt"],
        ["file", "项目.txt"]
    ]);
    for (slot, path) in [("P01", "mixed"), ("B01", "Sorted"), ("C01", "Sorted")] {
        let response = listing(&app, &admin, slot, path).await;
        let order = response["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| json!([entry["kind"], entry["name"]]))
            .collect::<Vec<_>>();
        assert_eq!(json!(order), expected, "{slot}");
        assert_eq!(response["directory_file_count"], "9", "{slot}");
        let child = listing(&app, &admin, slot, &format!("{path}/étage")).await;
        assert_eq!(child["entries"].as_array().unwrap().len(), 1, "{slot}");
        assert_eq!(child["entries"][0]["name"], "child.txt", "{slot}");
    }
    let entry = primary["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "Éclair.txt")
        .unwrap();
    let query = url_encoding::form::Serializer::new(String::new())
        .append_pair("repository_id", entry["repository_id"].as_str().unwrap())
        .append_pair("path", "MIXED/éclair.TXT")
        .finish();
    let file = entry["file_id"].as_str().unwrap();
    let history = success(
        &app,
        &admin,
        "GET",
        &format!("/api/file-manager/files/{file}/history?{query}"),
        None,
    )
    .await;
    assert_eq!(history["identity"]["file_id"], entry["file_id"]);
    assert_eq!(
        history["context"]["content"]["version_id"],
        entry["version_id"]
    );
}
