//! Exact phase-one directory behavior through the production HTTP router.
//! This module reuses the real Engine/AuthStore fixture and cold-open lifecycle.
use super::{Body, Fixture, Request, ServiceExt, StatusCode, Value, json, new_id, path_query};

async fn listing(fixture: &Fixture, path: &str) -> Value {
    fixture
        .json(
            "GET",
            &format!("/api/file-manager/disks/D01/entries?{}", path_query(path)),
            None,
        )
        .await
}

async fn status(fixture: &Fixture, repository: &str, path: &str) -> Value {
    let value = fixture
        .json(
            "GET",
            &format!(
                "/api/repositories/{repository}/directory-git?{}",
                path_query(path)
            ),
            None,
        )
        .await;
    let _: storage_contract::directory_git::DirectoryGitStatus =
        json_codec::from_value(value.clone()).unwrap();
    value
}

async fn initialize(fixture: &Fixture, repository: &str, path: &str) -> Value {
    let current = listing(fixture, path).await;
    fixture
        .json(
            "POST",
            &format!("/api/repositories/{repository}/directory-git/initialize"),
            Some(json!({
                "path":path,
                "expected_namespace_head":current["context"]["base_head"],
                "operation_id":new_id("op").unwrap()
            })),
        )
        .await
}

fn stage_body(current: &Value) -> Value {
    json!({
        "directory_id":current["directory_id"],
        "directory_branch_id":current["directory_branch_id"],
        "expected_tree_id":current["working_tree_id"],
        "expected_index_generation":current["index_generation"],
        "operation_id":new_id("op").unwrap()
    })
}

fn branch_body(current: &Value, namespace: &Value, name: &str) -> Value {
    json!({
        "path":current["path"],
        "name":name,
        "operation_id":new_id("op").unwrap(),
        "source":{
            "repository_id":current["repository_id"],
            "directory_id":current["directory_id"],
            "directory_branch_id":current["directory_branch_id"],
            "working_tree_id":current["working_tree_id"],
            "head_tree_id":current["head_tree_id"],
            "index_generation":current["index_generation"],
            "namespace_head":namespace["context"]["base_head"]
        }
    })
}

async fn stage(fixture: &Fixture, repository: &str, current: &Value) -> Value {
    fixture
        .json(
            "POST",
            &format!("/api/repositories/{repository}/directory-git/stage"),
            Some(stage_body(current)),
        )
        .await
}

fn commit_body(current: &Value) -> Value {
    json!({
        "expected_tree_id":current["working_tree_id"],
        "expected_index_generation":current["index_generation"],
        "message":"Reviewed directory checkpoint",
        "operation_id":new_id("op").unwrap()
    })
}

fn commit_route(repository: &str, current: &Value) -> String {
    format!(
        "/api/repositories/{repository}/directory-branches/{}/commits",
        current["directory_branch_id"].as_str().unwrap()
    )
}

async fn commit(fixture: &Fixture, repository: &str, current: &Value) -> Value {
    fixture
        .json(
            "POST",
            &commit_route(repository, current),
            Some(commit_body(current)),
        )
        .await
}

async fn checkpoint(fixture: &Fixture, repository: &str, path: &str) -> Value {
    stage(
        fixture,
        repository,
        &status(fixture, repository, path).await,
    )
    .await;
    commit(
        fixture,
        repository,
        &status(fixture, repository, path).await,
    )
    .await;
    status(fixture, repository, path).await
}

async fn preview(fixture: &Fixture, path: &str, branch: &str, tree: &str) -> Value {
    let query = url_encoding::form::Serializer::new(String::new())
        .append_pair("path", path)
        .append_pair("directory_branch_id", branch)
        .append_pair("expected_tree_id", tree)
        .finish();
    let result = fixture
        .json(
            "GET",
            &format!("/api/file-manager/disks/D01/entries?{query}"),
            None,
        )
        .await;
    assert_eq!(result["preview"], true);
    assert_eq!(result["writable"], false);
    result
}

fn entry<'a>(listing: &'a Value, name: &str) -> &'a Value {
    listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == name)
        .unwrap()
}

async fn reject(
    fixture: &Fixture,
    method: &str,
    route: &str,
    body: Value,
    expected: StatusCode,
) -> Value {
    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(route)
                .header("authorization", format!("Bearer {}", fixture.token))
                .header("content-type", "application/json")
                .body(Body::from(json_codec::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let actual = response.status();
    let bytes = http_routing::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(actual, expected, "{}", String::from_utf8_lossy(&bytes));
    json_codec::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn reviewed_index_and_later_work_survive_http_commit_replay_and_cold_open() {
    let fixture = Fixture::new(&["primary"]).await;
    let path = "资料 & 计划/报告.txt";
    let directory = "资料 & 计划";
    fixture.save("D01", path, b"reviewed body").await;
    let page = listing(&fixture, directory).await;
    let repository = page["context"]["repository_id"].as_str().unwrap();
    let before = status(&fixture, repository, directory).await;
    assert_eq!(before["initialized"], false);
    assert_eq!(status(&fixture, repository, directory).await, before);
    assert_eq!(listing(&fixture, directory).await, page);

    let initialize_route = format!("/api/repositories/{repository}/directory-git/initialize");
    let initialization = json!({
        "path":directory,
        "expected_namespace_head":page["context"]["base_head"],
        "operation_id":new_id("op").unwrap()
    });
    let mut stale = initialization.clone();
    stale["expected_namespace_head"] = new_id("commit").unwrap().into();
    reject(
        &fixture,
        "POST",
        &initialize_route,
        stale,
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(status(&fixture, repository, directory).await, before);
    let initialized = fixture
        .json("POST", &initialize_route, Some(initialization.clone()))
        .await;
    assert_eq!(
        fixture
            .json("POST", &initialize_route, Some(initialization))
            .await,
        initialized
    );
    let current = status(&fixture, repository, directory).await;
    assert_eq!(current["head_tree_id"], Value::Null);
    let stage_route = format!("/api/repositories/{repository}/directory-git/stage");
    let mut malformed = stage_body(&current);
    malformed["expected_index_generation"] = json!(0);
    reject(
        &fixture,
        "POST",
        &stage_route,
        malformed,
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(status(&fixture, repository, directory).await, current);
    stage(&fixture, repository, &current).await;

    fixture.save("D01", path, b"later working body").await;
    let reviewed = status(&fixture, repository, directory).await;
    assert_eq!(reviewed["staged"], true);
    assert_eq!(reviewed["unstaged"], true);
    let route = commit_route(repository, &reviewed);
    let request = commit_body(&reviewed);
    let outcome = fixture.json("POST", &route, Some(request.clone())).await;
    let committed = status(&fixture, repository, directory).await;
    assert_eq!(committed["staged"], false);
    assert_eq!(committed["unstaged"], true);
    let snapshot = preview(
        &fixture,
        directory,
        committed["directory_branch_id"].as_str().unwrap(),
        committed["head_tree_id"].as_str().unwrap(),
    )
    .await;
    let saved = entry(&snapshot, "报告.txt");
    assert_eq!(saved["read_only"], true);
    assert_eq!(
        fixture.body(saved["version_id"].as_str().unwrap()).await,
        b"reviewed body"
    );
    assert_eq!(status(&fixture, repository, directory).await, committed);
    let live = fixture.entry("D01", path).await;
    assert_eq!(
        fixture.body(live["version_id"].as_str().unwrap()).await,
        b"later working body"
    );

    let fixture = fixture.reopen().await;
    assert_eq!(status(&fixture, repository, directory).await, committed);
    assert_eq!(fixture.json("POST", &route, Some(request)).await, outcome);
    assert_eq!(fixture.entry("D01", path).await, live);
    assert_eq!(
        fixture.body(saved["version_id"].as_str().unwrap()).await,
        b"reviewed body"
    );
    assert_eq!(
        fixture.body(live["version_id"].as_str().unwrap()).await,
        b"later working body"
    );
    let next = checkpoint(&fixture, repository, directory).await;
    assert_ne!(next["head_tree_id"], committed["head_tree_id"]);
    assert_eq!(next["unstaged"], false);
}

#[tokio::test]
async fn dirty_work_blocks_branch_changes_until_explicit_checkpoint_and_checkout() {
    let fixture = Fixture::new(&["primary"]).await;
    fixture.save("D01", "docs/file.txt", b"base").await;
    fixture.save("D01", "outside.txt", b"untouched").await;
    let page = listing(&fixture, "docs").await;
    let repository = page["context"]["repository_id"].as_str().unwrap();
    initialize(&fixture, repository, "docs").await;
    let original = checkpoint(&fixture, repository, "docs").await;
    let branch_route = "/api/file-manager/disks/D01/directory-branches";
    let current = listing(&fixture, "docs").await;
    let request = branch_body(&original, &current, "frozen");
    let created = fixture
        .json("POST", branch_route, Some(request.clone()))
        .await;
    assert_eq!(created["result"]["input"]["source"], request["source"]);
    let branch = created["result"]["directory_branch_id"].as_str().unwrap();
    let scopes = fixture
        .json(
            "GET",
            &format!("/api/repositories/{repository}/directory-branches"),
            None,
        )
        .await;
    let frozen = scopes["directory_branches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|scope| scope["directory_branch_id"] == branch)
        .unwrap();
    let frozen_tree = frozen["head_tree_id"].as_str().unwrap();
    assert_eq!(frozen_tree, original["head_tree_id"].as_str().unwrap());
    fixture.save("D01", "docs/file.txt", b"unstaged").await;
    let dirty = status(&fixture, repository, "docs").await;
    assert_eq!(dirty["staged"], false);
    assert_eq!(dirty["unstaged"], true);
    let no_change = commit(&fixture, repository, &dirty).await;
    assert_eq!(no_change["result"]["no_change"], true);
    assert_eq!(
        no_change["result"]["head_tree_id"],
        original["head_tree_id"]
    );
    let current = listing(&fixture, "docs").await;
    let denied = reject(
        &fixture,
        "POST",
        branch_route,
        branch_body(&dirty, &current, "must not exist"),
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(denied["error"]["code"], "DIRECTORY_BRANCH_CONFLICT");
    let checkout_route = format!("/api/repositories/{repository}/default-branch");
    let denied = reject(
        &fixture,
        "PUT",
        &checkout_route,
        json!({
            "directory_branch_id":branch,"expected_tree_id":frozen_tree,
            "expected_head":current["context"]["base_head"],"operation_id":new_id("op").unwrap()
        }),
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(denied["error"]["code"], "DIRECTORY_BRANCH_CONFLICT");
    assert_eq!(status(&fixture, repository, "docs").await, dirty);
    let snapshot = preview(&fixture, "docs", branch, frozen_tree).await;
    assert_eq!(
        fixture
            .body(entry(&snapshot, "file.txt")["version_id"].as_str().unwrap())
            .await,
        b"base"
    );
    let working = fixture.entry("D01", "docs/file.txt").await;
    assert_eq!(
        fixture.body(working["version_id"].as_str().unwrap()).await,
        b"unstaged"
    );
    let saved = checkpoint(&fixture, repository, "docs").await;
    assert_ne!(saved["head_tree_id"], original["head_tree_id"]);

    let current = listing(&fixture, "docs").await;
    let request = json!({
        "directory_branch_id":branch,"expected_tree_id":frozen_tree,
        "expected_head":current["context"]["base_head"],"operation_id":new_id("op").unwrap()
    });
    let outcome = fixture
        .json("PUT", &checkout_route, Some(request.clone()))
        .await;
    assert_eq!(outcome["operation_kind"], "directory.git.checkout");
    let checked_out = status(&fixture, repository, "docs").await;
    assert_eq!(checked_out["directory_branch_id"], branch);
    assert_eq!(checked_out["staged"], false);
    assert_eq!(checked_out["unstaged"], false);
    let fixture = fixture.reopen().await;
    assert_eq!(status(&fixture, repository, "docs").await, checked_out);
    assert_eq!(
        fixture.json("PUT", &checkout_route, Some(request)).await,
        outcome
    );
    let selected = fixture.entry("D01", "docs/file.txt").await;
    assert_eq!(
        fixture.body(selected["version_id"].as_str().unwrap()).await,
        b"base"
    );
    assert_eq!(
        fixture.body(working["version_id"].as_str().unwrap()).await,
        b"unstaged"
    );
    let outside = fixture.entry("D01", "outside.txt").await;
    assert_eq!(
        fixture.body(outside["version_id"].as_str().unwrap()).await,
        b"untouched"
    );
}

async fn stage_child(fixture: &Fixture, repository: &str, parent: &Value, child: &Value) {
    fixture
        .json(
            "POST",
            &format!("/api/repositories/{repository}/directory-git/children/stage"),
            Some(json!({
                "directory_id":parent["directory_id"],
                "directory_branch_id":parent["directory_branch_id"],
                "expected_index_generation":parent["index_generation"],
                "child_directory_id":child["directory_id"],
                "expected_child_head_tree_id":child["head_tree_id"],
                "operation_id":new_id("op").unwrap()
            })),
        )
        .await;
}

#[tokio::test]
async fn child_reference_staging_keeps_parent_work_out_of_commit_across_cold_open() {
    let fixture = Fixture::new(&["primary"]).await;
    fixture
        .save("D01", "parent/own.txt", b"parent reviewed")
        .await;
    fixture
        .save("D01", "parent/child/file.txt", b"child one")
        .await;
    let page = listing(&fixture, "parent").await;
    let repository = page["context"]["repository_id"].as_str().unwrap();
    initialize(&fixture, repository, "parent/child").await;
    let child_one = checkpoint(&fixture, repository, "parent/child").await;
    initialize(&fixture, repository, "parent").await;
    let parent = status(&fixture, repository, "parent").await;
    stage_child(&fixture, repository, &parent, &child_one).await;
    commit(
        &fixture,
        repository,
        &status(&fixture, repository, "parent").await,
    )
    .await;
    let parent_one = status(&fixture, repository, "parent").await;
    assert_eq!(parent_one["children"].as_array().unwrap().len(), 1);
    assert_eq!(
        parent_one["children"][0]["committed_tree_id"],
        child_one["head_tree_id"]
    );

    fixture
        .save("D01", "parent/child/file.txt", b"child two")
        .await;
    let child_two = checkpoint(&fixture, repository, "parent/child").await;
    let parent_still = status(&fixture, repository, "parent").await;
    assert_eq!(parent_still["head_tree_id"], parent_one["head_tree_id"]);
    assert_eq!(
        parent_still["children"][0]["committed_tree_id"],
        child_one["head_tree_id"]
    );
    assert_eq!(parent_still["children"][0]["reference_changed"], true);
    fixture
        .save("D01", "parent/own.txt", b"parent later work")
        .await;
    stage_child(
        &fixture,
        repository,
        &status(&fixture, repository, "parent").await,
        &child_two,
    )
    .await;
    let reviewed = status(&fixture, repository, "parent").await;
    assert_eq!(reviewed["staged"], true);
    assert_eq!(reviewed["unstaged"], true);
    commit(&fixture, repository, &reviewed).await;
    let parent_two = status(&fixture, repository, "parent").await;
    assert_ne!(parent_two["head_tree_id"], parent_one["head_tree_id"]);
    assert_eq!(
        parent_two["children"][0]["committed_tree_id"],
        child_two["head_tree_id"]
    );
    assert_eq!(parent_two["staged"], false);
    assert_eq!(parent_two["unstaged"], true);
    let snapshot = preview(
        &fixture,
        "parent",
        parent_two["directory_branch_id"].as_str().unwrap(),
        parent_two["head_tree_id"].as_str().unwrap(),
    )
    .await;
    let frozen = entry(&snapshot, "own.txt");
    assert_eq!(
        fixture.body(frozen["version_id"].as_str().unwrap()).await,
        b"parent reviewed"
    );
    let fixture = fixture.reopen().await;
    assert_eq!(status(&fixture, repository, "parent").await, parent_two);
    assert_eq!(
        status(&fixture, repository, "parent/child").await,
        child_two
    );
    let own = fixture.entry("D01", "parent/own.txt").await;
    assert_eq!(
        fixture.body(own["version_id"].as_str().unwrap()).await,
        b"parent later work"
    );
    let child = fixture.entry("D01", "parent/child/file.txt").await;
    assert_eq!(
        fixture.body(child["version_id"].as_str().unwrap()).await,
        b"child two"
    );
    assert_eq!(
        fixture.body(frozen["version_id"].as_str().unwrap()).await,
        b"parent reviewed"
    );
}
