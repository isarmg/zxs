//! One typed live snapshot owns disk records and their current lookup index.
use super::wire;
use std::collections::HashMap;

pub(super) struct Snapshot {
    sample: wire::Sample,
    disk_index: HashMap<String, usize>,
}
impl PartialEq for Snapshot {
    fn eq(&self, other: &Self) -> bool {
        // The private index is determined entirely by the ordered sample.
        self.sample == other.sample
    }
}
impl Snapshot {
    pub fn sample(&self) -> &wire::Sample {
        &self.sample
    }

    pub fn disk(&self, id: &str) -> Option<&wire::Disk> {
        self.disk_index
            .get(id)
            .map(|index| &self.sample.disks[*index])
    }
}

pub(super) fn decode(value: json_codec::Value) -> Result<Snapshot, crate::api::Error> {
    project(value).map_err(|source| {
        let mut error = crate::api::error(
            storage_contract::ErrorCode::InvalidResponse,
            web_interface::i18n::t(
                "系统观测响应格式无效。",
                "Invalid system observation response.",
            ),
        );
        error.source = Some(std::sync::Arc::new(
            crate::api::ErrorSource::MonitorSnapshot(source),
        ));
        error
    })
}

fn project(value: json_codec::Value) -> Result<Snapshot, json_codec::Error> {
    let sample: wire::Sample = json_codec::from_value(value)?;
    let mut disk_index = HashMap::with_capacity(sample.disks.len());
    for (index, disk) in sample.disks.iter().enumerate() {
        if disk.id.is_empty() || disk_index.insert(disk.id.clone(), index).is_some() {
            return Err(data_serialization::de::Error::custom(
                "duplicate or empty disk id",
            ));
        }
    }
    Ok(Snapshot { sample, disk_index })
}
