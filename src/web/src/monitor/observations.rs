use data_serialization::{Deserialize, Deserializer};
use json_codec::Value;
use std::collections::BTreeMap;

pub(super) use storage_contract::DiskObservationIssue;

pub(super) type DiskIssues = BTreeMap<String, Option<DiskObservationIssue>>;

#[derive(Deserialize)]
struct Snapshot {
    disks: Vec<Disk>,
}
#[derive(Deserialize)]
struct Disk {
    id: String,
    #[serialization(deserialize_with = "nullable")]
    observation_failure: Option<DiskObservationIssue>,
}
fn nullable<'de, D: Deserializer<'de>>(d: D) -> Result<Option<DiskObservationIssue>, D::Error> {
    Option::deserialize(d)
}

pub(super) fn decode(value: &Value) -> Result<DiskIssues, json_codec::Error> {
    let snapshot = Snapshot::deserialize(value)?;
    let mut issues = BTreeMap::new();
    for disk in snapshot.disks {
        if disk.id.is_empty() || issues.insert(disk.id, disk.observation_failure).is_some() {
            return Err(data_serialization::de::Error::custom(
                "duplicate or empty disk id",
            ));
        }
    }
    Ok(issues)
}
