use super::*;

const ROUTE: &str = "/api/repositories/{repository}/placement-configuration";

fn fixture(name: &str) -> Value {
    read(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../test/tools/contract-check/fixtures/placement-configuration")
            .join(name),
    )
    .unwrap()
}

fn schemas(name: &str, method: &str) -> Vec<schema_validation::Validator> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/contracts");
    let mut core = read(&root.join("core.schema.json")).unwrap();
    let mut api = read(&root.join("openapi.json")).unwrap();
    let mut route = api.clone();
    core["$ref"] = format!("#/$defs/{name}").into();
    api["$ref"] = format!("#/components/schemas/{name}").into();
    let body = if method == "get" {
        &route["paths"][ROUTE][method]["responses"]["200"]["content"]["application/json"]["schema"]
    } else {
        &route["paths"][ROUTE][method]["requestBody"]["content"]["application/json"]["schema"]
    };
    assert_eq!(body["$ref"], format!("#/components/schemas/{name}"));
    route["allOf"] = json!([body]);
    [&core, &api, &route]
        .into_iter()
        .map(|value| validator(value).unwrap())
        .collect()
}

#[test]
fn placement_configuration_read_schemas_validate_real_dto_fixtures_strictly() {
    for schema in schemas("PlacementConfigurationResponse", "get") {
        for name in ["read-root.json", "read-directory.json"] {
            let body = fixture(name);
            schema
                .validate(&body)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            for pointer in ["", "/scope", "/targets/0"] {
                let object = body.pointer(pointer).unwrap().as_object().unwrap();
                for required in object.keys() {
                    let mut missing = body.clone();
                    missing
                        .pointer_mut(pointer)
                        .unwrap()
                        .as_object_mut()
                        .unwrap()
                        .remove(required);
                    assert!(
                        !schema.is_valid(&missing),
                        "{name}: missing {pointer}/{required}"
                    );
                }
                let mut unknown = body.clone();
                unknown
                    .pointer_mut(pointer)
                    .unwrap()
                    .as_object_mut()
                    .unwrap()
                    .insert("unknown".into(), json!(true));
                assert!(
                    !schema.is_valid(&unknown),
                    "{name}: unknown field at {pointer}"
                );
            }
        }
        let body = fixture("read-directory.json");
        for (pointer, value) in [
            ("/repository_id", json!(null)),
            ("/repository_name", json!(null)),
            ("/scope/id", json!("file_00000000000000000000000000000002")),
            ("/scope/path", json!(null)),
            ("/rule_id", json!("")),
            ("/rule_id", json!(0)),
            ("/generation", json!(0)),
            ("/generation", json!("01")),
            ("/generation", json!("18446744073709551616")),
            ("/primary_slot_id", json!("")),
            ("/inherited", json!("false")),
            ("/required_copies", json!(0)),
            ("/required_copies", json!("3")),
            ("/required_copies", json!(4294967296_u64)),
            ("/targets", json!(null)),
            ("/targets/0/root", json!(null)),
            ("/targets/0/root", json!("")),
            ("/targets/0/unavailable_reason", json!("unknown")),
            ("/targets/0/unavailable_reason", json!(false)),
        ] {
            let mut invalid = body.clone();
            *invalid.pointer_mut(pointer).unwrap() = value;
            assert!(
                !schema.is_valid(&invalid),
                "accepted invalid {pointer}: {invalid}"
            );
        }
    }
}

#[test]
fn placement_configuration_write_schemas_match_real_dto_boundary_cases() {
    let invalid_cases = fixture("invalid-write-fields.json");
    let invalid_cases = invalid_cases.as_array().unwrap();
    assert_eq!(invalid_cases.len(), 25);
    for schema in schemas("ConfigurePlacementRequest", "put") {
        for name in ["create.json", "update.json"] {
            let body = fixture(name);
            schema
                .validate(&body)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            for field in body.as_object().unwrap().keys() {
                let mut missing = body.clone();
                missing.as_object_mut().unwrap().remove(field);
                assert!(!schema.is_valid(&missing), "{name}: missing {field}");
            }
            for case in invalid_cases {
                let field = case["field"].as_str().unwrap();
                let mut invalid = body.clone();
                invalid[field] = case["value"].clone();
                assert!(!schema.is_valid(&invalid), "{name}: {case}");
            }
            let mut explicit = body;
            explicit["repository_id"] = "repo_00000000000000000000000000000001".into();
            assert!(schema.is_valid(&explicit));
        }
    }
}

#[test]
fn placement_configuration_openapi_preserves_authentication_and_conflict_errors() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/contracts/openapi.json");
    let api = read(&path).unwrap();
    for method in ["get", "put"] {
        let operation = &api["paths"][ROUTE][method];
        assert_eq!(
            operation.get("security").unwrap_or(&api["security"]),
            &json!([{"bearerAuth": []}, {"sessionCookie": []}])
        );
        let parameters = operation["parameters"].as_array().unwrap();
        let repository = parameters
            .iter()
            .find(|parameter| parameter["name"] == "repository")
            .unwrap();
        assert_eq!(repository["in"], "path");
        assert_eq!(repository["required"], true);
        assert_eq!(
            repository["schema"]["$ref"],
            "#/components/schemas/RepositoryId"
        );
        for status in ["401", "403"] {
            assert_eq!(
                operation["responses"][status]["content"]["application/json"]["schema"]["$ref"],
                "#/components/schemas/ErrorResponse"
            );
        }
    }
    let put = &api["paths"][ROUTE]["put"];
    assert_eq!(put["requestBody"]["required"], true);
    assert_eq!(
        put["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/GenericOperation"
    );
    let conflict = &put["responses"]["409"];
    let mut schema = api.clone();
    schema["allOf"] = json!([conflict["content"]["application/json"]["schema"]]);
    let schema = validator(&schema).unwrap();
    for code in ["CONFIG_CONFLICT", "SCOPE_CHANGED"] {
        assert!(conflict["description"].as_str().unwrap().contains(code));
        schema
            .validate(&json!({"error":{
                "code":code, "message":"configuration changed", "retryable":false,
                "request_id":"req_00000000000000000000000000000001"
            }}))
            .unwrap();
    }
}
