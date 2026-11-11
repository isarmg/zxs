use super::*;
use json_codec::{Value, json};

fn wire() -> Value {
    json!({
        "current": {
            "repository_id": "repo_00000000000000000000000000000001",
            "directory_id": "entry_00000000000000000000000000000002",
            "directory_branch_id": "dbr_00000000000000000000000000000003",
            "working_tree_id": "dtree_00000000000000000000000000000004",
            "head_tree_id": "dtree_00000000000000000000000000000005",
            "index_generation": "9007199254740993",
            "namespace_head": "commit_00000000000000000000000000000006"
        },
        "tree_id": "dtree_00000000000000000000000000000007"
    })
}

#[test]
fn historical_source_roundtrips_current_cas_and_distinct_historical_tree() {
    let raw = wire();
    let source: HistoricalDirectoryBranchSource = json_codec::from_value(raw.clone()).unwrap();
    assert_eq!(source.current().index_generation, 9_007_199_254_740_993);
    assert_eq!(source.tree_id().to_string(), raw["tree_id"]);
    assert_ne!(source.tree_id().to_string(), source.current().head_tree_id);
    assert_eq!(json_codec::to_value(&source).unwrap(), raw);
}

#[test]
fn historical_source_rejects_missing_unknown_and_invalid_cas_fields() {
    for key in ["current", "tree_id"] {
        let mut raw = wire();
        raw.as_object_mut().unwrap().remove(key);
        assert!(json_codec::from_value::<HistoricalDirectoryBranchSource>(raw).is_err());
    }
    for (key, value) in [
        ("tree_id", json!("tree_00000000000000000000000000000007")),
        ("unknown", json!(true)),
    ] {
        let mut raw = wire();
        raw[key] = value;
        assert!(json_codec::from_value::<HistoricalDirectoryBranchSource>(raw).is_err());
    }
    for (key, value) in [
        ("index_generation", json!(0)),
        ("index_generation", json!("0")),
        ("index_generation", json!("01")),
        ("repository_id", json!("entry_00000000000000000000000000000001")),
        ("directory_id", json!("")),
        ("unknown", json!(true)),
    ] {
        let mut raw = wire();
        raw["current"][key] = value;
        assert!(json_codec::from_value::<HistoricalDirectoryBranchSource>(raw).is_err());
    }
}

#[test]
fn historical_source_constructor_admits_root_directory_and_large_generation() {
    let mut raw = wire();
    raw["current"]["directory_id"] = raw["current"]["repository_id"].clone();
    raw["current"]["index_generation"] = json!(u64::MAX.to_string());
    let source: HistoricalDirectoryBranchSource = json_codec::from_value(raw.clone()).unwrap();
    assert_eq!(json_codec::to_value(&source).unwrap(), raw);
    let mut current = source.current().clone();
    current.index_generation = 0;
    assert!(HistoricalDirectoryBranchSource::new(current, source.tree_id()).is_err());
}

#[test]
fn historical_source_fixed_vectors_match_strict_wire_type() {
    let vectors: Value =
        json_codec::from_str(include_str!("../../../docs/contracts/vectors.json")).unwrap();
    let mut counts = [0, 0];
    for (index, group) in ["positive", "negative"].into_iter().enumerate() {
        for vector in vectors[group].as_array().unwrap() {
            if vector["schema"] != "HistoricalDirectoryBranchSource" {
                continue;
            }
            let decoded = json_codec::from_value::<HistoricalDirectoryBranchSource>(
                vector["value"].clone(),
            );
            if index == 0 {
                assert_eq!(json_codec::to_value(decoded.unwrap()).unwrap(), vector["value"]);
            } else {
                assert!(decoded.is_err(), "{}", vector["name"]);
            }
            counts[index] += 1;
        }
    }
    assert_eq!(counts, [1, 4]);
}
