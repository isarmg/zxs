use super::*;
use json_codec::json;

#[test]
fn protection_wire_preserves_scoped_options_and_decimal_generations() {
    for scoped in [false, true] {
        let policy = storage_contract::ProtectionPolicy {
            policy_id: "policy".into(),
            repository_id: "repository".into(),
            path_prefix: "Folder".into(),
            target_copies: 3,
            inheritable: !scoped,
            scope_directory_id: scoped.then(|| "directory".into()),
            scope_file_id: scoped.then(|| "file".into()),
            backup_prefixes: if scoped {
                BTreeMap::from([("B01".into(), "Backup".into())])
            } else {
                BTreeMap::new()
            },
            generation: u64::MAX,
        };
        let expected = json_codec::to_value(&policy).unwrap();
        let response = PolicyResponse::from(policy);
        assert_eq!(json_codec::to_value(&response).unwrap(), expected);
        assert_eq!(
            storage_contract::canonical_json(&response).unwrap(),
            storage_contract::canonical_json(&expected).unwrap()
        );
        assert_eq!(expected.get("scope_directory_id").is_some(), scoped);
        assert_eq!(expected.get("backup_prefixes").is_some(), scoped);
        assert_eq!(expected["generation"], "18446744073709551615");
    }
    let rule = storage_contract::PlacementRule {
        rule_id: "rule".into(),
        repository_id: "repository".into(),
        path_prefix: "".into(),
        primary_slot_id: "P01".into(),
        generation: u64::MAX,
    };
    let expected = json_codec::to_value(&rule).unwrap();
    let response = PlacementRuleResponse::from(rule);
    assert_eq!(json_codec::to_value(&response).unwrap(), expected);
    assert_eq!(
        expected,
        json!({"rule_id":"rule","repository_id":"repository","path_prefix":"","primary_slot_id":"P01","generation":"18446744073709551615"})
    );
    assert_eq!(
        storage_contract::canonical_json(&response).unwrap(),
        storage_contract::canonical_json(&expected).unwrap()
    );
}
