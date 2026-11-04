use super::*;

#[tokio::test]
async fn placement_configuration_http_authentication_scope_and_generation_fences() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "docs/a.txt", b"old").await;
    let listing = success(
        &app,
        &token,
        "GET",
        "/api/file-manager/disks/P01/entries",
        None,
    )
    .await;
    let repo = listing["context"]["repository_id"].as_str().unwrap();
    let endpoint = format!("/api/repositories/{repo}/placement-configuration");
    let read_path = format!("{endpoint}?path=docs");
    let (unauthorized, _) = send(&app, None, "GET", &read_path, None).await;
    assert_eq!(unauthorized, StatusCode::UNAUTHORIZED);
    let snapshot = success(&app, &token, "GET", &read_path, None).await;
    assert_eq!(snapshot["repository_id"], repo);
    assert_eq!(snapshot["scope"]["path"], "docs");
    assert_eq!(snapshot["generation"], "0");
    assert_eq!(snapshot["rule_id"], Value::Null);
    assert_eq!(snapshot["targets"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["targets"][0]["slot_id"], "P01");
    assert!(
        snapshot["targets"][0]["root"]
            .as_str()
            .unwrap()
            .ends_with("d1")
    );
    let request = json!({"path":"docs","primary_slot_id":"P01",
        "expected_scope_id":snapshot["scope"]["id"],"expected_rule_id":snapshot["rule_id"],
        "expected_generation":snapshot["generation"],"operation_id":new_id("op").unwrap()});
    let (unauthorized, _) = send(&app, None, "PUT", &endpoint, Some(request.clone())).await;
    assert_eq!(unauthorized, StatusCode::UNAUTHORIZED);
    let receipt = success(&app, &token, "PUT", &endpoint, Some(request.clone())).await;
    assert_eq!(receipt["operation_kind"], "placement.configure");
    let current = success(&app, &token, "GET", &read_path, None).await;
    assert_eq!(current["generation"], "1");
    assert!(current["rule_id"].as_str().unwrap().starts_with("rule_"));
    let replay = success(&app, &token, "PUT", &endpoint, Some(request.clone())).await;
    assert_eq!(receipt["operation_id"], replay["operation_id"]);
    let mut stale = request;
    stale["operation_id"] = new_id("op").unwrap().into();
    let (status, error) = send(&app, Some(&token), "PUT", &endpoint, Some(stale)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["error"]["code"], "CONFIG_CONFLICT");
    let mut replaced = json!({"path":"docs","primary_slot_id":"P01",
        "expected_scope_id":"entry_00000000000000000000000000000000",
        "expected_rule_id":current["rule_id"],"expected_generation":current["generation"],
        "operation_id":new_id("op").unwrap()});
    let (status, error) = send(&app, Some(&token), "PUT", &endpoint, Some(replaced.clone())).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["error"]["code"], "SCOPE_CHANGED");
    replaced["expected_scope_id"] = current["scope"]["id"].clone();
    replaced["primary_slot_id"] = "B01".into();
    replaced["operation_id"] = new_id("op").unwrap().into();
    let (status, _) = send(&app, Some(&token), "PUT", &endpoint, Some(replaced)).await;
    assert!(status.is_client_error());
    assert_eq!(
        success(&app, &token, "GET", &read_path, None).await,
        current
    );
}
