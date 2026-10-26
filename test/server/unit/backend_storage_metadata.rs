use super::*;
use json_codec::json;

#[test]
fn topology_wire_exposes_only_fixed_public_metadata() {
    let response = TopologyResponse::from(TopologySnapshot {
        generation: u64::MAX,
        slots: vec![
            TopologySlot {
                slot_id: "P01".into(),
                disk_id: "disk-primary".into(),
                level: 1,
                upstream_slot_id: None,
                metadata_member: true,
                binding_generation: 7,
            },
            TopologySlot {
                slot_id: "B01".into(),
                disk_id: "disk-backup".into(),
                level: 2,
                upstream_slot_id: Some("P01".into()),
                metadata_member: false,
                binding_generation: u64::MAX,
            },
        ],
    });
    let expected = json!({"topology_id":"default","generation":"18446744073709551615","slots":[
        {"slot_id":"P01","disk_id":"disk-primary","level":1,"upstream_slot_id":null,"metadata_member":true,"binding_generation":"7"},
        {"slot_id":"B01","disk_id":"disk-backup","level":2,"upstream_slot_id":"P01","metadata_member":false,"binding_generation":"18446744073709551615"}
    ]});
    assert_eq!(json_codec::to_value(&response).unwrap(), expected);
    assert_eq!(
        storage_contract::canonical_json(&response).unwrap(),
        storage_contract::canonical_json(&expected).unwrap()
    );
}
#[test]
fn placement_wire_preserves_frozen_routes_and_decimal_binding_map() {
    let placement = storage_contract::Placement {
        version_id: "version".into(),
        primary_slot_id: "P01".into(),
        target_copies: 2,
        route_slot_ids: vec!["P01".into(), "B01".into()],
        topology_generation: u64::MAX,
        binding_generations: BTreeMap::from([("P01".into(), 7), ("B01".into(), u64::MAX)]),
    };
    let expected = json_codec::to_value(&placement).unwrap();
    let response = PlacementResponse::from(placement);
    assert_eq!(json_codec::to_value(&response).unwrap(), expected);
    assert_eq!(
        expected["binding_generations"],
        json!({"B01":"18446744073709551615","P01":"7"})
    );
    assert_eq!(
        storage_contract::canonical_json(&response).unwrap(),
        storage_contract::canonical_json(&expected).unwrap()
    );
}
