use super::*;
use json_codec::{Value, json};

fn hash(number: u64) -> String {
    format!("{number:064x}")
}
fn identity(name: &str, role: &str) -> Value {
    json!({"origin":{"pool_id":"pool","disk_id":name,"slot_id":name,
        "binding_generation":"18446744073709551615"},"incarnation":hash(100),"role":role})
}
fn point(name: &str, role: &str, pin: u64) -> Value {
    json!({"identity":identity(name,role),"publication":hash(pin)})
}
fn sample() -> Value {
    json!({
        "protocol":"independent_source_topology",
        "root":{"identity":identity("third","tertiary"),"publication":hash(9),"sequence":"9007199254740993"},
        "nodes":[
            {"identity":identity("first","primary"),"pins":[hash(1),hash(2)]},
            {"identity":identity("second_a","secondary"),"pins":[hash(3)]},
            {"identity":identity("second_b","secondary"),"pins":[hash(4)]},
            {"identity":identity("third","tertiary"),"pins":[hash(9)]}
        ],
        "relations":[
            {"source":point("first","primary",1),"consumer":point("second_a","secondary",3)},
            {"source":point("first","primary",2),"consumer":point("second_b","secondary",4)},
            {"source":point("second_a","secondary",3),"consumer":point("third","tertiary",9)},
            {"source":point("second_b","secondary",4),"consumer":point("third","tertiary",9)}
        ]
    })
}
fn read(value: Value) -> Result<IndependentSourceTopology, json_codec::Error> {
    json_codec::from_value(value)
}

#[test]
fn source_graph_roundtrip_retains_each_publication_and_full_width_integer() {
    let value = sample();
    let graph = read(value.clone()).unwrap();
    assert_eq!(graph.root().sequence, 9007199254740993);
    assert_eq!(
        graph.nodes()[0].identity.origin.binding_generation.get(),
        u64::MAX
    );
    assert_eq!(graph.nodes()[0].pins.len(), 2);
    assert_eq!(graph.relations().len(), 4);
    assert_eq!(json_codec::to_value(graph).unwrap(), value);
}

#[test]
fn unpublished_disk_has_only_its_empty_root_node() {
    let mut value = sample();
    value["root"]["publication"] = Value::Null;
    value["root"]["sequence"] = "0".into();
    value["nodes"] = json!([{"identity":identity("third","tertiary"),"pins":[]}]);
    value["relations"] = json!([]);
    assert!(read(value.clone()).is_ok());
    value["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"identity":identity("first","primary"),"pins":[hash(1)]}));
    assert!(read(value).is_err());
}

#[test]
fn source_graph_rejects_detached_missing_and_duplicate_pins() {
    for change in 0..7 {
        let mut value = sample();
        match change {
            0 => {
                value["relations"].as_array_mut().unwrap().remove(0);
            }
            1 => value["nodes"][0]["pins"] = json!([hash(1), hash(1)]),
            2 => value["nodes"][0]["pins"] = json!([hash(2), hash(1)]),
            3 => value["nodes"][0]["pins"] = json!([hash(1)]),
            4 => value["nodes"][0]["pins"] = json!([hash(1), hash(2), hash(8)]),
            5 => {
                value["nodes"].as_array_mut().unwrap().remove(3);
            }
            6 => value["relations"][0]["source"]["publication"] = hash(9).into(),
            _ => unreachable!(),
        }
        assert!(read(value).is_err(), "change {change}");
    }
}

#[test]
fn source_graph_rejects_wrong_identity_incarnation_direction_and_duplicate_edges() {
    for change in 0..6 {
        let mut value = sample();
        match change {
            0 => value["relations"][0]["source"]["identity"]["incarnation"] = hash(99).into(),
            1 => {
                value["relations"][0]["source"]["identity"]["origin"]["pool_id"] =
                    "different".into()
            }
            2 => value["relations"][0]["source"]["identity"]["role"] = "tertiary".into(),
            3 => {
                let edge = value["relations"][0].clone();
                value["relations"].as_array_mut().unwrap().push(edge);
            }
            4 => {
                let node = value["nodes"][0].clone();
                value["nodes"].as_array_mut().unwrap().push(node);
            }
            5 => value["nodes"][3]["pins"] = json!([hash(8)]),
            _ => unreachable!(),
        }
        assert!(read(value).is_err(), "change {change}");
    }
}

#[test]
fn source_graph_rejects_authority_claims_and_missing_protocol_fields() {
    for field in ["online", "writable", "complete_discovery", "registration"] {
        let mut value = sample();
        value[field] = true.into();
        assert!(read(value).is_err());
    }
    let mut value = sample();
    value.as_object_mut().unwrap().remove("protocol");
    assert!(read(value).is_err());
    let mut value = sample();
    value["root"].as_object_mut().unwrap().remove("publication");
    assert!(read(value).is_err());
    let mut value = sample();
    value["protocol"] = json!({"independent_source_topology": null});
    assert!(read(value).is_err());
}

#[test]
fn source_graph_array_budgets_apply_during_deserialization() {
    for (field, count) in [
        ("nodes", MAX_TOPOLOGY_NODES),
        ("relations", MAX_TOPOLOGY_RELATIONS),
    ] {
        let mut value = sample();
        let row = value[field][0].clone();
        value[field] = Value::Array(vec![row; count + 1]);
        assert!(read(value).is_err());
    }
}

#[test]
fn published_preview_vectors_use_the_actual_rust_contract() {
    let vectors: Value =
        json_codec::from_str(include_str!("../../../docs/contracts/vectors.json")).unwrap();
    let mut checked = 0;
    for group in ["positive", "negative"] {
        for case in vectors[group].as_array().unwrap() {
            if !case["name"]
                .as_str()
                .unwrap()
                .starts_with("independent-preview-")
            {
                continue;
            }
            let value = case["value"].clone();
            let valid = match case["schema"].as_str().unwrap() {
                "IndependentDiskPreviewRequest" => {
                    json_codec::from_value::<IndependentDiskPreviewRequest>(value).is_ok()
                }
                "IndependentDiskPreview" => {
                    json_codec::from_value::<IndependentDiskPreview>(value).is_ok()
                }
                other => panic!("unexpected independent preview schema: {other}"),
            };
            assert_eq!(valid, group == "positive", "{}", case["name"]);
            checked += 1;
        }
    }
    assert_eq!(checked, 7);
}

#[test]
fn preview_requires_same_inventory_checkpoint_and_rejects_old_responses() {
    let vectors: Value =
        json_codec::from_str(include_str!("../../../docs/contracts/vectors.json")).unwrap();
    let sample = vectors["positive"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "independent-preview-exact-graph")
        .unwrap()["value"]
        .clone();
    assert!(json_codec::from_value::<IndependentDiskPreview>(sample.clone()).is_ok());
    for change in 0..4 {
        let mut value = sample.clone();
        match change {
            0 => {
                value.as_object_mut().unwrap().remove("inventory");
            }
            1 => value["inventory"]["root"]["publication"] = hash(77).into(),
            2 => value["inventory"]["root"]["sequence"] = "987".into(),
            3 => value["inventory"]["root"]["identity"]["incarnation"] = hash(77).into(),
            _ => unreachable!(),
        }
        assert!(
            json_codec::from_value::<IndependentDiskPreview>(value).is_err(),
            "change {change}"
        );
    }
}

#[test]
fn preview_cross_checks_inventory_owners_without_forgetting_retired_sources() {
    let vectors: Value =
        json_codec::from_str(include_str!("../../../docs/contracts/vectors.json")).unwrap();
    let mut valid = vectors["positive"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "independent-preview-exact-graph")
        .unwrap()["value"]
        .clone();
    let owner = valid["topology"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["identity"]["role"] == "primary")
        .unwrap()["identity"]
        .clone();
    valid["inventory"]["versions"] = json!([{
        "owner":owner,"history_id":"history","file_id":"file",
        "commit":hash(80),"body":hash(81)
    }]);
    assert!(json_codec::from_value::<IndependentDiskPreview>(valid.clone()).is_ok());
    for change in 0..2 {
        let mut changed = valid.clone();
        if change == 0 {
            changed["inventory"]["versions"][0]["owner"]["incarnation"] = hash(77).into();
        } else {
            changed["inventory"]["versions"][0]["owner"]["origin"] =
                changed["topology"]["root"]["identity"]["origin"].clone();
        }
        assert!(
            json_codec::from_value::<IndependentDiskPreview>(changed).is_err(),
            "conflicting known source owner {change}"
        );
    }
    let mut retired = valid;
    retired["inventory"]["versions"][0]["owner"]["origin"]["disk_id"] = "retired".into();
    retired["inventory"]["versions"][0]["owner"]["origin"]["slot_id"] = "retired".into();
    retired["inventory"]["versions"][0]["owner"]["incarnation"] = hash(77).into();
    assert!(json_codec::from_value::<IndependentDiskPreview>(retired).is_ok());
}
