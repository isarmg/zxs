use super::*;
use storage_contract::content_matches::MAX_CONTENT_MATCHES;

#[test]
fn content_matches_vectors_follow_shared_wire_and_published_route() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut core = read(&root.join("docs/contracts/core.schema.json")).unwrap();
    let mut api = read(&root.join("docs/contracts/openapi.json")).unwrap();
    let vectors = read(&root.join("docs/contracts/vectors.json")).unwrap();
    assert_eq!(api["components"]["schemas"], bundle(&core["$defs"]));
    core["$ref"] = "#/$defs/ContentMatchPage".into();
    api["$ref"] = "#/components/schemas/ContentMatchPage".into();
    let core_validator = validator(&core).unwrap();
    let api_validator = validator(&api).unwrap();
    let mut count = 0;
    for group in ["positive", "negative"] {
        for case in array(&vectors[group])
            .unwrap()
            .iter()
            .filter(|case| case["schema"] == "ContentMatchPage")
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
                    semantics("ContentMatchPage", &case["value"]).is_ok(),
                    group == "positive",
                    "{}",
                    case["name"]
                );
            }
        }
    }
    assert_eq!(count, 6);
    let operation = &api["paths"]["/api/file-manager/files/{file}/content-matches"]["get"];
    assert_eq!(
        operation["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/ContentMatchPage"
    );
    let limit = array(&operation["parameters"])
        .unwrap()
        .iter()
        .find(|parameter| parameter["name"] == "limit")
        .unwrap();
    assert_eq!(limit["schema"]["maximum"], MAX_CONTENT_MATCHES);
    assert_eq!(limit["required"], true);
    assert_eq!(operation["x-rejectUnknownQueryParameters"], true);
    assert_eq!(operation["x-rejectDuplicateQueryParameters"], true);
}
