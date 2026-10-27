use super::*;
use json_codec::json;

#[test]
fn disk_workspace_projection_keeps_wire_fields_without_local_staging_paths() {
    let workspace=json_codec::from_value(json!({
        "workspace_id":"workspace","purpose":"normal","branch_id":"branch","principal":"actor","base_head":"head","fencing_token":"11",
        "files":{"item":{"item_id":"item","path":"public.txt","source_path":"/private/staging/body","size":"3","content_hash":"0".repeat(64),"dirty_generation":"1","base_version_id":null}},
        "namespace_operations":[],"created_at":"10","state":"open"
    })).unwrap();
    let response = DiskWriteSessionResponse::from(DiskWriteSession {
        workspace,
        relative_path: "public.txt".into(),
        display_path: "Repo/public.txt".into(),
    });
    let response = storage_contract::strict_json(&json_codec::to_vec(&response).unwrap()).unwrap();
    assert_eq!(response["workspace_id"], "workspace");
    assert_eq!(response["relative_path"], "public.txt");
    assert_eq!(response["display_path"], "Repo/public.txt");
    assert_eq!(response["fencing_token"], "11");
    assert_eq!(
        response["files"]["item"],
        json!({"item_id":"item","path":"public.txt","size":"3","content_hash":"0".repeat(64),"dirty_generation":"1","base_version_id":null})
    );
    assert!(!response.to_string().contains("/private/staging/body"));
    assert!(response.get("operation_id").is_none());
}
