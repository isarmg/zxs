//! Drive real HTTP operations with the exact request builders used by Web forms.
use super::*;
#[path = "../../../../src/web/src/admin/request.rs"]
mod request;
#[path = "../../../../src/web/src/admin/response.rs"]
mod response;
use request::{InputError, Kind};
fn fields(pairs: &[(&str, &str)]) -> request::Fields {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}
async fn apply(app: &Router, token: &str, kind: Kind, fields: &request::Fields) -> Value {
    let request = request::build(kind, fields, &new_id("op"), "admin").unwrap();
    apply_request(app, token, request).await
}
async fn apply_request(app: &Router, token: &str, request: request::Request) -> Value {
    let request::Request {
        method,
        path,
        body,
        contract,
    } = request;
    let value = success(app, token, method, &path, body).await;
    contract.validate(value).unwrap()
}
#[tokio::test]
async fn web_daily_forms_execute_real_management_operations() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let admin = login(&app, "admin", "integration-test-password").await;
    let repo = apply(
        &app,
        &admin,
        Kind::Repository,
        &fields(&[("name", "Web managed"), ("slot", "P01"), ("copies", "1")]),
    )
    .await;
    let id = repo["repository_id"].as_str().unwrap();
    let branch = repo["main_branch_id"].as_str().unwrap();
    let history = repo["history_tree_id"].as_str().unwrap();
    // Retired account/grant actions are not representable by the production Kind.
    assert!(matches!(
        request::build(Kind::Inspect, &fields(&[]), &new_id("op"), "admin"),
        Err(InputError::Inspect)
    ));
    let session = login(&app, "admin", "integration-test-password").await;
    let repository_path = format!("/api/repositories/{id}");
    assert_eq!(
        success(&app, &session, "GET", &repository_path, None).await["repository_id"],
        id
    );
    success(&app, &session, "POST", "/api/auth/logout", None).await;
    for path in [repository_path.as_str(), "/api/auth/me"] {
        assert_eq!(
            send(&app, Some(&session), "GET", path, None).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        success(&app, &admin, "GET", &repository_path, None).await["repository_id"],
        id
    );
    for (kind, prefix) in [(Kind::Policy, "policy"), (Kind::Rule, "rule")] {
        let rule = new_id(prefix);
        let f = fields(&[
            ("id", &rule),
            ("repository", id),
            ("copies", "1"),
            ("slot", "P01"),
            ("generation", "0"),
        ]);
        apply(&app, &admin, kind, &f).await;
        let request = request::build(kind, &f, &new_id("op"), "admin").unwrap();
        assert_eq!(
            send(
                &app,
                Some(&admin),
                request.method,
                &request.path,
                request.body
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
    }
    let binding = success(
        &app,
        &admin,
        "GET",
        &format!("/api/repositories/{id}/smb-binding"),
        None,
    )
    .await;
    apply(
        &app,
        &admin,
        Kind::Smb,
        &fields(&[
            ("repository", id),
            ("branch", branch),
            ("enabled", "false"),
            (
                "generation",
                binding["history_generation"].as_str().unwrap(),
            ),
        ]),
    )
    .await;
    assert_eq!(
        success(
            &app,
            &admin,
            "GET",
            &format!("/api/repositories/{id}/smb-binding"),
            None
        )
        .await["enabled"],
        false
    );
    let current = success(
        &app,
        &admin,
        "GET",
        &format!("/api/branches/{branch}"),
        None,
    )
    .await;
    let commit = current["head_commit_id"].as_str().unwrap();
    let created = apply(
        &app,
        &admin,
        Kind::Branch,
        &fields(&[
            ("repository", id),
            ("commit", commit),
            ("name", "Web branch"),
        ]),
    )
    .await;
    apply(
        &app,
        &admin,
        Kind::Link,
        &fields(&[
            ("history", history),
            ("source_branch", branch),
            ("source_commit", commit),
            ("target_branch", created["branch_id"].as_str().unwrap()),
            ("target_commit", commit),
        ]),
    )
    .await;
    for publish in [false, true] {
        let plan = apply(
            &app,
            &admin,
            Kind::Save,
            &fields(&[
                ("history", history),
                ("commit", commit),
                (
                    "name",
                    if publish {
                        "Published snapshot"
                    } else {
                        "Cancelled snapshot"
                    },
                ),
            ]),
        )
        .await;
        assert!(request::plan_action(&plan, true, false, &new_id("op")).is_err());
        let request = request::plan_action(&plan, publish, true, &new_id("op")).unwrap();
        let result = apply_request(&app, &admin, request).await;
        if publish {
            assert_eq!(result["commit_state"], "PUBLISHED");
        }
    }
    success(&app, &admin, "GET", "/api/admin/check", None).await;
}
#[test]
fn web_daily_contracts_reject_invalid_input_and_bind_exact_review_digest() {
    assert!(matches!(
        request::build(
            Kind::Repository,
            &fields(&[("name", "test"), ("slot", "P01"), ("copies", "0")]),
            "op_test",
            "admin"
        ),
        Err(InputError::ZeroCopies)
    ));
    let error = request::build(
        Kind::Policy,
        &fields(&[
            ("id", "p"),
            ("repository", "r"),
            ("copies", "1"),
            ("generation", "-1"),
        ]),
        "op_test",
        "admin",
    )
    .unwrap_err();
    assert!(matches!(error, InputError::Generation(_)));
    assert!(std::error::Error::source(&error).is_some());

    let plan = json!({"plan_id":"plan/x","digest":"reviewed-digest","state":"ready"});
    let request = request::plan_action(&plan, true, true, "op_test").unwrap();
    assert_eq!(request.path, "/api/plans/plan%2Fx/commit");
    assert_eq!(request.body.unwrap()["plan_digest"], "reviewed-digest");
    assert!(
        request::plan_action(
            &json!({"plan_id":"p","digest":"d","state":"completed"}),
            true,
            true,
            "op_test"
        )
        .is_err()
    );
    for (kind, key, value) in [
        (Kind::Purge, "versions", "v1,v2"),
        (Kind::Split, "commit", "c1"),
    ] {
        let request = request::build(
            kind,
            &fields(&[("history", "h"), (key, value)]),
            "op_test",
            "admin",
        )
        .unwrap();
        assert!(request.path.ends_with("-plans"));
        assert!(request.body.unwrap().get("operation_id").is_none());
        assert!(!request.path.ends_with("/commit"));
    }
}

#[tokio::test]
async fn web_purge_and_split_preview_digest_cancel_and_commit_use_real_engine() {
    for kind in [Kind::Purge, Kind::Split] {
        let root = tempfile::tempdir().unwrap();
        let system = root.path().join("system");
        let mut engine = Engine::initialize(
            &system,
            Config::development([
                root.path().join("d1"),
                root.path().join("d2"),
                root.path().join("d3"),
            ]),
        )
        .unwrap();
        let repo = engine
            .create_repository(
                &storage_contract::RepositoryName::parse("Maintenance fixture").unwrap(),
                "P01",
                1,
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        let mut first = String::new();
        let mut cut = String::new();
        for bytes in [b"first".as_slice(), b"second".as_slice()] {
            let ws = engine
                .create_workspace(&repo.main_branch_id, "admin")
                .unwrap();
            engine
                .stage_bytes(&ws.workspace_id, "fixture.txt", bytes)
                .unwrap();
            let op = engine
                .commit_workspace(CommitRequest {
                    workspace_id: ws.workspace_id,
                    operation_id: storage_contract::OperationId::generate(),
                    principal: "admin".into(),
                    expected_head: ws.base_head,
                    fencing_token: ws.fencing_token,
                    message: "Fixture".into(),
                    request_digest: None,
                })
                .unwrap();
            if first.is_empty() {
                first = engine.list_tree(&repo.main_branch_id).unwrap()[0]
                    .version_id
                    .clone();
                cut = op.commit_id.unwrap();
            }
        }
        let backend = Arc::new(EngineBackend::new(engine));
        let auth = backend
            .protect_auth(
                AuthStore::initialize(&system, "admin", "integration-test-password").unwrap(),
            )
            .unwrap();
        let app = storage_server::router(backend, auth, ServerConfig::loopback(&system)).unwrap();
        let admin = login(&app, "admin", "integration-test-password").await;
        let input = fields(&[
            ("history", &repo.history_tree_id),
            ("versions", &first),
            ("commit", &cut),
        ]);
        let cancel = apply(&app, &admin, kind, &input).await;
        let request = request::plan_action(&cancel, false, false, &new_id("op")).unwrap();
        apply_request(&app, &admin, request).await;
        let plan = apply(&app, &admin, kind, &input).await;
        let mut request = request::plan_action(&plan, true, true, &new_id("op")).unwrap();
        request.body.as_mut().unwrap()["plan_digest"] = "wrong-review-digest".into();
        assert!(
            !send(
                &app,
                Some(&admin),
                request.method,
                &request.path,
                request.body
            )
            .await
            .0
            .is_success()
        );
        let request = request::plan_action(&plan, true, true, &new_id("op")).unwrap();
        assert_eq!(
            apply_request(&app, &admin, request).await["commit_state"],
            "PUBLISHED"
        );
    }
}
