use super::*;
use json_codec::json;

#[test]
fn event_page_wire_retains_open_kind_and_decimal_cursors() {
    let event = storage_contract::Event {
        sequence: u64::MAX,
        operation_id: "op_00000000000000000000000000000000".parse().unwrap(),
        kind: "extension.kind".into(),
        object_id: "object".into(),
    };
    let expected = json!({"events":[event.clone()],"next_cursor":u64::MAX.to_string()});
    let response = EventPageResponse::from(EventPage {
        events: vec![event],
        next_cursor: u64::MAX,
    });
    assert_eq!(json_codec::to_value(&response).unwrap(), expected);
    assert_eq!(storage_contract::canonical_json(&response).unwrap(),br#"{"events":[{"kind":"extension.kind","object_id":"object","operation_id":"op_00000000000000000000000000000000","sequence":"18446744073709551615"}],"next_cursor":"18446744073709551615"}"#);
    assert_eq!(
        json_codec::to_value(EventPageResponse::from(EventPage {
            events: vec![],
            next_cursor: 17
        }))
        .unwrap(),
        json!({"events":[],"next_cursor":"17"})
    );
}
