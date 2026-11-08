//! Exercise retained Web configuration and plan review against real HTTP operations.
//! Legacy plan creation remains backend API coverage, not a Web form.
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
struct Applied {
    value: Value,
    action: response::Action,
}
impl Applied {
    fn into_plan(self) -> response::ValidatedPlan {
        let response::Action::Plan(plan) = self.action else {
            panic!("expected validated plan")
        };
        plan
    }
}
fn plan_fixture(id: &str, digest: &str, state: &str) -> response::ValidatedPlan {
    let value = json!({"plan_id":id,"digest":digest,"state":state,"kind":"purge",
        "principal":"admin","expected_sequence":"4","input":{"history_tree_id":"h"}});
    let validated = response::Contract::Plan { id: id.into() }
        .validate(value)
        .unwrap();
    let response::Action::Plan(plan) = validated.action else {
        panic!("expected validated plan")
    };
    plan
}
async fn apply(app: &Router, token: &str, kind: Kind, fields: &request::Fields) -> Applied {
    let request = request::build(kind, fields, &new_id("op").unwrap()).unwrap();
    apply_request(app, token, request).await
}
async fn apply_request(app: &Router, token: &str, request: request::Request) -> Applied {
    let request::Request {
        method,
        path,
        body,
        contract,
    } = request;
    let value = success(app, token, method, &path, body).await;
    let validated = contract.validate(value.clone()).unwrap();
    assert_eq!(
        validated.display,
        json_codec::to_string_pretty(&value).unwrap()
    );
    match &validated.action {
        response::Action::Mutation { operation } => {
            assert_eq!(contract.mutation_operation(), Some(operation.as_str()))
        }
        response::Action::PlanReceipt(receipt) => {
            assert_eq!(
                contract.mutation_operation(),
                Some(receipt.operation_id.to_string().as_str())
            );
            assert_eq!(json_codec::to_value(receipt).unwrap(), value);
        }
        response::Action::Outcome { operation, status } => {
            assert_eq!(value["operation_id"], operation.as_str());
            assert_eq!(
                storage_contract::client_protocol::operation_status(&value),
                Some(*status)
            );
        }
        response::Action::Inspect | response::Action::Plan(_) => {}
    }
    Applied {
        value,
        action: validated.action,
    }
}
#[tokio::test]
async fn retained_web_forms_and_backend_legacy_operations_execute_real_http() {
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
    let id = repo.value["repository_id"].as_str().unwrap();
    let branch = repo.value["main_branch_id"].as_str().unwrap();
    let history = repo.value["history_tree_id"].as_str().unwrap();
    // Retired account/grant/SMB binding forms are not representable by Kind.
    assert!(matches!(
        request::build(Kind::Inspect, &fields(&[]), &new_id("op").unwrap()),
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
        let rule = new_id(prefix).unwrap();
        let f = fields(&[
            ("id", &rule),
            ("repository", id),
            ("copies", "1"),
            ("slot", "P01"),
            ("generation", "0"),
        ]);
        apply(&app, &admin, kind, &f).await;
        let request = request::build(kind, &f, &new_id("op").unwrap()).unwrap();
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
    let current = success(
        &app,
        &admin,
        "GET",
        &format!("/api/branches/{branch}"),
        None,
    )
    .await;
    let commit = current["head_commit_id"].as_str().unwrap();
    let created = success(
        &app,
        &admin,
        "POST",
        &format!("/api/repositories/{id}/branches"),
        Some(json!({
            "source_commit_id": commit, "name": "API branch", "operation_id": new_id("op").unwrap()
        })),
    )
    .await;
    assert_eq!(created["name"], "API branch");
    assert_eq!(created["fork_commit_id"], commit);
    assert_eq!(created["history_tree_id"], history);
    let link = success(
        &app,
        &admin,
        "POST",
        &format!("/api/history-trees/{history}/merge-links"),
        Some(json!({
            "source_branch_id": branch, "source_commit_id": commit,
            "target_branch_id": created["branch_id"].as_str().unwrap(), "target_commit_id": commit,
            "operation_id": new_id("op").unwrap()
        })),
    )
    .await;
    assert_eq!(link["history_tree_id"], history);
    assert_eq!(link["source_branch_id"], branch);
    assert_eq!(link["source_commit_id"], commit);
    assert_eq!(link["target_branch_id"], created["branch_id"]);
    assert_eq!(link["target_commit_id"], commit);
    for publish in [false, true] {
        let created = success(
            &app,
            &admin,
            "POST",
            &format!("/api/history-trees/{history}/historical-save-plans"),
            Some(json!({
                "source_commit_id": commit,
                "name": if publish { "Published snapshot" } else { "Cancelled snapshot" },
                "workspace_id": null, "primary_slot_id": null, "target_copies": null
            })),
        )
        .await;
        let query = request::query(
            request::QueryKind::Plan,
            &fields(&[("id", created["plan_id"].as_str().unwrap())]),
        )
        .unwrap();
        let plan = apply_request(&app, &admin, query).await.into_plan();
        assert!(request::plan_action(Some(&plan), true, false, &new_id("op").unwrap()).is_err());
        let request =
            request::plan_action(Some(&plan), publish, true, &new_id("op").unwrap()).unwrap();
        let result = apply_request(&app, &admin, request).await;
        if publish {
            assert_eq!(result.value["commit_state"], "PUBLISHED");
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
            "op_test"
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
    )
    .unwrap_err();
    assert!(matches!(error, InputError::Generation(_)));
    assert!(std::error::Error::source(&error).is_some());

    let plan = plan_fixture("plan/x", "reviewed-digest", "ready");
    let request = request::plan_action(Some(&plan), true, true, "op_test").unwrap();
    assert_eq!(request.path, "/api/plans/plan%2Fx/commit");
    assert_eq!(request.body.unwrap()["plan_digest"], "reviewed-digest");
    assert!(
        request::plan_action(
            Some(&plan_fixture("p", "d", "completed")),
            true,
            true,
            "op_test"
        )
        .is_err()
    );
}

#[tokio::test]
async fn backend_purge_and_split_plans_support_exact_web_review() {
    for kind in ["purge", "split"] {
        let root = tempfile::tempdir().unwrap();
        let system = root.path().join("system");
        let mut engine = Engine::initialize(
            &system,
            Config::development([
                root.path().join("d1"),
                root.path().join("d2"),
                root.path().join("d3"),
            ])
            .unwrap(),
        )
        .unwrap();
        let repo = engine
            .create_repository(
                &storage_contract::RepositoryName::parse("Maintenance fixture").unwrap(),
                "P01",
                1,
                "admin",
                storage_contract::OperationId::generate().unwrap(),
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
                    operation_id: storage_contract::OperationId::generate().unwrap(),
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
        let app = storage_server::router(backend, auth, ServerConfig::new(&system)).unwrap();
        let admin = login(&app, "admin", "integration-test-password").await;
        let path = format!("/api/history-trees/{}/{}-plans", repo.history_tree_id, kind);
        let input = if kind == "purge" {
            json!({"version_ids": [first]})
        } else {
            json!({"cut_commit_id": cut})
        };
        let created = success(&app, &admin, "POST", &path, Some(input.clone())).await;
        let query = request::query(
            request::QueryKind::Plan,
            &fields(&[("id", created["plan_id"].as_str().unwrap())]),
        )
        .unwrap();
        let cancel = apply_request(&app, &admin, query).await.into_plan();
        let request =
            request::plan_action(Some(&cancel), false, false, &new_id("op").unwrap()).unwrap();
        apply_request(&app, &admin, request).await;
        let created = success(&app, &admin, "POST", &path, Some(input)).await;
        let query = request::query(
            request::QueryKind::Plan,
            &fields(&[("id", created["plan_id"].as_str().unwrap())]),
        )
        .unwrap();
        let plan = apply_request(&app, &admin, query).await.into_plan();
        let mut request =
            request::plan_action(Some(&plan), true, true, &new_id("op").unwrap()).unwrap();
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
        let request =
            request::plan_action(Some(&plan), true, true, &new_id("op").unwrap()).unwrap();
        assert_eq!(
            apply_request(&app, &admin, request).await.value["commit_state"],
            "PUBLISHED"
        );
    }
}
