use data_serialization::{Deserialize, Serialize};
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serialization(rename_all = "camelCase")]
pub struct Sample {
    pub field_name: String,
}
pub mod reexport {
    pub use data_serialization::*;
}
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serialization(crate = "crate::reexport")]
pub struct Custom {
    pub field: String,
}
#[derive_where::derive_where(Serialize, Deserialize)]
#[serialization(crate = "data_serialization")]
pub struct Wrapped<T> {
    pub field: core::marker::PhantomData<T>,
}
#[test]
fn round_trip() {
    let v = Sample {
        field_name: "hello world".into(),
    };
    let json = json_codec::to_string(&v).unwrap();
    assert_eq!(json, r#"{"fieldName":"hello world"}"#);
    assert_eq!(json_codec::from_str::<Sample>(&json).unwrap(), v);
    let form = form_codec::to_string(&v).unwrap();
    assert_eq!(form, "fieldName=hello+world");
    assert_eq!(form_codec::from_str::<Sample>(&form).unwrap(), v);
    let c = Custom { field: "ok".into() };
    assert_eq!(
        json_codec::from_str::<Custom>(&json_codec::to_string(&c).unwrap()).unwrap(),
        c
    );
    let raw = json_codec::value::RawValue::from_string("{\"a\":1}".into()).unwrap();
    assert_eq!(json_codec::to_string(&raw).unwrap(), "{\"a\":1}");
    let n: json_codec::Value = json_codec::from_str("123456789012345678901234567890").unwrap();
    assert_eq!(
        json_codec::to_string(&n).unwrap(),
        "123456789012345678901234567890"
    );
}

#[cfg(test)]
#[derive(Debug, Deserialize, PartialEq)]
struct NamedRecord {
    value: u32,
}

#[cfg(test)]
fn assert_named_record_rejected<T: data_serialization::de::DeserializeOwned>(text: &str) {
    assert!(json_codec::from_str::<T>(text).is_err(), "text: {text}");
    assert!(
        json_codec::from_slice::<T>(text.as_bytes()).is_err(),
        "bytes: {text}"
    );
    assert!(
        json_codec::from_reader::<_, T>(text.as_bytes()).is_err(),
        "reader: {text}"
    );
    let value: json_codec::Value = json_codec::from_str(text).unwrap();
    assert!(T::deserialize(&value).is_err(), "borrowed value: {text}");
    assert!(
        json_codec::from_value::<T>(value).is_err(),
        "owned value: {text}"
    );
}

#[test]
fn named_struct_json_requires_objects_in_all_decoder_entrypoints() {
    #[derive(Deserialize)]
    #[allow(dead_code)]
    struct Container {
        optional: Option<NamedRecord>,
        sequence: Vec<NamedRecord>,
        mapping: std::collections::BTreeMap<String, NamedRecord>,
    }
    #[derive(Deserialize)]
    #[allow(dead_code)]
    enum Variant {
        Named { value: u32 },
    }
    assert_named_record_rejected::<NamedRecord>("[1]");
    assert_named_record_rejected::<Option<NamedRecord>>("[1]");
    assert_named_record_rejected::<Vec<NamedRecord>>("[[1]]");
    for text in [
        r#"{"optional":[1],"sequence":[],"mapping":{}}"#,
        r#"{"optional":null,"sequence":[[1]],"mapping":{}}"#,
        r#"{"optional":null,"sequence":[],"mapping":{"record":[1]}}"#,
    ] {
        assert_named_record_rejected::<Container>(text);
    }
    assert_named_record_rejected::<Variant>(r#"{"Named":[1]}"#);
    assert_eq!(
        json_codec::from_str::<NamedRecord>(r#"{"value":1}"#).unwrap(),
        NamedRecord { value: 1 }
    );
    assert!(
        json_codec::from_str::<Container>(
            r#"{"optional":{"value":1},"sequence":[{"value":2}],"mapping":{"record":{"value":3}}}"#
        )
        .is_ok()
    );
    assert!(json_codec::from_str::<Variant>(r#"{"Named":{"value":1}}"#).is_ok());
}

#[test]
fn ordinary_json_sequences_and_other_struct_representations_are_unchanged() {
    #[derive(Debug, Deserialize, PartialEq)]
    struct TupleRecord(u32, String);
    #[derive(Debug, Deserialize, PartialEq)]
    struct Newtype(Vec<u32>);
    #[derive(Debug, Deserialize, PartialEq)]
    struct Unit;
    #[derive(Debug, Deserialize, PartialEq)]
    enum Variant {
        Tuple(u32, u32),
        Newtype(Vec<u32>),
        Unit,
    }
    fn all_entrypoints<
        T: data_serialization::de::DeserializeOwned + PartialEq + std::fmt::Debug,
    >(
        text: &str,
        expected: T,
    ) {
        assert_eq!(json_codec::from_str::<T>(text).unwrap(), expected);
        assert_eq!(
            json_codec::from_slice::<T>(text.as_bytes()).unwrap(),
            expected
        );
        assert_eq!(
            json_codec::from_reader::<_, T>(text.as_bytes()).unwrap(),
            expected
        );
        let value: json_codec::Value = json_codec::from_str(text).unwrap();
        assert_eq!(T::deserialize(&value).unwrap(), expected);
        assert_eq!(json_codec::from_value::<T>(value).unwrap(), expected);
    }
    all_entrypoints("[1,2]", vec![1_u32, 2]);
    all_entrypoints("[1,2]", [1_u32, 2]);
    all_entrypoints(r#"[1,"two"]"#, (1_u32, String::from("two")));
    all_entrypoints(r#"[1,"two"]"#, TupleRecord(1, "two".into()));
    all_entrypoints("[1,2]", Newtype(vec![1, 2]));
    all_entrypoints("null", Unit);
    all_entrypoints(r#"{"Tuple":[1,2]}"#, Variant::Tuple(1, 2));
    all_entrypoints(r#"{"Newtype":[1,2]}"#, Variant::Newtype(vec![1, 2]));
    all_entrypoints(r#""Unit""#, Variant::Unit);
    all_entrypoints("[1,[2]]", json_codec::json!([1, [2]]));
    all_entrypoints(r#"{"value":1}"#, NamedRecord { value: 1 });
}
#[test]
fn protocol_tokens_and_error_path() {
    use toml::span::__unstable as wire;
    assert_eq!(wire::NAME, "$__serde_spanned_private_Spanned");
    assert_eq!(wire::START_FIELD, "$__serde_spanned_private_start");
    assert_eq!(wire::END_FIELD, "$__serde_spanned_private_end");
    assert_eq!(wire::VALUE_FIELD, "$__serde_spanned_private_value");
    let mut de = json_codec::Deserializer::from_str(r#"{"fieldName":1}"#);
    let err = data_serialization::error_path::deserialize::<_, Sample>(&mut de).unwrap_err();
    assert_eq!(err.path().to_string(), "fieldName");
}

flag_set::bitflags! {
 #[derive(Serialize,Deserialize,Debug,PartialEq)]
 #[serialization(transparent)]
 pub struct Bits: u32 { const A = 1; }
}
#[test]
fn bitflag_reexport() {
    let json = json_codec::to_string(&Bits::A).unwrap();
    assert_eq!(json, "\"A\"");
    assert_eq!(json_codec::from_str::<Bits>(&json).unwrap(), Bits::A);
}

#[test]
fn date_time_human_readable_round_trip_uses_owned_capability() {
    let value = date_time::OffsetDateTime::from_unix_timestamp_nanos(-1)
        .unwrap()
        .to_offset(date_time::UtcOffset::from_hms(5, 30, 0).unwrap());
    let json = json_codec::to_string(&value).unwrap();
    let restored = json_codec::from_str::<date_time::OffsetDateTime>(&json).unwrap();
    assert_eq!(restored.unix_timestamp_nanos(), -1);
    assert_eq!(restored.offset(), value.offset());
}
