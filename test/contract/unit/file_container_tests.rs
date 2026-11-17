//! Fixed shape vectors are not physical admission or directory capabilities.
use crate::file_container::{
    FileContainerRecord, NamespaceDirectoryRecord,
};
use crate::{State, canonical_json, decode_strict};
use json_codec::Value;

fn vectors() -> Value {
    decode_strict(include_bytes!("../../../docs/contracts/vectors.json")).unwrap()
}

fn positive(name: &str) -> Value {
    vectors()["positive"].as_array().unwrap().iter()
        .find(|case| case["name"] == name).unwrap()["value"].clone()
}

#[test]
fn ordinary_directory_vectors_preserve_independent_target_and_source_occurrences() {
    let value = positive("ordinary-directory-three-occurrences");
    let record: NamespaceDirectoryRecord = json_codec::from_value(value.clone()).unwrap();
    assert_eq!(json_codec::to_value(&record).unwrap(), value);
    assert_eq!(record.replicas.len(), 3);
    let disks: Vec<_> = record.replicas.keys().copied().collect();
    let original = &record.replicas[&disks[0]];
    let b = &record.replicas[&disks[1]];
    let c = &record.replicas[&disks[2]];
    assert!(original.source_chain.is_empty());
    assert!(original.source_prefixes.is_empty());
    assert_eq!(b.source_prefixes[0].source_disk, disks[0]);
    assert_eq!(c.source_prefixes[0].source_disk, disks[1]);
    assert_eq!(c.source_prefixes[1].source_disk, disks[0]);
    assert_ne!(b.source_prefixes[0].entity, c.source_prefixes[1].entity);
    assert_eq!(b.source_chain[0].disk_id, disks[0].to_string());
    assert_eq!(c.source_chain[0].disk_id, disks[1].to_string());
    assert_eq!(c.source_chain[1].disk_id, disks[0].to_string());
    assert_ne!(original.root_id, b.root_id);
    assert_ne!(b.root_id, c.root_id);
    assert_ne!(original.entity, b.entity);
    assert_ne!(b.entity, c.entity);
    assert_eq!(record.created_by, original.created_by);
    assert_ne!(record.created_by, b.created_by);
    assert_ne!(record.created_by, c.created_by);
    assert_eq!(decode_strict::<NamespaceDirectoryRecord>(&canonical_json(&record).unwrap())
        .unwrap(), record);
}

#[test]
fn ordinary_file_parent_and_directory_records_reject_missing_or_ambiguous_fields() {
    let value = positive("ordinary-file-parent-and-three-occurrences");
    let file: FileContainerRecord = json_codec::from_value(value.clone()).unwrap();
    let directory: NamespaceDirectoryRecord = json_codec::from_value(
        positive("ordinary-directory-three-occurrences"),
    ).unwrap();
    assert_eq!(file.namespace.parent,
        crate::file_container::ContainerParent::Directory(directory.entry));
    assert_eq!(file.owner_disk, directory.owner_disk);
    assert_eq!(json_codec::to_value(&file).unwrap(), value);
    for field in ["repository", "branch", "entry", "parent", "name", "owner_disk",
        "created_by", "replicas"]
    {
        let mut missing = json_codec::to_value(&directory).unwrap();
        missing.as_object_mut().unwrap().remove(field);
        assert!(json_codec::from_value::<NamespaceDirectoryRecord>(missing).is_err(), "{field}");
    }
    for field in ["root_id", "source_chain", "source_prefixes", "parent_entity", "preparation",
        "entity", "lifecycle", "created_by"]
    {
        let mut missing = json_codec::to_value(&directory).unwrap();
        let replica = missing["replicas"].as_object_mut().unwrap().values_mut().next().unwrap();
        replica.as_object_mut().unwrap().remove(field);
        assert!(json_codec::from_value::<NamespaceDirectoryRecord>(missing).is_err(), "{field}");
    }
    for case in vectors()["negative"].as_array().unwrap() {
        match case["schema"].as_str().unwrap() {
            "FileContainerRecord" => {
                assert!(json_codec::from_value::<FileContainerRecord>(case["value"].clone())
                    .is_err(), "{}", case["name"]);
            }
            "NamespaceDirectoryRecord" => {
                assert!(json_codec::from_value::<NamespaceDirectoryRecord>(case["value"].clone())
                    .is_err(), "{}", case["name"]);
            }
            _ => {}
        }
    }
}

#[test]
fn ordinary_state_requires_both_physical_maps_even_when_empty() {
    let state = State::default();
    let bytes = canonical_json(&state).unwrap();
    assert_eq!(decode_strict::<State>(&bytes).unwrap(), state);
    let value: Value = decode_strict(&bytes).unwrap();
    for field in ["file_containers", "namespace_directories"] {
        assert!(value[field].as_object().unwrap().is_empty());
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(json_codec::from_value::<State>(missing).is_err(), "{field}");
        let mut null = value.clone();
        null[field] = Value::Null;
        assert!(json_codec::from_value::<State>(null).is_err(), "{field}");
    }
}
