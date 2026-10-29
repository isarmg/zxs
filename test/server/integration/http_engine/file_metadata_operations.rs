use super::*;

#[tokio::test]
async fn scoped_and_retained_policies_require_the_backup_configuration_endpoint() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "docs/stay.txt", b"stay").await;
    manager_upload(&app, &token, "docs/moved.txt", b"moved").await;
    let listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries?path=docs",
        None,
    )
    .await;
    let repo = listing["context"]["repository_id"].as_str().unwrap();
    let route = format!("/api/repositories/{repo}/backup-configuration");
    let baseline = success(&app, &token, "GET", &format!("{route}?path=docs"), None).await;
    let configured = success(&app, &token, "PUT", &route, Some(json!({
        "path":"docs", "target_copies":2, "backup_prefixes":{"B01":"old"},
        "expected_generation":"0", "expected_scope_id":baseline["scope"]["id"], "operation_id":new_id("op")
    }))).await;
    let current = configured["result"]["policy"]["policy_id"]
        .as_str()
        .unwrap();
    // A metadata-only workspace move updates the selected source and destination
    // views together, so the file is no longer retained inside the source scope.
    let workspace = success(
        &app,
        &token,
        "POST",
        "/api/file-manager/disks/P01/write-sessions",
        Some(json!({"path":"docs/moved.txt"})),
    )
    .await;
    let workspace_id = workspace["workspace_id"].as_str().unwrap();
    success(
        &app,
        &token,
        "POST",
        &format!("/api/workspaces/{workspace_id}/namespace-operations"),
        Some(json!({"kind":"rename","path":"docs/moved.txt","destination":"moved.txt"})),
    )
    .await;
    let mut commit = success(
        &app,
        &token,
        "POST",
        &format!("/api/workspaces/{workspace_id}/seal"),
        Some(json!({})),
    )
    .await;
    assert_eq!(commit["file_placement"], json!({}));
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
        &token,
        "POST",
        &format!("/api/workspaces/{workspace_id}/commit"),
        Some(commit),
    )
    .await;
    let changed = success(&app, &token, "PUT", &route, Some(json!({
        "path":"docs", "target_copies":3, "backup_prefixes":{"B01":"new","C01":"cold"},
        "expected_generation":"1", "expected_scope_id":baseline["scope"]["id"], "operation_id":new_id("op")
    }))).await;
    assert_eq!(changed["result"]["policy"]["policy_id"], current);
    assert_eq!(changed["result"]["file_count"], "1");
    let moved_route = format!("{route}?path=moved.txt");
    let moved = success(&app, &token, "GET", &moved_route, None).await;
    assert_eq!(moved["target_copies"], 2);
    assert_eq!(moved["inherited"], false);
    assert_eq!(moved["backup_paths"]["B01"], "old/moved.txt");
    let retained = moved["effective_policy_id"].as_str().unwrap();
    assert_ne!(retained, current);
    for (id, inheritable) in [(current, true), (retained, false)] {
        let policy_route = format!("/api/policies/{id}");
        let policy = success(&app, &token, "GET", &policy_route, None).await;
        assert_eq!(policy["inheritable"], inheritable);
        let (status, rejected) = send(
            &app,
            Some(&token),
            "PUT",
            &policy_route,
            Some(json!({
                "repository_id":repo, "path_prefix":policy["path_prefix"], "target_copies":1,
                "expected_generation":policy["generation"], "operation_id":new_id("op")
            })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(rejected["error"]["code"], "BACKUP_CONFIGURATION_REQUIRED");
        assert_eq!(
            success(&app, &token, "GET", &policy_route, None).await,
            policy
        );
    }
    assert_eq!(
        success(&app, &token, "GET", &moved_route, None).await,
        moved
    );
    // Ordinary public saves must use the retained file binding, not the current
    // source rule or the repository fallback supplied at seal time.
    let saved = manager_upload(&app, &token, "moved.txt", b"moved update").await;
    let version = saved["result"]["versions"][0]["version_id"]
        .as_str()
        .unwrap();
    let placement = success(
        &app,
        &token,
        "GET",
        &format!("/api/versions/{version}/placement"),
        None,
    )
    .await;
    assert_eq!(placement["target_copies"], 2);
    assert_eq!(placement["route_slot_ids"], json!(["P01", "B01"]));
    assert_eq!(
        success(&app, &token, "GET", &moved_route, None).await,
        moved
    );
}

async fn branch(app: &Router, token: &str, file: &str, version: &str, name: &str) -> String {
    success(app,token,"POST",&format!("/api/file-manager/files/{file}/branches"),Some(json!({"source_version_id":version,"source_node_id":version,"name":name,"operation_id":new_id("op")}))).await["result"]["branch_id"].as_str().unwrap().into()
}
async fn select(app: &Router, token: &str, file: &str, branch: &str) {
    success(
        app,
        token,
        "POST",
        &format!("/api/file-manager/files/{file}/select"),
        Some(json!({"branch_id":branch,"operation_id":new_id("op")})),
    )
    .await;
}
/// The bounded node page plus its branch and merge pages, so the regressions
/// below still compare the whole visible history of their small fixtures.
async fn history(app: &Router, token: &str, file: &str) -> Value {
    history_page(app, token, file).await
}

#[tokio::test]
async fn file_selection_replays_after_its_branch_is_merged_and_path_is_renamed() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "report.txt", b"base body").await;
    let listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    let file = listing["entries"][0]["file_id"].as_str().unwrap();
    let base = listing["entries"][0]["version_id"].as_str().unwrap();
    let repo = listing["context"]["repository_id"].as_str().unwrap();
    let left = branch(&app, &token, file, base, "left").await;
    let right = branch(&app, &token, file, base, "right").await;
    let route = format!("/api/file-manager/files/{file}/select");
    let unchanged = history(&app, &token, file).await;
    for body in [
        json!({"branch_id":right,"repository_id":repo,"operation_id":new_id("op")}),
        json!({"branch_id":right,"path":"report.txt","operation_id":new_id("op")}),
        json!({"branch_id":right,"repository_id":null,"path":null,"operation_id":new_id("op")}),
        json!({"branch_id":right,"repository_id":repo,"path":"","operation_id":new_id("op")}),
        json!({"branch_id":right,"repository_id":repo,"path":"../report.txt","operation_id":new_id("op")}),
        json!({"branch_id":right,"repository_id":repo,"path":"Cafe\u{301}","operation_id":new_id("op")}),
        json!({"branch_id":right,"operation_id":"bad"}),
        json!({"branch_id":right,"operation_id":new_id("op"),"file_id":"other"}),
        json!({"branch_id":right,"operation_id":new_id("op"),"unknown":0}),
    ] {
        assert_eq!(
            send(&app, Some(&token), "POST", &route, Some(body)).await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(history(&app, &token, file).await, unchanged);
    }
    assert_eq!(
        send(&app, None, "POST", &route, Some(json!({"path":null})))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let mut selections = Vec::new();
    for scoped in [false, true] {
        let mut request = json!({"branch_id":right,"operation_id":new_id("op")});
        if scoped {
            request["repository_id"] = repo.into();
            request["path"] = "report.txt".into();
        }
        let original = success(&app, &token, "POST", &route, Some(request.clone())).await;
        let mut canonical = json!({"file_id":file,"branch_id":right});
        if scoped {
            canonical["repository_id"] = repo.into();
            canonical["path"] = "report.txt".into();
        }
        assert_eq!(
            original["request_digest"],
            storage_contract::object_hash(
                "request",
                &json!({"kind":"file.branch.select","principal":"admin","body":canonical})
            )
            .unwrap()
        );
        let mut changed_context = request.clone();
        if scoped {
            changed_context
                .as_object_mut()
                .unwrap()
                .remove("repository_id");
            changed_context.as_object_mut().unwrap().remove("path");
        } else {
            changed_context["repository_id"] = repo.into();
            changed_context["path"] = "report.txt".into();
        }
        let (status, error) = send(&app, Some(&token), "POST", &route, Some(changed_context)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(error["error"]["code"], "IDEMPOTENCY_KEY_REUSED");
        selections.push((request, original));
    }
    success(
        &app,
        &token,
        "POST",
        &format!("/api/file-manager/files/{file}/merge"),
        Some(json!({
            "inputs":[{"branch_id":left,"expected_node_id":base},{"branch_id":right,"expected_node_id":base}],
            "chosen_node_id":base,"surviving_branch_id":left,"confirmed":true,"operation_id":new_id("op")
        })),
    )
    .await;
    manager_namespace(&app, &token, "rename", "report.txt", Some("renamed.txt")).await;
    drop(app);
    let system = root.path().join("system");
    let backend = Arc::new(EngineBackend::new(reopen_after_router_drop(&system).await));
    let auth = backend.open_auth(&system).unwrap();
    let app = storage_server::router(backend, auth, ServerConfig::loopback(&system)).unwrap();
    let token = login(&app, "admin", "integration-test-password").await;
    let revoked = revoked_session(&app).await;
    let before = history(&app, &token, file).await;
    for (request, original) in selections {
        assert_eq!(
            send(&app, Some(&revoked), "POST", &route, Some(request.clone()))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        let replay = success(&app, &token, "POST", &route, Some(request.clone())).await;
        for field in [
            "operation_id",
            "request_digest",
            "publication_sequence",
            "result",
        ] {
            assert_eq!(replay[field], original[field], "{field}");
        }
        let mut changed = request.clone();
        changed["branch_id"] = left.clone().into();
        let (status, response) = send(&app, Some(&token), "POST", &route, Some(changed)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(response["error"]["code"], "IDEMPOTENCY_KEY_REUSED");
        let mut fresh = request;
        fresh["operation_id"] = new_id("op").into();
        if fresh.get("path").is_some() {
            fresh["path"] = "renamed.txt".into();
        }
        let (status, response) = send(&app, Some(&token), "POST", &route, Some(fresh)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(response["error"]["code"], "BRANCH_NOT_ACTIVE");
    }
    assert_eq!(history(&app, &token, file).await, before);
}

#[tokio::test]
async fn metadata_mutation_replay_binds_location_after_rename_and_reopen() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "report.txt", b"base body").await;
    let listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    let file = listing["entries"][0]["file_id"].as_str().unwrap();
    let base = listing["entries"][0]["version_id"].as_str().unwrap();
    let repo = listing["context"]["repository_id"].as_str().unwrap();
    let left = branch(&app, &token, file, base, "left").await;
    let right = branch(&app, &token, file, base, "right").await;
    let tag_route = format!("/api/file-manager/files/{file}/tags");
    let tag_request = json!({
        "targets":[{"branch_id":left,"source_node_id":base,"expected_node_id":base}],
        "add":["reviewed"],"remove":[],"repository_id":repo,"path":"report.txt","operation_id":new_id("op")
    });
    let tagged = success(&app, &token, "POST", &tag_route, Some(tag_request.clone())).await;
    let merge_route = format!("/api/file-manager/files/{file}/merge");
    let merge_request = json!({
        "inputs":[{"branch_id":left,"expected_node_id":tagged["result"]["nodes"][0]["node_id"]},{"branch_id":right,"expected_node_id":base}],
        "chosen_node_id":base,"surviving_branch_id":left,"confirmed":true,
        "repository_id":repo,"path":"report.txt","operation_id":new_id("op")
    });
    let merged = success(
        &app,
        &token,
        "POST",
        &merge_route,
        Some(merge_request.clone()),
    )
    .await;
    manager_namespace(&app, &token, "rename", "report.txt", Some("renamed.txt")).await;
    drop(app);
    let system = root.path().join("system");
    let backend = Arc::new(EngineBackend::new(reopen_after_router_drop(&system).await));
    let auth = backend.open_auth(&system).unwrap();
    let app = storage_server::router(backend, auth, ServerConfig::loopback(&system)).unwrap();
    let token = login(&app, "admin", "integration-test-password").await;
    let before = history(&app, &token, file).await;
    let events = success(&app, &token, "GET", "/api/events", None).await;
    for (route, request, original) in [
        (tag_route, tag_request, tagged),
        (merge_route, merge_request, merged),
    ] {
        let operation = request["operation_id"].as_str().unwrap();
        let queried = success(
            &app,
            &token,
            "GET",
            &format!("/api/operations/{operation}"),
            None,
        )
        .await;
        let replay = success(&app, &token, "POST", &route, Some(request.clone())).await;
        for field in [
            "operation_id",
            "request_digest",
            "publication_sequence",
            "result",
        ] {
            assert_eq!(queried[field], original[field], "{field}");
            assert_eq!(replay[field], original[field], "{field}");
        }
        let mut path_changed = request.clone();
        path_changed["path"] = "renamed.txt".into();
        let mut repository_changed = request.clone();
        repository_changed["repository_id"] = new_id("repo").into();
        let mut context_removed = request.clone();
        context_removed
            .as_object_mut()
            .unwrap()
            .remove("repository_id");
        context_removed.as_object_mut().unwrap().remove("path");
        let mut payload_changed = request.clone();
        if payload_changed.get("add").is_some() {
            payload_changed["add"] = json!(["different"]);
        } else {
            payload_changed["surviving_branch_id"] = right.clone().into();
        }
        for changed in [
            path_changed,
            repository_changed,
            context_removed,
            payload_changed,
        ] {
            let (status, response) = send(&app, Some(&token), "POST", &route, Some(changed)).await;
            assert_eq!(status, StatusCode::CONFLICT, "{response}");
            assert_eq!(response["error"]["code"], "IDEMPOTENCY_KEY_REUSED");
        }
        let mut unpaired = request.clone();
        unpaired.as_object_mut().unwrap().remove("path");
        let mut null_path = request.clone();
        null_path["path"] = Value::Null;
        for malformed in [unpaired, null_path] {
            assert_eq!(
                send(&app, Some(&token), "POST", &route, Some(malformed))
                    .await
                    .0,
                StatusCode::BAD_REQUEST
            );
        }
        if request.get("confirmed").is_some() {
            let mut unconfirmed = request.clone();
            unconfirmed["confirmed"] = false.into();
            assert_eq!(
                send(&app, Some(&token), "POST", &route, Some(unconfirmed))
                    .await
                    .0,
                StatusCode::BAD_REQUEST
            );
        }
        let mut fresh = request;
        fresh["operation_id"] = new_id("op").into();
        let (status, response) = send(&app, Some(&token), "POST", &route, Some(fresh)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(response["error"]["code"], "FILE_NOT_FOUND");
    }
    assert_eq!(history(&app, &token, file).await, before);
    assert_eq!(
        success(&app, &token, "GET", "/api/events", None).await,
        events
    );
}

#[tokio::test]
async fn tags_and_file_merges_publish_reviewed_node_graphs_without_new_bodies() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "report.txt", b"base body").await;
    let listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    let file = listing["entries"][0]["file_id"].as_str().unwrap();
    let base = listing["entries"][0]["version_id"].as_str().unwrap();
    let repo = listing["context"]["repository_id"].as_str().unwrap();
    let left = branch(&app, &token, file, base, "left").await;
    let right = branch(&app, &token, file, base, "right").await;
    select(&app, &token, file, &left).await;
    let tag_route = format!("/api/file-manager/files/{file}/tags");
    let tagged=success(&app,&token,"POST",&tag_route,Some(json!({"targets":[{"branch_id":left,"source_node_id":base,"expected_node_id":base}],"add":["已审核","work"],"remove":[],"operation_id":new_id("op")}))).await;
    let tag_node = tagged["result"]["nodes"][0]["node_id"].as_str().unwrap();
    assert!(tag_node.starts_with("fmeta_"));
    let initial = history(&app, &token, file).await;
    assert_eq!(content_nodes(&initial).len(), 1);
    assert_eq!(initial["context"]["current_node_id"], tag_node);
    let snapshot = initial["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["node_id"] == tag_node)
        .unwrap();
    assert_eq!(snapshot["kind"], "tags");
    assert_eq!(snapshot["tags"], json!(["work", "已审核"]));
    assert_eq!(
        snapshot["parents"]["edges"],
        json!([{"node_id":base,"in_window":true}])
    );
    assert_eq!(snapshot["parents"]["status"], "complete");
    success(
        &app,
        &token,
        "PATCH",
        &format!("/api/file-manager/files/{file}/nodes/{tag_node}"),
        Some(json!({"note":"独立标签备注","operation_id":new_id("op")})),
    )
    .await;
    manager_namespace(&app, &token, "rename", "report.txt", Some("renamed.txt")).await;
    let renamed = history(&app, &token, file).await;
    let path_node = renamed["context"]["current_node_id"].as_str().unwrap();
    manager_upload(&app, &token, "renamed.txt", b"left body").await;
    let written = history(&app, &token, file).await;
    let left_node = written["context"]["current_node_id"].as_str().unwrap();
    assert_eq!(written["nodes"][0]["tags"], json!(["work", "已审核"]));
    assert_eq!(
        written["nodes"][0]["parents"]["edges"],
        json!([{"node_id":path_node,"in_window":true}])
    );
    let historical=success(&app,&token,"POST",&tag_route,Some(json!({"targets":[{"branch_id":left,"source_node_id":tag_node,"expected_node_id":left_node}],"add":["归档"],"remove":["work"],"operation_id":new_id("op")}))).await;
    assert_eq!(historical["result"]["nodes"][0]["advanced"], false);
    assert_eq!(
        history(&app, &token, file).await["context"]["current_node_id"],
        left_node
    );
    let before_invalid = history(&app, &token, file).await;
    for bad in [" bad".to_string(), "bad\nlabel".into(), "文".repeat(43)] {
        assert_eq!(send(&app,Some(&token),"POST",&tag_route,Some(json!({"targets":[{"branch_id":left,"source_node_id":left_node,"expected_node_id":left_node}],"add":[bad],"remove":[],"operation_id":new_id("op")}))).await.0,StatusCode::BAD_REQUEST);
    }
    assert_eq!(history(&app, &token, file).await, before_invalid);
    select(&app, &token, file, &right).await;
    manager_upload(&app, &token, "renamed.txt", b"right body").await;
    let right_history = history(&app, &token, file).await;
    let right_node = right_history["context"]["current_node_id"]
        .as_str()
        .unwrap();
    let merge_route = format!("/api/file-manager/files/{file}/merge");
    let request = json!({"inputs":[{"branch_id":left,"expected_node_id":left_node},{"branch_id":right,"expected_node_id":right_node}],"chosen_node_id":tag_node,"surviving_branch_id":left,"confirmed":true,"repository_id":repo,"path":"renamed.txt","operation_id":new_id("op")});
    let mut unconfirmed = request.clone();
    unconfirmed["confirmed"] = false.into();
    assert_eq!(
        send(&app, Some(&token), "POST", &merge_route, Some(unconfirmed))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let mut stale = request.clone();
    stale["inputs"][1]["expected_node_id"] = base.into();
    assert_eq!(
        send(&app, Some(&token), "POST", &merge_route, Some(stale))
            .await
            .0,
        StatusCode::CONFLICT
    );
    let merged = success(&app, &token, "POST", &merge_route, Some(request.clone())).await;
    assert_eq!(
        success(&app, &token, "POST", &merge_route, Some(request)).await["result"],
        merged["result"]
    );
    let node = merged["result"]["node_id"].as_str().unwrap();
    assert!(node.starts_with("fmeta_"));
    let after = history(&app, &token, file).await;
    assert_eq!(content_nodes(&after).len(), 3);
    assert_eq!(after["context"]["current_node_id"], node);
    assert_eq!(after["context"]["version_id"], base);
    let result = after["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["node_id"] == node)
        .unwrap();
    assert_eq!(result["kind"], "merge");
    assert_eq!(
        result["path"], "report.txt",
        "merge metadata describes its chosen source, independent of parent UUID order"
    );
    assert_eq!(result["tags"], json!(["work", "已审核"]));
    let alias = after["branches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|branch| branch["branch_id"] == right)
        .unwrap();
    assert_eq!(alias["read_only"], true);
    assert_eq!(alias["alias_of"], left);
    assert_eq!(alias["node_id"], right_node);
    assert!(alias.get("pending_file_versions").is_none());
    assert_eq!(after["merges"].as_array().unwrap().len(), 1);
    let merge = &after["merges"][0];
    assert_eq!(merge["result_node_id"], node);
    assert_eq!(merge["chosen_node_id"], tag_node);
    assert_eq!(merge["reused_node"], false);
    assert_eq!(merge["input_count"], 2);
    assert_eq!(merge["parent_count"], 3);
    let merge_id = merge["merge_id"].as_str().unwrap();
    let history_root = after["history_root"].as_str().unwrap();
    let inputs = success(&app, &token, "GET", &format!("/api/file-manager/files/{file}/history/merges/{merge_id}/inputs?history_root={history_root}&limit=1"), None).await;
    assert_eq!(inputs["total"], 2);
    assert_eq!(
        inputs["inputs"],
        json!([{"index":0,"branch_id":left,"node_id":left_node}])
    );
    assert_eq!(inputs["next_offset"], 1);
    let rest = success(&app, &token, "GET", &format!("/api/file-manager/files/{file}/history/merges/{merge_id}/inputs?history_root={history_root}&offset=1"), None).await;
    assert_eq!(
        rest["inputs"],
        json!([{"index":1,"branch_id":right,"node_id":right_node}])
    );
    assert!(rest["next_offset"].is_null());
    let stored_parents = success(&app, &token, "GET", &format!("/api/file-manager/files/{file}/history/merges/{merge_id}/parents?history_root={history_root}"), None).await;
    assert_eq!(stored_parents["total"], 3);
    assert_eq!(
        node_parents(&app, &token, &after, node)
            .await
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(version_bytes(&app, &token, base).await, b"base body");
    assert_eq!(
        send(
            &app,
            Some(&token),
            "POST",
            &format!("/api/file-manager/files/{file}/select"),
            Some(json!({"branch_id":right,"operation_id":new_id("op")}))
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        send(
            &app,
            Some(&token),
            "PATCH",
            &format!("/api/file-manager/files/{file}/branches/{right}"),
            Some(json!({"name":"forbidden label","operation_id":new_id("op")}))
        )
        .await
        .0,
        StatusCode::CONFLICT
    );

    let session = login(&app, "admin", "integration-test-password").await;
    assert_eq!(
        history(&app, &session, file).await["context"]["current_node_id"],
        node
    );
    success(&app, &session, "POST", "/api/auth/logout", None).await;
    assert_eq!(
        send(
            &app,
            Some(&session),
            "POST",
            &tag_route,
            Some(json!({"targets":[],"add":[],"remove":[],"operation_id":new_id("op")}))
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let audit = success(&app, &token, "GET", "/api/file-manager/history", None).await;
    for action in ["file.branches.merge", "file.tags.update"] {
        assert!(
            audit["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|event| event["action"] == action)
        );
    }
    // Reviewed contract fixtures are the exact wire pages, never the
    // assembled regression view built by `history_page`.
    let wire_root = after["history_root"].as_str().unwrap();
    for (name, route) in [
        (
            "file-merge-tag-history.json",
            format!("/api/file-manager/files/{file}/history?limit=200"),
        ),
        (
            "file-merge-tag-history-branches.json",
            format!("/api/file-manager/files/{file}/history/branches?history_root={wire_root}"),
        ),
        (
            "file-merge-tag-history-merges.json",
            format!("/api/file-manager/files/{file}/history/merges?history_root={wire_root}"),
        ),
        (
            "file-merge-tag-history-merge-inputs.json",
            format!(
                "/api/file-manager/files/{file}/history/merges/{merge_id}/inputs?history_root={wire_root}"
            ),
        ),
    ] {
        capture_wire(name, &success(&app, &token, "GET", &route, None).await);
    }
    drop(app);
    let engine = reopen_after_router_drop(&root.path().join("system")).await;
    assert_eq!(engine.file_branch_head(&left).unwrap(), node);
}

#[tokio::test]
async fn scoped_backup_configuration_checks_scope_identity_generation_and_authentication() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "docs/nested/report.txt", b"first body").await;
    manager_upload(&app, &token, "docs/nested/report.txt", b"second body").await;
    let listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries?path=docs/nested",
        None,
    )
    .await;
    let file = listing["entries"][0]["file_id"].as_str().unwrap();
    let repo = listing["context"]["repository_id"].as_str().unwrap();
    let route = format!("/api/repositories/{repo}/backup-configuration");
    let before = success(&app, &token, "GET", &format!("{route}?path=docs"), None).await;
    assert!(before.get("can_manage").is_none());
    assert_eq!(before["generation"], "0");
    let request = json!({"path":"docs","target_copies":3,"backup_prefixes":{"B01":"工作备份","C01":"archive/docs"},"expected_generation":"0","expected_scope_id":before["scope"]["id"],"operation_id":new_id("op")});
    let configured = success(&app, &token, "PUT", &route, Some(request.clone())).await;
    assert_eq!(
        success(&app, &token, "PUT", &route, Some(request.clone())).await["result"],
        configured["result"]
    );
    let after = success(&app, &token, "GET", &format!("{route}?path=docs"), None).await;
    assert_eq!(after["generation"], "1");
    assert_eq!(after["target_copies"], 3);
    let inherited = success(
        &app,
        &token,
        "GET",
        &format!("{route}?path=docs/nested/report.txt"),
        None,
    )
    .await;
    assert_eq!(inherited["generation"], "0");
    assert_eq!(inherited["effective_generation"], "1");
    assert_eq!(inherited["inherited"], true);
    assert_eq!(
        inherited["backup_paths"]["B01"],
        "工作备份/nested/report.txt"
    );
    assert_eq!(
        inherited["backup_paths"]["C01"],
        "archive/docs/nested/report.txt"
    );
    let mut stale = request.clone();
    stale["operation_id"] = new_id("op").into();
    assert_eq!(
        send(&app, Some(&token), "PUT", &route, Some(stale)).await.0,
        StatusCode::CONFLICT
    );
    let mut wrong = request.clone();
    wrong["operation_id"] = new_id("op").into();
    wrong["expected_scope_id"] = new_id("entry").into();
    wrong["expected_generation"] = "1".into();
    assert_eq!(
        send(&app, Some(&token), "PUT", &route, Some(wrong)).await.0,
        StatusCode::CONFLICT
    );
    for prefixes in [
        json!({"B01":"../bad","C01":"archive"}),
        json!({"B01":"missing C"}),
    ] {
        assert_eq!(send(&app,Some(&token),"PUT",&route,Some(json!({"path":"docs","target_copies":3,"backup_prefixes":prefixes,"expected_generation":"1","expected_scope_id":before["scope"]["id"],"operation_id":new_id("op")}))).await.0,StatusCode::BAD_REQUEST);
    }
    assert_eq!(
        success(&app, &token, "GET", &format!("{route}?path=docs"), None).await,
        after
    );
    manager_upload(&app, &token, "docs/new.txt", b"new body inherits prefixes").await;
    let new_file = success(
        &app,
        &token,
        "GET",
        &format!("{route}?path=docs/new.txt"),
        None,
    )
    .await;
    assert_eq!(new_file["backup_paths"]["B01"], "工作备份/new.txt");

    let session = login(&app, "admin", "integration-test-password").await;
    assert_eq!(
        success(&app, &session, "GET", &format!("{route}?path=docs"), None).await,
        success(&app, &token, "GET", &format!("{route}?path=docs"), None).await
    );
    success(&app, &session, "POST", "/api/auth/logout", None).await;
    assert_eq!(
        send(&app, Some(&session), "PUT", &route, Some(request))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    capture_wire("scoped-backup-configuration.json", &inherited);
    drop(app);
    let engine = reopen_after_router_drop(&root.path().join("system")).await;
    assert_eq!(engine.file_history(file).unwrap().len(), 2);
    assert_eq!(
        engine.state().file_backup_paths[file]["B01"],
        "工作备份/nested/report.txt"
    );
}

#[tokio::test]
async fn contextual_branch_selection_preserves_current_file_when_target_body_is_lost() {
    let root = tempfile::tempdir().unwrap();
    let system = root.path().join("system");
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "report.txt", b"original body").await;
    let listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    let file = listing["entries"][0]["file_id"].as_str().unwrap();
    let version = listing["entries"][0]["version_id"].as_str().unwrap();
    let repo = listing["context"]["repository_id"].as_str().unwrap();
    let original_branch = listing["entries"][0]["selected_branch_id"]
        .as_str()
        .unwrap();
    let other = branch(&app, &token, file, version, "other").await;
    select(&app, &token, file, &other).await;
    manager_upload(&app, &token, "report.txt", b"still readable body").await;
    let before = history(&app, &token, file).await;
    drop(app);
    let engine = reopen_after_router_drop(&system).await;
    let history_id = &engine.state().versions[version].history_tree_id;
    for disk in &engine.config().disks {
        let directory = disk
            .root
            .join("histories")
            .join(history_id)
            .join("versions")
            .join(version);
        if directory.exists() {
            std::fs::remove_file(fixture_payload(&directory, engine.state())).unwrap();
        }
    }
    let backend = Arc::new(EngineBackend::new(engine));
    let auth = backend.open_auth(&system).unwrap();
    let app = storage_server::router(backend, auth, ServerConfig::loopback(&system)).unwrap();
    let token = login(&app, "admin", "integration-test-password").await;
    let route = format!("/api/file-manager/files/{file}/select");
    for contextual in [false, true] {
        let mut request = json!({"branch_id":original_branch,"operation_id":new_id("op")});
        if contextual {
            request["repository_id"] = repo.into();
            request["path"] = "report.txt".into();
        }
        let (status, response) = send(&app, Some(&token), "POST", &route, Some(request)).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{response}");
        assert_eq!(response["error"]["code"], "CONTENT_UNAVAILABLE");
    }
    let after = history(&app, &token, file).await;
    assert_eq!(after["selected_branch_id"], before["selected_branch_id"]);
    assert_eq!(after["current_node_id"], before["current_node_id"]);
    assert_eq!(after["versions"], before["versions"]);
}

#[tokio::test]
async fn file_merge_requires_explicit_consent_and_preserves_both_location_digests() {
    for scoped in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let app = application(root.path());
        let token = login(&app, "admin", "integration-test-password").await;
        manager_upload(&app, &token, "report.txt", b"chosen body").await;
        let listing = success(
            &app,
            &token,
            "GET",
            "/api/file-manager/disks/P01/entries",
            None,
        )
        .await;
        let file = listing["entries"][0]["file_id"].as_str().unwrap();
        let base = listing["entries"][0]["version_id"].as_str().unwrap();
        let repo = listing["context"]["repository_id"].as_str().unwrap();
        let left = branch(&app, &token, file, base, "left").await;
        let right = branch(&app, &token, file, base, "right").await;
        let route = format!("/api/file-manager/files/{file}/merge");
        let inputs = json!([{"branch_id":left,"expected_node_id":base},{"branch_id":right,"expected_node_id":base}]);
        let mut request = json!({"inputs":inputs,"chosen_node_id":base,"surviving_branch_id":left,"confirmed":true,"operation_id":new_id("op")});
        let location = if scoped {
            request["repository_id"] = repo.into();
            request["path"] = "report.txt".into();
            json!({"repository_id":repo,"path":"report.txt"})
        } else {
            Value::Null
        };
        let before = history(&app, &token, file).await;
        for (key, value, code) in [
            ("inputs", None, "INVALID_REQUEST"),
            ("inputs", Some(Value::Null), "INVALID_REQUEST"),
            (
                "inputs",
                Some(json!([[left, base], [right, base]])),
                "INVALID_REQUEST",
            ),
            ("operation_id", Some(json!("bad")), "INVALID_ID"),
            ("file_id", Some(json!("other")), "INVALID_REQUEST"),
            ("unknown", Some(json!(0)), "INVALID_REQUEST"),
        ] {
            let mut invalid = request.clone();
            match value {
                Some(value) => {
                    invalid[key] = value;
                }
                None => {
                    invalid.as_object_mut().unwrap().remove(key);
                }
            }
            let (status, rejected) = send(&app, Some(&token), "POST", &route, Some(invalid)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{rejected}");
            assert_eq!(rejected["error"]["code"], code);
            assert_eq!(history(&app, &token, file).await, before);
        }
        for published in [false, true] {
            for consent in [
                None,
                Some(Value::Null),
                Some(json!(false)),
                Some(json!("true")),
                Some(json!(1)),
                Some(json!({})),
            ] {
                let mut invalid = request.clone();
                match consent {
                    Some(value) => {
                        invalid["confirmed"] = value;
                    }
                    None => {
                        invalid.as_object_mut().unwrap().remove("confirmed");
                    }
                }
                let snapshot = history(&app, &token, file).await;
                let (status, rejected) =
                    send(&app, Some(&token), "POST", &route, Some(invalid)).await;
                assert_eq!(status, StatusCode::BAD_REQUEST, "{rejected}");
                assert_eq!(rejected["error"]["code"], "INVALID_REQUEST");
                assert_eq!(history(&app, &token, file).await, snapshot);
            }
            if !published {
                let receipt = success(&app, &token, "POST", &route, Some(request.clone())).await;
                assert_eq!(receipt["request_digest"],storage_contract::object_hash("request",&json!({"kind":"file.branches.merge","principal":"admin","body":{"file_id":file,"inputs":inputs,"chosen_node_id":base,"surviving_branch_id":left,"location":location}})).unwrap());
            }
        }
        let revoked = revoked_session(&app).await;
        assert_eq!(
            send(&app, Some(&revoked), "POST", &route, Some(request.clone()))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        let mut changed = request.clone();
        if scoped {
            changed.as_object_mut().unwrap().remove("repository_id");
            changed.as_object_mut().unwrap().remove("path");
        } else {
            changed["repository_id"] = repo.into();
            changed["path"] = "report.txt".into();
        }
        let (status, rejected) = send(&app, Some(&token), "POST", &route, Some(changed)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(rejected["error"]["code"], "IDEMPOTENCY_KEY_REUSED");
        let after = history(&app, &token, file).await;
        drop(app);
        let system = root.path().join("system");
        let backend = Arc::new(EngineBackend::new(reopen_after_router_drop(&system).await));
        let auth = backend.open_auth(&system).unwrap();
        let app = storage_server::router(backend, auth, ServerConfig::loopback(&system)).unwrap();
        let token = login(&app, "admin", "integration-test-password").await;
        success(&app, &token, "POST", &route, Some(request)).await;
        assert_eq!(history(&app, &token, file).await, after);
    }
}

#[tokio::test]
async fn file_tag_batches_keep_static_policy_atomic_preflight_and_permanent_replay() {
    for scoped in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let app = application(root.path());
        let token = login(&app, "admin", "integration-test-password").await;
        manager_upload(&app, &token, "report.txt", b"tag source").await;
        let listing = success(
            &app,
            &token,
            "GET",
            "/api/file-manager/disks/P01/entries",
            None,
        )
        .await;
        let file = listing["entries"][0]["file_id"].as_str().unwrap();
        let base = listing["entries"][0]["version_id"].as_str().unwrap();
        let repo = listing["context"]["repository_id"].as_str().unwrap();
        let left = branch(&app, &token, file, base, "left").await;
        let right = branch(&app, &token, file, base, "right").await;
        let route = format!("/api/file-manager/files/{file}/tags");
        let targets = json!([{"branch_id":left,"source_node_id":base,"expected_node_id":base},{"branch_id":right,"source_node_id":base,"expected_node_id":base}]);
        let mut request =
            json!({"targets":targets,"add":["x"],"remove":[],"operation_id":new_id("op")});
        let location = if scoped {
            request["repository_id"] = repo.into();
            request["path"] = "report.txt".into();
            json!({"repository_id":repo,"path":"report.txt"})
        } else {
            Value::Null
        };
        let before = history(&app, &token, file).await;
        for (key, value, code) in [
            ("targets", None, "INVALID_REQUEST"),
            ("targets", Some(Value::Null), "INVALID_REQUEST"),
            ("targets", Some(json!([])), "INVALID_REQUEST"),
            (
                "targets",
                Some(json!([targets[0], targets[0]])),
                "INVALID_REQUEST",
            ),
            (
                "targets",
                Some(
                    json!([{"branch_id":left,"source_node_id":base,"expected_node_id":base,"unknown":true}]),
                ),
                "INVALID_REQUEST",
            ),
            (
                "targets",
                Some(json!([[left, base, base]])),
                "INVALID_REQUEST",
            ),
            ("add", None, "INVALID_REQUEST"),
            ("add", Some(Value::Null), "INVALID_REQUEST"),
            ("add", Some(json!([1])), "INVALID_REQUEST"),
            ("add", Some(json!("x")), "INVALID_REQUEST"),
            ("remove", None, "INVALID_REQUEST"),
            ("remove", Some(Value::Null), "INVALID_REQUEST"),
            (
                "remove",
                Some(json!(vec!["removed"; 17])),
                "INVALID_REQUEST",
            ),
            ("add", Some(json!([""])), "INVALID_TAG"),
            ("path", Some(Value::Null), "INVALID_REQUEST"),
            ("add", Some(json!(vec!["x"; 17])), "INVALID_REQUEST"),
            ("add", Some(json!([" x"])), "INVALID_TAG"),
            ("add", Some(json!(["bad\nlabel"])), "INVALID_TAG"),
            ("add", Some(json!(["文".repeat(43)])), "INVALID_TAG"),
            ("remove", Some(json!(["x"])), "INVALID_TAG"),
            ("operation_id", Some(json!("bad")), "INVALID_ID"),
            ("unknown", Some(json!(true)), "INVALID_REQUEST"),
        ] {
            let mut invalid = request.clone();
            match value {
                Some(value) => {
                    invalid[key] = value;
                }
                None => {
                    invalid.as_object_mut().unwrap().remove(key);
                }
            }
            let (status, rejected) = send(&app, Some(&token), "POST", &route, Some(invalid)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{rejected}");
            assert_eq!(rejected["error"]["code"], code);
            assert_eq!(history(&app, &token, file).await, before);
        }
        let mut stale = request.clone();
        stale["targets"][1]["expected_node_id"] = "stale".into();
        let (status, rejected) = send(&app, Some(&token), "POST", &route, Some(stale)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(rejected["error"]["code"], "FILE_BRANCH_CONFLICT");
        assert_eq!(history(&app, &token, file).await, before);
        let full = (0..16).map(|i| format!("tag{i}")).collect::<Vec<_>>();
        let seeded = success(
            &app,
            &token,
            "POST",
            &route,
            Some(
                json!({"targets":[targets[1]],"add":full,"remove":[],"operation_id":new_id("op")}),
            ),
        )
        .await;
        let right_head = seeded["result"]["nodes"][0]["node_id"].as_str().unwrap();
        request["targets"][1]["source_node_id"] = right_head.into();
        request["targets"][1]["expected_node_id"] = right_head.into();
        let before_overflow = history(&app, &token, file).await;
        let (status, rejected) =
            send(&app, Some(&token), "POST", &route, Some(request.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(rejected["error"]["code"], "TAG_LIMIT_EXCEEDED");
        assert_eq!(
            history(&app, &token, file).await,
            before_overflow,
            "second-target overflow must not publish the first target"
        );
        request["remove"] = json!(["tag0"]);
        let receipt = success(&app, &token, "POST", &route, Some(request.clone())).await;
        assert_eq!(receipt["request_digest"],storage_contract::object_hash("request",&json!({"kind":"file.tags.update","principal":"admin","body":{"file_id":file,"targets":request["targets"],"add":["x"],"remove":["tag0"],"location":location}})).unwrap());
        assert_eq!(receipt["result"]["nodes"][0]["tags"], json!(["x"]));
        assert_eq!(
            receipt["result"]["nodes"][1]["tags"]
                .as_array()
                .unwrap()
                .len(),
            16
        );
        let after = history(&app, &token, file).await;
        let mut changed = request.clone();
        changed["add"] = json!(["x", "x"]);
        let (status, rejected) = send(&app, Some(&token), "POST", &route, Some(changed)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(rejected["error"]["code"], "IDEMPOTENCY_KEY_REUSED");
        let mut invalid = request.clone();
        invalid["add"] = json!([" bad"]);
        let (status, rejected) = send(&app, Some(&token), "POST", &route, Some(invalid)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            rejected["error"]["code"], "INVALID_TAG",
            "static policy still precedes the receipt lookup"
        );
        assert_eq!(history(&app, &token, file).await, after);
        let left_head = receipt["result"]["nodes"][0]["node_id"].as_str().unwrap();
        success(&app,&token,"POST",&route,Some(json!({"targets":[{"branch_id":left,"source_node_id":left_head,"expected_node_id":left_head}],"add":["later"],"remove":["x"],"operation_id":new_id("op")}))).await;
        let later = history(&app, &token, file).await;
        drop(app);
        let system = root.path().join("system");
        let backend = Arc::new(EngineBackend::new(reopen_after_router_drop(&system).await));
        let auth = backend.open_auth(&system).unwrap();
        let app = storage_server::router(backend, auth, ServerConfig::loopback(&system)).unwrap();
        let token = login(&app, "admin", "integration-test-password").await;
        let revoked = revoked_session(&app).await;
        assert_eq!(
            send(&app, Some(&revoked), "POST", &route, Some(request.clone()))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        let replay = success(&app, &token, "POST", &route, Some(request.clone())).await;
        for field in [
            "operation_id",
            "request_digest",
            "publication_sequence",
            "result",
        ] {
            assert_eq!(replay[field], receipt[field], "{field}");
        }
        let mut fresh = request;
        fresh["operation_id"] = new_id("op").into();
        let (status, rejected) = send(&app, Some(&token), "POST", &route, Some(fresh)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(rejected["error"]["code"], "FILE_BRANCH_CONFLICT");
        assert_eq!(history(&app, &token, file).await, later);
    }
}

#[tokio::test]
async fn typed_backup_routes_preserve_wire_admission_digest_and_permanent_receipt() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "report.txt", b"backup receipt body").await;
    let listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    let repo = listing["context"]["repository_id"].as_str().unwrap();
    let route = format!("/api/repositories/{repo}/backup-configuration");
    let before = success(&app, &token, "GET", &route, None).await;
    assert_eq!(
        before,
        success(
            &app,
            &token,
            "GET",
            &format!("{route}?path=&unused=accepted"),
            None
        )
        .await
    );
    let operation = new_id("op");
    let request = json!({"target_copies":2,"backup_prefixes":{"B01":"backup"},"expected_generation":"0","expected_scope_id":before["scope"]["id"],"operation_id":operation});
    for (key, value, code) in [
        ("path", json!(null), "INVALID_REQUEST"),
        (
            "target_copies",
            json!(u64::from(u32::MAX) + 1),
            "INVALID_REQUEST",
        ),
        ("backup_prefixes", json!({"B01":null}), "INVALID_REQUEST"),
        ("expected_generation", json!("00"), "INVALID_REQUEST"),
        ("expected_generation", json!("+0"), "INVALID_REQUEST"),
        ("expected_generation", json!(0), "INVALID_REQUEST"),
        ("expected_scope_id", json!(null), "INVALID_REQUEST"),
        ("operation_id", json!("bad"), "INVALID_ID"),
        ("repository_id", json!("other"), "INVALID_REQUEST"),
        ("unknown", json!(true), "INVALID_REQUEST"),
    ] {
        let mut invalid = request.clone();
        invalid[key] = value;
        let (status, response) = send(&app, Some(&token), "PUT", &route, Some(invalid)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{key}: {response}");
        assert_eq!(response["error"]["code"], code);
        assert_eq!(success(&app, &token, "GET", &route, None).await, before);
    }
    let mut incomplete = request.clone();
    incomplete["target_copies"] = 4.into();
    let (status, rejected) = send(&app, Some(&token), "PUT", &route, Some(incomplete)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{rejected}");
    assert_eq!(rejected["error"]["code"], "ROUTE_INCOMPLETE");
    let original = success(&app, &token, "PUT", &route, Some(request.clone())).await;
    let canonical_body = json!({"repository_id":repo,"path":"","expected_scope_id":before["scope"]["id"],"target_copies":2,"backup_prefixes":{"B01":"backup"},"expected_generation":"0"});
    assert_eq!(
        original["request_digest"],
        storage_contract::object_hash(
            "request",
            &json!({"kind":"backup.configure","principal":"admin","body":canonical_body})
        )
        .unwrap()
    );
    let mut explicit_root = request.clone();
    explicit_root["path"] = "".into();
    assert_eq!(
        success(&app, &token, "PUT", &route, Some(explicit_root)).await["result"],
        original["result"]
    );
    let mut newer = request.clone();
    newer["operation_id"] = new_id("op").into();
    newer["expected_generation"] = "1".into();
    newer["backup_prefixes"] = json!({"B01":"new-backup"});
    success(&app, &token, "PUT", &route, Some(newer)).await;
    let after = success(&app, &token, "GET", &route, None).await;
    assert_eq!(after["generation"], "2");
    drop(app);
    let system = root.path().join("system");
    let backend = Arc::new(EngineBackend::new(reopen_after_router_drop(&system).await));
    let auth = backend.open_auth(&system).unwrap();
    let app = storage_server::router(backend, auth, ServerConfig::loopback(&system)).unwrap();
    let token = login(&app, "admin", "integration-test-password").await;
    let replay = success(&app, &token, "PUT", &route, Some(request.clone())).await;
    assert_eq!(replay["result"], original["result"]);
    assert_eq!(replay["sequence"], original["sequence"]);
    assert_eq!(replay["request_digest"], original["request_digest"]);
    assert_eq!(success(&app, &token, "GET", &route, None).await, after);
    let mut stale_new_operation = request;
    stale_new_operation["operation_id"] = new_id("op").into();
    let (status, rejected) =
        send(&app, Some(&token), "PUT", &route, Some(stale_new_operation)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{rejected}");
    assert_eq!(rejected["error"]["code"], "CONFIG_CONFLICT");
}
