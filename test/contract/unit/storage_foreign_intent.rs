use super::*;
use data_serialization::de::DeserializeOwned;
use json_codec::Value;
fn roundtrip<T: DeserializeOwned + Serialize>(value: Value) -> Result<Value, json_codec::Error> {
    json_codec::to_value(json_codec::from_value::<T>(value)?)
}
#[test]
fn foreign_intent_fixed_vectors_keep_explicit_save_scope_and_target_local_addresses() {
    let vectors: Value =
        json_codec::from_str(include_str!("../../../docs/contracts/vectors.json")).unwrap();
    let mut counts = [0, 0];
    for (index, group) in ["positive", "negative"].into_iter().enumerate() {
        for vector in vectors[group].as_array().unwrap() {
            let name = vector["name"].as_str().unwrap();
            if !name.starts_with("foreign-intent-") {
                continue;
            }
            let value = vector["value"].clone();
            let decoded = match vector["schema"].as_str().unwrap() {
                "ForeignIntentRequest" => roundtrip::<ForeignIntentRequest>(value),
                "ForeignIntentPreview" => roundtrip::<ForeignIntentPreview>(value),
                "ForeignIntentSave" => roundtrip::<ForeignIntentSave>(value),
                "ForeignIntentQuery" => roundtrip::<ForeignIntentQuery>(value),
                "ForeignIntentStatus" => roundtrip::<ForeignIntentStatus>(value),
                _ => panic!("unhandled foreign intent schema"),
            };
            if index == 0 {
                assert_eq!(decoded.unwrap(), vector["value"], "{name}");
            } else {
                assert!(decoded.is_err(), "accepted {name}");
            }
            counts[index] += 1;
        }
    }
    assert_eq!(counts, [5, 6]);
}
