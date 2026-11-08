use super::*;

#[test]
fn shared_history_published_contract_matches_strict_component_semantics() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut core = read(&root.join("docs/contracts/core.schema.json")).unwrap();
    let mut api = read(&root.join("docs/contracts/openapi.json")).unwrap();
    let vectors = read(&root.join("docs/contracts/vectors.json")).unwrap();
    assert_eq!(api["components"]["schemas"], bundle(&core["$defs"]));
    core["$ref"] = "#/$defs/SharedHistoryGraph".into();
    api["$ref"] = "#/components/schemas/SharedHistoryGraph".into();
    let core_validator = validator(&core).unwrap();
    let api_validator = validator(&api).unwrap();
    let mut count = 0;
    for group in ["positive", "negative"] {
        for case in array(&vectors[group])
            .unwrap()
            .iter()
            .filter(|case| case["schema"] == "SharedHistoryGraph")
        {
            count += 1;
            let shape_valid = group == "positive" || case["layer"] == "semantic";
            assert_eq!(
                core_validator.is_valid(&case["value"]),
                shape_valid,
                "{}",
                case["name"]
            );
            assert_eq!(
                api_validator.is_valid(&case["value"]),
                shape_valid,
                "{}",
                case["name"]
            );
            if shape_valid {
                assert_eq!(
                    semantics("SharedHistoryGraph", &case["value"]).is_ok(),
                    group == "positive",
                    "{}",
                    case["name"]
                );
            }
        }
    }
    assert_eq!(count, 9);
    let operation = &api["paths"]["/api/file-manager/files/{file}/shared-history"]["get"];
    assert_eq!(
        operation["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/SharedHistoryGraph"
    );
    assert_eq!(array(&operation["parameters"]).unwrap().len(), 2);
    assert_eq!(operation["x-rejectUnknownQueryParameters"], true);
    assert_eq!(operation["x-rejectDuplicateQueryParameters"], true);
}
