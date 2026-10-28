use super::*;
use json_codec::json;
#[test]
fn capacity_roundtrip_requires_both_scopes_with_canonical_decimal_strings() {
    let measure = CapacityMeasure {
        used: 0,
        limit: u64::MAX,
        remaining: u64::MAX,
    };
    let scope = WorkspaceCapacityScope {
        records: measure.clone(),
        cold_records: 0,
        metadata_bytes: measure,
    };
    let value = WorkspaceCapacity {
        global: scope.clone(),
        principal: scope,
        warning: CapacityWarning::Ok,
        automatic_retirement: true,
    };
    let encoded = json_codec::to_value(&value).unwrap();
    assert_eq!(encoded["global"], encoded["principal"]);
    assert_eq!(
        encoded["principal"]["records"]["limit"],
        u64::MAX.to_string()
    );
    assert_eq!(
        json_codec::from_value::<WorkspaceCapacity>(encoded.clone()).unwrap(),
        value
    );
    for bad in [json!(1), json!("01"), json!("18446744073709551616")] {
        let mut malformed = encoded.clone();
        malformed["principal"]["records"]["used"] = bad;
        assert!(json_codec::from_value::<WorkspaceCapacity>(malformed).is_err());
    }
    let mut unknown = encoded.clone();
    unknown["principal"]["owner"] = json!("other user");
    assert!(json_codec::from_value::<WorkspaceCapacity>(unknown).is_err());
    let mut null_scope = encoded.clone();
    null_scope["global"] = json!(null);
    assert!(json_codec::from_value::<WorkspaceCapacity>(null_scope).is_err());
    let mut missing_scope = encoded.clone();
    missing_scope.as_object_mut().unwrap().remove("global");
    assert!(json_codec::from_value::<WorkspaceCapacity>(missing_scope).is_err());
    let mut automatic = encoded.clone();
    automatic["automatic_retirement"] = json!("true");
    assert!(json_codec::from_value::<WorkspaceCapacity>(automatic).is_err());
    let mut missing_cold = encoded.clone();
    missing_cold["principal"]
        .as_object_mut()
        .unwrap()
        .remove("cold_records");
    assert!(json_codec::from_value::<WorkspaceCapacity>(missing_cold).is_err());
    let mut unknown = encoded;
    unknown["warning"] = json!("unknown");
    assert!(json_codec::from_value::<WorkspaceCapacity>(unknown).is_err());
}
