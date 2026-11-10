use data_serialization::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Capture {
    format: Format,
    #[serialization(with = "storage_contract::u64_string")]
    source_sequence: u64,
    #[serialization(with = "storage_contract::u64_string")]
    source_business_generation: u64,
    registry: Registry,
    disks: BTreeMap<String, Disk>,
}
#[derive(Clone, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "recursive-backup-source-paths")]
    SourcePaths,
}
#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Registry {
    root_hash: String,
    #[serialization(with = "storage_contract::u64_string")]
    view_generation: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Disk {
    identity: Identity,
    level: u32,
    #[serialization(deserialize_with = "required_upstream")]
    upstream_slot_id: Option<String>,
    #[serialization(deserialize_with = "disk_name")]
    name: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Identity {
    pool_id: String,
    disk_id: String,
    slot_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    binding_generation: u64,
}
fn required_upstream<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Option::deserialize(d)
}
fn disk_name<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let name = String::deserialize(d)?;
    name_policy::validate_name(&name).map_err(data_serialization::de::Error::custom)?;
    Ok(name)
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Prefix {
    pub(super) copies: u32,
    pub(super) path: String,
}
impl Capture {
    pub(super) fn prefixes(
        &self,
        routes: &[Vec<String>],
    ) -> Result<BTreeMap<String, Prefix>, &'static str> {
        let mut result = BTreeMap::new();
        for route in routes {
            let mut previous: Option<&Disk> = None;
            let mut prefix = String::new();
            for slot in route {
                let disk = self.disks.get(slot).ok_or("missing captured route disk")?;
                if disk.identity.slot_id != *slot {
                    return Err("captured disk identity does not match the route slot");
                }
                if let Some(source) = previous {
                    if source.level.checked_add(1) != Some(disk.level)
                        || disk.upstream_slot_id.as_deref()
                            != Some(source.identity.slot_id.as_str())
                        || disk.identity.pool_id != source.identity.pool_id
                    {
                        return Err("route does not follow the captured immediate source");
                    }
                    prefix = if prefix.is_empty() {
                        source.name.clone()
                    } else {
                        format!("{}/{prefix}", source.name)
                    };
                    let next = Prefix {
                        copies: disk.level,
                        path: prefix.clone(),
                    };
                    if let Some(existing) = result.insert(slot.clone(), next.clone())
                        && existing != next
                    {
                        return Err("one target has conflicting source prefixes");
                    }
                } else if disk.level != 1 || disk.upstream_slot_id.is_some() {
                    return Err("route must start at a primary disk");
                }
                previous = Some(disk);
            }
        }
        Ok(result)
    }
}
