use super::*;
use json_codec::{Value, json};

fn sample() -> Value {
    let owner = json!({"origin":{"pool_id":"pool","disk_id":"primary","slot_id":"primary","binding_generation":"18446744073709551615"},"incarnation":"a".repeat(64),"role":"primary"});
    json!({"root":{"identity":owner.clone(),"publication":"b".repeat(64),"sequence":"1"},"versions":[{
        "owner":owner,"history_id":"history","file_id":"file","commit":"c".repeat(64),"body":"d".repeat(64)
    }]})
}
fn read(value: Value) -> Result<IndependentHistoryInventory, json_codec::Error> {
    json_codec::from_value(value)
}

#[test]
fn retained_inventory_roundtrips_without_content_availability_claims() {
    let value = sample();
    let inventory = read(value.clone()).unwrap();
    assert_eq!(inventory.versions().len(), 1);
    assert_eq!(
        inventory.versions()[0]
            .owner
            .origin
            .binding_generation
            .get(),
        u64::MAX
    );
    assert_eq!(json_codec::to_value(inventory).unwrap(), value);
}

#[test]
fn retained_inventory_rejects_unknown_old_and_conflicting_records() {
    for change in 0..10 {
        let mut value = sample();
        match change {
            0 => {
                value.as_object_mut().unwrap().remove("versions");
            }
            1 => value["versions"][0]["availability"] = "available".into(),
            2 => value["versions"][0]["owner"]["role"] = "secondary".into(),
            3 => value["versions"][0]["owner"]["incarnation"] = "e".repeat(64).into(),
            4 => value["versions"][0]["commit"] = "bad".into(),
            5 => value["versions"][0]["body"] = "F".repeat(64).into(),
            6 => value["versions"][0]["file_id"] = "../file".into(),
            7 => {
                value["root"]["publication"] = Value::Null;
                value["root"]["sequence"] = "0".into();
            }
            8 => {
                let row = value["versions"][0].clone();
                value["versions"].as_array_mut().unwrap().push(row);
            }
            9 => value["root"]["sequence"] = 1.into(),
            _ => unreachable!(),
        }
        assert!(read(value).is_err(), "change {change}");
    }
}

#[test]
fn backup_inventory_preserves_old_primary_identity_not_in_current_source_graph() {
    let mut value = sample();
    value["root"]["identity"]["role"] = "tertiary".into();
    value["root"]["identity"]["origin"]["disk_id"] = "tertiary".into();
    let mut second = value["versions"][0].clone();
    second["owner"]["origin"]["disk_id"] = "another_primary".into();
    second["commit"] = "e".repeat(64).into();
    value["versions"].as_array_mut().unwrap().push(second);
    assert_eq!(read(value.clone()).unwrap().versions().len(), 2);
    value["versions"][1]["owner"]["origin"]["disk_id"] = "primary".into();
    value["versions"][1]["owner"]["incarnation"] = "f".repeat(64).into();
    assert!(read(value).is_err());
}

#[test]
fn retained_inventory_enforces_decode_and_constructor_budgets() {
    let mut value = sample();
    let row = value["versions"][0].clone();
    value["versions"] = Value::Array(vec![row; MAX_RETAINED_VERSIONS + 1]);
    assert!(read(value).is_err());
    let inventory = read(sample()).unwrap();
    assert_eq!(
        IndependentHistoryInventory::new(
            inventory.root().clone(),
            vec![inventory.versions()[0].clone(); MAX_RETAINED_VERSIONS + 1]
        ),
        Err(InvalidObservation::HistoryBudget)
    );
}
