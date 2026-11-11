use crate::{
    ErrorCode, Generation, GenerationExhausted, OperationId, Sequence, SequenceExhausted,
    canonical_json,
};
use data_serialization::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[test]
fn counters_preserve_full_width_decimal_strings() {
    for value in [
        0,
        1,
        (1 << 53) - 1,
        1 << 53,
        (1 << 53) + 1,
        u64::MAX - 1,
        u64::MAX,
    ] {
        let text = value.to_string();
        let bytes = format!("\"{text}\"");
        let generation = Generation::new(value);
        let sequence = Sequence::new(value);
        assert_eq!(generation.get(), value);
        assert_eq!(sequence.get(), value);
        assert_eq!(generation.to_string(), text);
        assert_eq!(sequence.to_string(), text);
        assert_eq!(text.parse::<Generation>().unwrap(), generation);
        assert_eq!(text.parse::<Sequence>().unwrap(), sequence);
        assert_eq!(canonical_json(&generation).unwrap(), bytes.as_bytes());
        assert_eq!(canonical_json(&sequence).unwrap(), bytes.as_bytes());
        assert_eq!(
            json_codec::from_str::<Generation>(&bytes).unwrap(),
            generation
        );
        assert_eq!(json_codec::from_str::<Sequence>(&bytes).unwrap(), sequence);
    }
}

#[test]
fn counter_parsers_reject_noncanonical_decimal_text_and_other_json_types() {
    for text in [
        "",
        "00",
        "01",
        "+1",
        "-1",
        " 1",
        "1 ",
        "1\n",
        "1.0",
        "1e0",
        "１８",
        "18446744073709551616",
        "100000000000000000000",
    ] {
        assert_eq!(
            text.parse::<Generation>().unwrap_err().code(),
            ErrorCode::InvalidInteger
        );
        assert_eq!(
            text.parse::<Sequence>().unwrap_err().code(),
            ErrorCode::InvalidInteger
        );
        let bytes = json_codec::to_string(text).unwrap();
        assert!(json_codec::from_str::<Generation>(&bytes).is_err());
        assert!(json_codec::from_str::<Sequence>(&bytes).is_err());
    }
    for bytes in ["0", "1", "-1", "1.0", "true", "null", "[]", "{}"] {
        assert!(json_codec::from_str::<Generation>(bytes).is_err());
        assert!(json_codec::from_str::<Sequence>(bytes).is_err());
    }
}

#[test]
fn counter_successors_report_exhaustion_at_the_representable_maximum() {
    for value in [0, 1, (1 << 53) - 1, u64::MAX - 1] {
        assert_eq!(
            Generation::new(value).next().unwrap(),
            Generation::new(value + 1)
        );
        assert_eq!(
            Sequence::new(value).next().unwrap(),
            Sequence::new(value + 1)
        );
    }
    assert_eq!(Generation::new(u64::MAX).next(), Err(GenerationExhausted));
    assert_eq!(Sequence::new(u64::MAX).next(), Err(SequenceExhausted));
}

#[derive(Serialize, Deserialize)]
struct Receipt {
    sequence: Sequence,
    #[serialization(flatten)]
    fields: BTreeMap<String, json_codec::Value>,
}

#[test]
fn published_receipt_vector_roundtrips_through_typed_sequence() {
    let bytes: &[u8] = include_bytes!("../fixtures/operation-receipts.json");
    let receipts: BTreeMap<OperationId, Receipt> = json_codec::from_slice(bytes).unwrap();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts.values().next().unwrap().sequence, Sequence::new(7));
    assert_eq!(canonical_json(&receipts).unwrap(), bytes);
}

#[derive(Serialize, Deserialize)]
struct SmbAccount {
    generation: Generation,
    #[serialization(flatten)]
    fields: BTreeMap<String, json_codec::Value>,
}

#[test]
fn published_smb_account_vector_preserves_maximum_generation_bytes() {
    // docs/contracts/vectors.json: positive/smb-configured-available.
    let bytes = concat!(
        r#"{"candidates":["fixture-smb","fixture-next"],"#,
        r#""generation":"18446744073709551615","password_change":"available","#,
        r#""state":"configured","username":"fixture-smb"}"#,
    );
    let account: SmbAccount = json_codec::from_str(bytes).unwrap();
    assert_eq!(account.generation, Generation::new(u64::MAX));
    assert_eq!(canonical_json(&account).unwrap(), bytes.as_bytes());
}
