use super::*;
use crate::http_boundary::ApiErrorSource;
use storage_contract::ErrorCode;

#[test]
fn destination_retains_slot_parent_and_unadmitted_history_names() {
    for slot in ["P01".to_owned(), "disk_01-X".into(), "A".repeat(64)] {
        for parent in [None, Some("Café".to_owned())] {
            let names = std::collections::BTreeMap::from([
                ("group-a".into(), "Straße".into()),
                ("group-b".into(), "CON.txt".into()),
            ]);
            let wire = ForeignPlanTarget {
                slot_id: slot.clone(),
                parent: parent.clone(),
                history_names: names.clone(),
            };
            let destination = ForeignDestination::try_from(&wire).unwrap();
            assert_eq!(destination.slot.as_str(), slot);
            assert_eq!(
                destination.parent.as_ref().map(|path| path.as_str()),
                parent.as_deref()
            );
            assert_eq!(destination.names, names);
        }
    }
}

#[test]
fn destination_admission_preserves_slot_before_parent_error_order() {
    for (slot, parent, expected) in [
        ("".to_owned(), "../bad", ErrorCode::InvalidSlotId),
        ("0disk".into(), "../bad", ErrorCode::InvalidSlotId),
        ("A".repeat(65), "../bad", ErrorCode::InvalidSlotId),
        ("é".into(), "../bad", ErrorCode::InvalidSlotId),
        ("P01".into(), "../bad", ErrorCode::NameInvalidCharacter),
        ("P01".into(), "Cafe\u{301}", ErrorCode::NameNotNormalized),
    ] {
        let wire = ForeignPlanTarget {
            slot_id: slot,
            parent: Some(parent.into()),
            history_names: std::collections::BTreeMap::new(),
        };
        let error = ForeignDestination::try_from(&wire).err().unwrap();
        assert_eq!(error.status, ::http::StatusCode::BAD_REQUEST);
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        let ApiErrorSource::Contract(source) = error.source.as_deref().unwrap() else {
            panic!("expected the naming policy source");
        };
        assert_eq!(source.code(), expected);
    }
}
