use super::*;
use storage_contract::file_lifecycle::{Cursor, MAX_EVENTS, Page, Request};

#[test]
fn lifecycle_published_contract_rejects_prior_wire_and_invalid_fact_evidence() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut core = read(&root.join("docs/contracts/core.schema.json")).unwrap();
    let mut api = read(&root.join("docs/contracts/openapi.json")).unwrap();
    let vectors = read(&root.join("docs/contracts/vectors.json")).unwrap();
    assert_eq!(api["components"]["schemas"], bundle(&core["$defs"]));
    let mut positives = 0;
    let mut negatives = 0;
    let mut prior_shapes = BTreeSet::new();
    for name in ["FileLifecyclePage", "FileLifecycleCursor"] {
        core["$ref"] = format!("#/$defs/{name}").into();
        api["$ref"] = format!("#/components/schemas/{name}").into();
        let core_schema = validator(&core).unwrap();
        let api_schema = validator(&api).unwrap();
        for group in ["positive", "negative"] {
            for case in array(&vectors[group])
                .unwrap()
                .iter()
                .filter(|case| case["schema"] == name)
            {
                let label = case["name"].as_str().unwrap();
                let valid = group == "positive";
                let shape_valid = valid || case["layer"] == "semantic";
                assert_eq!(core_schema.is_valid(&case["value"]), shape_valid, "{label}");
                assert_eq!(api_schema.is_valid(&case["value"]), shape_valid, "{label}");
                // Even schema-invalid examples must fail real typed admission.
                assert_eq!(semantics(name, &case["value"]).is_ok(), valid, "{label}");
                if valid {
                    positives += 1;
                } else {
                    negatives += 1;
                }
                if label.starts_with("lifecycle-prior-") {
                    assert!(!valid);
                    assert_eq!(case["layer"], "schema");
                    assert!(prior_shapes.insert(label));
                }
            }
        }
    }
    assert_eq!((positives, negatives), (3, 13));
    assert_eq!(
        prior_shapes,
        BTreeSet::from([
            "lifecycle-prior-flat-event",
            "lifecycle-prior-three-part-cursor",
        ]),
    );
    let get = &api["paths"]["/api/file-manager/files/{file}/history/lifecycle"]["get"];
    assert_eq!(
        get["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/FileLifecyclePage",
    );
    assert_eq!(array(&get["parameters"]).unwrap().len(), 6);
    assert_eq!(get["x-maxResponseBytes"], 65536);
    assert_eq!(get["x-rejectUnknownQueryParameters"], true);
    assert_eq!(get["x-rejectDuplicateQueryParameters"], true);
}

#[test]
fn lifecycle_short_pages_bind_the_exact_member_without_forging_a_final_page() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut core = read(&root.join("docs/contracts/core.schema.json")).unwrap();
    core["$ref"] = "#/$defs/FileLifecyclePage".into();
    let schema = validator(&core).unwrap();
    let vectors = read(&root.join("docs/contracts/vectors.json")).unwrap();
    let example = array(&vectors["positive"])
        .unwrap()
        .iter()
        .find(|case| case["name"] == "lifecycle-path-members-and-delete-undo-import")
        .unwrap();
    let mut page: Page = json_codec::from_value(example["value"].clone()).unwrap();
    let request = Request::parse(
        page.file_id.as_str(),
        &page.history_root.to_string(),
        None,
        None,
        None,
        MAX_EVENTS,
    )
    .unwrap();
    let next_events = page.events.split_off(1);
    let first = &page.events[0];
    page.next_cursor = Some(Cursor {
        binding: request.binding(&page.context, page.observation_sequence),
        after_operation_id: first.operation_id,
        after_sequence: first.sequence,
        after_source_id: first.source_id(),
    });
    let wire = json_codec::to_value(&page).unwrap();
    assert!(schema.is_valid(&wire));
    semantics("FileLifecyclePage", &wire).unwrap();
    let mut next_request = request.clone();
    next_request.cursor = page.next_cursor.clone();
    let mut next = page.clone();
    next.events = next_events;
    next.next_cursor = None;
    assert_eq!(next.events[0].operation_id, first.operation_id);
    assert_eq!(next.events[0].sequence, first.sequence);
    assert_ne!(next.events[0].source_id(), first.source_id());
    next.validate(&next_request).unwrap();
    assert!(schema.is_valid(&json_codec::to_value(&next).unwrap()));

    page.next_cursor.as_mut().unwrap().after_source_id = next.events[0].source_id();
    assert!(semantics("FileLifecyclePage", &json_codec::to_value(&page).unwrap()).is_err());
    page.events.clear();
    assert!(semantics("FileLifecyclePage", &json_codec::to_value(&page).unwrap()).is_err());
}
