//! One immutable same-payload installation baseline, never an upgrade license.
use super::payload;
use crate::{Result, private_io::{BundleDestination, ProgramDestination}};
use data_serialization::{Deserialize, Serialize};

pub(super) const NAME: &str = ".zxs-helper-program-baseline";
const MAX_BYTES: usize = 16 * 1024;

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct PayloadIdentity {
    version: String,
    commit: String,
    tree: String,
    target: String,
    sha256: String,
    bytes: u64,
}
impl PayloadIdentity {
    pub(super) fn from_payload(payload: payload::Payload) -> Self {
        Self {
            version: payload.version.into(), commit: payload.commit.into(),
            tree: payload.tree.into(), target: payload.target.into(),
            sha256: payload.sha256.into(), bytes: payload.bytes.len() as u64,
        }
    }
}

#[derive(Serialize)]
struct Baseline {
    format: &'static str,
    revision: u32,
    destination: &'static str,
    host_device: String,
    host_inode: String,
    parent_device: String,
    parent_inode: String,
    payload: PayloadIdentity,
    data_or_configuration_modified: bool,
    service_started: bool,
}

pub(super) fn expected(host: &BundleDestination, program: &ProgramDestination, payload: payload::Payload) -> Result<Vec<u8>> {
    let host_id = host.identity();
    let parent = program.parent_identity();
    Ok(json_codec::to_vec_pretty(&Baseline {
        format: "zxs-helper-installed-program-baseline", revision: 1,
        destination: super::DESTINATION,
        host_device: host_id.device.to_string(), host_inode: host_id.inode.to_string(),
        parent_device: parent.device.to_string(), parent_inode: parent.inode.to_string(),
        payload: PayloadIdentity::from_payload(payload),
        data_or_configuration_modified: false, service_started: false,
    })?)
}

pub(super) fn existing(host: &BundleDestination, expected: &[u8]) -> Result<bool> {
    match host.read_operation(NAME, "receipt.json", MAX_BYTES)? {
        Some(bytes) if bytes == expected => Ok(true),
        Some(_) => Err("installed program baseline does not match this exact helper payload and host; repair is not an upgrade or downgrade".into()),
        None if host.operation_started(NAME)? => Err("program baseline is incomplete or damaged; it was not replaced or inferred from runtime data".into()),
        None => Ok(false),
    }
}

pub(super) fn require(host: &BundleDestination, expected: &[u8]) -> Result<()> {
    if !existing(host, expected)? {
        return Err("same-payload repair requires the original verified program installation baseline; missing provenance is not permission to replace an unknown version".into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../test/helper/unit/installation/baseline_tests.rs"]
mod tests;
