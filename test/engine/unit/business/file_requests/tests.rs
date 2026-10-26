use super::*;

#[test]
fn metadata_request_bodies_bind_absent_and_supplied_location() {
    let location = FileLocation {
        repository_id: "repo".into(),
        path: "report.txt".into(),
    };
    let mut tags = FileNodeTagsUpdateRequest {
        file_id: "file",
        targets: &[],
        add: &[],
        remove: &[],
        location: None,
        principal: "owner",
        operation_id: "op_00000000000000000000000000000000".parse().unwrap(),
    };
    let mut merge = FileBranchMergeRequest {
        file_id: "file",
        inputs: &[],
        chosen_node_id: "node",
        surviving_branch_id: "branch",
        location: None,
        principal: "owner",
        operation_id: "op_00000000000000000000000000000000".parse().unwrap(),
    };
    for unscoped in [tags.body(), merge.body()] {
        assert_eq!(unscoped.get("location"), Some(&Value::Null));
        let mut incomplete = unscoped.clone();
        incomplete.as_object_mut().unwrap().remove("location");
        assert_ne!(
            crate::object_hash("request", &unscoped).unwrap(),
            crate::object_hash("request", &incomplete).unwrap()
        );
    }
    tags.location = Some(&location);
    merge.location = Some(&location);
    for scoped in [tags.body(), merge.body()] {
        assert_eq!(
            scoped["location"],
            json!({"repository_id":"repo","path":"report.txt"})
        );
    }
}
