//! Phase-one body saves and container tags through the real authenticated HTTP
//! router. Temporary level-one roots do not require a Samba daemon or a browser.
#[path = "phase1_save_tags/content_matches.rs"]
mod content_matches;
#[path = "phase1_save_tags/directory_git.rs"]
mod directory_git;
#[path = "phase1_save_tags/discovery.rs"]
mod discovery;
#[path = "phase1_save_tags/file_lifecycle.rs"]
mod file_lifecycle;
#[path = "phase1_save_tags/rejection.rs"]
mod rejection;
#[path = "phase1_save_tags/shared_history.rs"]
mod shared_history;

use http_routing::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use json_codec::{Value, json};
use service_stack::ServiceExt;
use std::{path::Path, sync::Arc, time::Duration};
use storage_contract::{Config, Mode, new_id, request_digest};
use storage_engine::Engine;
use storage_server::{ServerConfig, auth::AuthStore, engine_backend::EngineBackend};

struct Fixture {
    root: tempfile::TempDir,
    app: Router,
    token: String,
}

fn router(engine: Engine, auth: AuthStore, system: &Path) -> Router {
    let backend = Arc::new(EngineBackend::new(engine));
    let auth = backend.protect_auth(auth).unwrap();
    storage_server::router(backend, auth, ServerConfig::new(system)).unwrap()
}

impl Fixture {
    async fn new(disks: &[&str]) -> Self {
        let root = tempfile::tempdir().unwrap();
        let system = root.path().join("system");
        let config = Config::independent_roots(
            Mode::Development,
            disks.iter().map(|name| root.path().join(name)),
        )
        .unwrap();
        let engine = Engine::initialize(&system, config).unwrap();
        let auth = AuthStore::initialize(&system, "admin", "phase-one-test-password").unwrap();
        let app = router(engine, auth, &system);
        let login = request(
            &app,
            None,
            "POST",
            "/api/auth/login",
            Some(json!({"username":"admin", "password":"phase-one-test-password"})),
        )
        .await;
        Self {
            root,
            app,
            token: login["token"].as_str().unwrap().to_owned(),
        }
    }

    async fn json(&self, method: &str, route: &str, body: Option<Value>) -> Value {
        request(&self.app, Some(&self.token), method, route, body).await
    }

    async fn reopen(self) -> Self {
        let Self { root, app, token } = self;
        drop(app);
        let system = root.path().join("system");
        // Router teardown stops new samples. A sampler already holding the
        // engine may still be finishing; only its actual writer lock is retried.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let engine = loop {
            match Engine::open(&system) {
                Ok(engine) => break engine,
                Err(error) if error.code() == storage_contract::ErrorCode::WriterAlreadyActive => {
                    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                    assert!(!remaining.is_zero(), "router retained writer lock: {error}");
                    tokio::time::sleep(remaining.min(Duration::from_millis(10))).await;
                }
                Err(error) => panic!("cold reopen failed: {error}"),
            }
        };
        let auth = AuthStore::open(&system).unwrap();
        let app = router(engine, auth, &system);
        Self { root, app, token }
    }

    async fn save(&self, slot: &str, path: &str, bytes: &[u8]) -> Value {
        let workspace = self
            .json(
                "POST",
                &format!("/api/file-manager/disks/{slot}/write-sessions"),
                Some(json!({"path":path})),
            )
            .await;
        let id = workspace["workspace_id"].as_str().unwrap();
        let item = new_id("entry").unwrap();
        let query = path_query(workspace["relative_path"].as_str().unwrap());
        let response = self
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/workspaces/{id}/files/{item}/content?{query}"))
                    .header("authorization", format!("Bearer {}", self.token))
                    .header("x-zxs-logical-size", bytes.len().to_string())
                    .header("x-zxs-content-hash", blake3::hash(bytes).to_hex().as_str())
                    .header("x-zxs-upload-generation", "1")
                    .body(Body::from(bytes.to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let staged = json_response(response).await;
        assert_eq!(staged["size"], bytes.len().to_string());
        assert_eq!(
            staged["content_hash"],
            blake3::hash(bytes).to_hex().as_str()
        );
        let mut body = self
            .json(
                "POST",
                &format!("/api/workspaces/{id}/seal"),
                Some(json!({})),
            )
            .await;
        body["message"] = "phase-one save".into();
        body["request_digest"] = request_digest(
            "POST",
            "/api/workspaces/{workspace}/commit",
            json!({"workspace":id}),
            json!({}),
            &body,
        )
        .unwrap()
        .into();
        self.json("POST", &format!("/api/workspaces/{id}/commit"), Some(body))
            .await
    }

    async fn entry(&self, slot: &str, path: &str) -> Value {
        let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
        let listing = self
            .json(
                "GET",
                &format!(
                    "/api/file-manager/disks/{slot}/entries?{}",
                    path_query(parent)
                ),
                None,
            )
            .await;
        listing["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == name)
            .unwrap()
            .clone()
    }

    async fn history(&self, file: &str) -> Value {
        let page = self
            .json(
                "GET",
                &format!("/api/file-manager/files/{file}/history"),
                None,
            )
            .await;
        assert!(
            page["page"]["next_cursor"].is_null(),
            "fixture must fit one page"
        );
        page
    }

    async fn versions(&self, file: &str, nodes: &Value) -> Value {
        let root = nodes["identity"]["history_root"].as_str().unwrap();
        let query = url_encoding::form::Serializer::new(String::new())
            .append_pair("history_root", root)
            .finish();
        let page = self
            .json("GET", &format!("/api/files/{file}/history?{query}"), None)
            .await;
        assert_eq!(page["identity"]["history_root"], root);
        assert!(page["page"]["next_cursor"].is_null());
        page["versions"].clone()
    }

    async fn read_tags(&self, file: &str) -> Value {
        let page = self.history(file).await;
        let context = &page["identity"]["context"];
        let query = url_encoding::form::Serializer::new(String::new())
            .append_pair("repository_id", context["repository_id"].as_str().unwrap())
            .append_pair("path", context["path"].as_str().unwrap())
            .finish();
        self.json("GET", &format!("/api/file-manager/files/{file}/tags?{query}"), None).await
    }

    async fn replace_tags(&self, file: &str, tags: &[&str]) -> Value {
        let snapshot = self.read_tags(file).await;
        self.json(
            "POST",
            &format!("/api/file-manager/files/{file}/tags"),
            Some(json!({
                "repository_id": snapshot["context"]["repository_id"],
                "path": snapshot["context"]["path"],
                "expected_generation": snapshot["generation"],
                "tags": tags,
                "operation_id": new_id("op").unwrap(),
            })),
        ).await
    }

    async fn assert_tag_receipt(&self, original: &Value) {
        let operation = original["operation_id"].as_str().unwrap();
        for suffix in ["", "/outcome"] {
            let queried = self.json(
                "GET", &format!("/api/operations/{operation}{suffix}"), None,
            ).await;
            assert_eq!(queried["state"], "completed");
            for field in ["operation_id", "operation_kind", "request_digest",
                "publication_sequence", "result"] {
                assert_eq!(queried[field], original[field], "{field}");
            }
            let replacement: storage_contract::file_container_tags::Replacement =
                json_codec::from_value(queried["result"].clone()).unwrap();
            replacement.validate(&replacement.request).unwrap();
        }
    }

    async fn body(&self, version: &str) -> Vec<u8> {
        let response = self
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/versions/{version}/content"))
                    .header("authorization", format!("Bearer {}", self.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        http_routing::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec()
    }
}

fn path_query(path: &str) -> String {
    url_encoding::form::Serializer::new(String::new())
        .append_pair("path", path)
        .finish()
}

async fn request(
    app: &Router,
    token: Option<&str>,
    method: &str,
    route: &str,
    body: Option<Value>,
) -> Value {
    let mut request = Request::builder()
        .method(method)
        .uri(route)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(Body::from(
                    json_codec::to_vec(&body.unwrap_or_else(|| json!({}))).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    json_response(response).await
}

async fn json_response(response: http_routing::response::Response) -> Value {
    let status = response.status();
    let bytes = http_routing::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    json_codec::from_slice(&bytes).unwrap()
}

fn current_node(page: &Value) -> &Value {
    let id = &page["identity"]["context"]["current_node_id"];
    page["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["node_id"] == *id)
        .unwrap()
}

#[tokio::test]
async fn unicode_save_tags_unchanged_body_and_replacement_survive_cold_open() {
    let fixture = Fixture::new(&["primary"]).await;
    let path = "资料/报告 & 预算.txt";
    let first = b"first body\0with binary\xff";
    fixture.save("D01", path, first).await;
    let entry = fixture.entry("D01", path).await;
    let file = entry["file_id"].as_str().unwrap();
    let original = entry["version_id"].as_str().unwrap();
    let added = fixture.replace_tags(file, &["review", "已审核"]).await;
    assert_eq!(added["result"]["changed"], true);
    let tagged = fixture.history(file).await;
    assert_eq!(current_node(&tagged)["version_id"], original);
    assert_eq!(fixture.read_tags(file).await["tags"], json!(["review", "已审核"]));
    assert_eq!(
        fixture
            .versions(file, &tagged)
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );

    fixture.save("D01", path, first).await;
    assert_eq!(fixture.entry("D01", path).await["version_id"], original);
    let unchanged = fixture.history(file).await;
    assert_eq!(
        current_node(&unchanged),
        current_node(&tagged),
        "unchanged bytes must retain the same content node"
    );
    assert_eq!(unchanged["nodes"], tagged["nodes"]);
    assert_eq!(
        fixture
            .versions(file, &unchanged)
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let second = b"second body, shorter";
    fixture.save("D01", path, second).await;
    let written = fixture.history(file).await;
    assert_eq!(fixture.read_tags(file).await["tags"], json!(["review", "已审核"]));
    let replacement = current_node(&written)["version_id"].as_str().unwrap();
    assert_ne!(replacement, original);
    let replacement_receipt = fixture.replace_tags(file, &["final", "已审核"]).await;
    let final_history = fixture.history(file).await;
    assert_eq!(
        fixture.read_tags(file).await["tags"],
        json!(["final", "已审核"])
    );
    assert_eq!(
        fixture
            .versions(file, &final_history)
            .await
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let original_node = final_history["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["node_id"] == original)
        .unwrap();
    assert!(original_node.get("tags").is_none());
    assert!(final_history["nodes"].as_array().unwrap().iter().all(|node| {
        node.get("tags").is_none() && node["kind"] != "tags"
    }));
    assert_eq!(fixture.body(original).await, first);
    assert_eq!(fixture.body(replacement).await, second);

    let tags = fixture.read_tags(file).await;
    fixture.assert_tag_receipt(&replacement_receipt).await;
    let fixture = fixture.reopen().await;
    assert_eq!(fixture.read_tags(file).await, tags);
    fixture.assert_tag_receipt(&added).await;
    fixture.assert_tag_receipt(&replacement_receipt).await;
    assert_eq!(fixture.history(file).await, final_history);
    assert_eq!(fixture.entry("D01", path).await["file_id"], file);
    assert_eq!(fixture.body(original).await, first);
    assert_eq!(fixture.body(replacement).await, second);
}

#[tokio::test]
async fn empty_file_and_truncate_to_empty_keep_identity_and_container_tags() {
    let fixture = Fixture::new(&["primary"]).await;
    fixture.save("D01", "empty.bin", b"").await;
    let empty = fixture.entry("D01", "empty.bin").await;
    let file = empty["file_id"].as_str().unwrap();
    assert_eq!(empty["size"], "0");
    assert!(
        fixture
            .body(empty["version_id"].as_str().unwrap())
            .await
            .is_empty()
    );
    fixture.replace_tags(file, &["keep"]).await;
    // Cross the configured 262144-byte checksum boundary with non-text bytes.
    let bytes = (0..=255).cycle().take(262144 + 17).collect::<Vec<u8>>();
    fixture.save("D01", "empty.bin", &bytes).await;
    let nonempty = fixture.entry("D01", "empty.bin").await;
    assert_eq!(nonempty["file_id"], file);
    assert_eq!(
        fixture.body(nonempty["version_id"].as_str().unwrap()).await,
        bytes
    );
    fixture.save("D01", "empty.bin", b"").await;
    let truncated = fixture.entry("D01", "empty.bin").await;
    assert_eq!(truncated["file_id"], file);
    assert_eq!(truncated["size"], "0");
    let history = fixture.history(file).await;
    assert_eq!(fixture.read_tags(file).await["tags"], json!(["keep"]));
    let tags = fixture.read_tags(file).await;
    let fixture = fixture.reopen().await;
    assert_eq!(fixture.read_tags(file).await, tags);
    assert_eq!(fixture.history(file).await, history);
    assert!(
        fixture
            .body(truncated["version_id"].as_str().unwrap())
            .await
            .is_empty()
    );
    assert_eq!(
        fixture.body(nonempty["version_id"].as_str().unwrap()).await,
        bytes
    );
}

#[tokio::test]
async fn same_named_files_on_two_level_one_disks_keep_independent_tags() {
    let fixture = Fixture::new(&["alpha", "beta"]).await;
    for slot in ["D01", "D02"] {
        fixture.save(slot, "same.txt", b"same body").await;
    }
    let alpha = fixture.entry("D01", "same.txt").await;
    let beta = fixture.entry("D02", "same.txt").await;
    let left = alpha["file_id"].as_str().unwrap();
    let right = beta["file_id"].as_str().unwrap();
    assert_ne!(left, right);
    fixture.replace_tags(left, &["alpha"]).await;
    assert_eq!(
        fixture.read_tags(right).await["tags"],
        json!([])
    );
    fixture.replace_tags(right, &["beta"]).await;
    let left_history = fixture.history(left).await;
    let right_history = fixture.history(right).await;
    assert_eq!(fixture.read_tags(left).await["tags"], json!(["alpha"]));
    assert_eq!(fixture.read_tags(right).await["tags"], json!(["beta"]));
    let left_tags = fixture.read_tags(left).await;
    let right_tags = fixture.read_tags(right).await;
    let fixture = fixture.reopen().await;
    assert_eq!(fixture.read_tags(left).await, left_tags);
    assert_eq!(fixture.read_tags(right).await, right_tags);
    assert_eq!(fixture.history(left).await, left_history);
    assert_eq!(fixture.history(right).await, right_history);
    assert_eq!(
        fixture.body(alpha["version_id"].as_str().unwrap()).await,
        b"same body"
    );
    assert_eq!(
        fixture.body(beta["version_id"].as_str().unwrap()).await,
        b"same body"
    );
}
