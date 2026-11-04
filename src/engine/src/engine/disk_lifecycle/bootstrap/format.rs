//! Bounded canonical persistence records, not published pool objects.
use super::*;
use data_serialization::{Deserialize, Serialize};
use storage_contract::{canonical_json, decode_strict, encode_hashed_object};

const RECORD_DOMAIN: &str = "bootstrap-locator";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum RecordFormat {
    #[serialization(rename = "restrictive-bootstrap-locator")]
    RestrictiveLocator,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum HeadFormat {
    #[serialization(rename = "restrictive-bootstrap-head")]
    RestrictiveHead,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Head {
    format: HeadFormat,
    pub(super) pool_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub(super) generation: u64,
    // ContentHash supplies strict fixed-width hex syntax only; this digest is
    // computed with RECORD_DOMAIN, never as a file-body hash or authority proof.
    pub(super) record_hash: storage_contract::ContentHash,
}
impl Head {
    pub(super) fn decode(bytes: &[u8]) -> Result<Self> {
        let head: Self = decode_canonical(bytes, 2048)?;
        storage_contract::validate_id(&head.pool_id, "pool")
            .map_err(|e| BootstrapError::encoding("head pool", e))?;
        if head.generation == 0 {
            return Err(BootstrapError::ManagementOnly("zero head generation"));
        }
        Ok(head)
    }
    pub(super) fn bytes(&self) -> Result<Vec<u8>> {
        canonical_json(self).map_err(|e| BootstrapError::encoding("head", e))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct RootRecord {
    pub(super) disk_id: String,
    pub(super) slot_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub(super) binding_generation: u64,
    pub(super) path: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub(super) device: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub(super) inode: u64,
    pub(super) metadata_member: bool,
}
impl RootRecord {
    pub(super) fn validate(&self) -> Result<()> {
        storage_contract::validate_id(&self.disk_id, "disk")
            .map_err(|e| BootstrapError::encoding("root disk identity", e))?;
        name_policy::validate_slot(&self.slot_id)
            .map_err(|source| BootstrapError::Slot { source })?;
        if self.binding_generation == 0 {
            return Err(BootstrapError::ManagementOnly(
                "zero root binding generation",
            ));
        }
        canonical_root(Path::new(&self.path))?;
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct StopRecord {
    pub(super) disk_id: String,
    pub(super) operation_id: OperationId,
    pub(super) intent_digest: IntentDigest,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Record {
    format: RecordFormat,
    pub(super) pool_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub(super) generation: u64,
    pub(super) previous: Option<Head>,
    pub(super) roots: Vec<RootRecord>,
    pub(super) retained: Vec<String>,
    pub(super) stops: Vec<StopRecord>,
}
impl Record {
    pub(super) fn initial(pool_id: &str, observations: Vec<RootObservation>) -> Result<Self> {
        if observations.is_empty() || observations.len() > MAX_ROOTS {
            return Err(BootstrapError::ManagementOnly(
                "root count exceeds locator bound",
            ));
        }
        let mut roots = Vec::with_capacity(observations.len());
        for observation in observations {
            if observation.pool_id != pool_id {
                return Err(BootstrapError::ManagementOnly(
                    "observed root belongs to another pool",
                ));
            }
            roots.push(observation.record);
        }
        roots.sort_by(|a, b| a.disk_id.cmp(&b.disk_id));
        let retained = roots
            .iter()
            .filter(|root| root.metadata_member)
            .map(|root| root.disk_id.clone())
            .collect();
        let record = Self {
            format: RecordFormat::RestrictiveLocator,
            pool_id: pool_id.into(),
            generation: 1,
            previous: None,
            roots,
            retained,
            stops: Vec::new(),
        };
        record.validate()?;
        Ok(record)
    }
    pub(super) fn stop(
        previous: &LoadedLocator,
        target: &DiskRegistrationIdentity,
        operation_id: OperationId,
        intent_digest: IntentDigest,
    ) -> Result<Self> {
        let source = &previous.record;
        let root = source
            .roots
            .iter()
            .find(|root| root.disk_id == target.disk_id)
            .ok_or(BootstrapError::ManagementOnly(
                "stop target is not a recorded binding",
            ))?;
        if source.pool_id != target.pool_id
            || root.slot_id != target.slot_id
            || root.binding_generation != target.binding_generation
        {
            return Err(BootstrapError::ManagementOnly(
                "stop target binding differs",
            ));
        }
        if source
            .stops
            .iter()
            .any(|stop| stop.disk_id == target.disk_id || stop.operation_id == operation_id)
        {
            return Err(BootstrapError::ManagementOnly(
                "stop target or operation already recorded",
            ));
        }
        let mut record = source.clone();
        record.generation =
            source
                .generation
                .checked_add(1)
                .ok_or(BootstrapError::ManagementOnly(
                    "locator generation overflow",
                ))?;
        record.previous = Some(previous.head.clone());
        record.retained.retain(|disk_id| disk_id != &target.disk_id);
        record.stops.push(StopRecord {
            disk_id: target.disk_id.clone(),
            operation_id,
            intent_digest,
        });
        record.stops.sort_by(|a, b| a.disk_id.cmp(&b.disk_id));
        record.validate()?;
        Ok(record)
    }
    pub(super) fn decode(bytes: &[u8]) -> Result<Self> {
        let record: Self = decode_canonical(bytes, MAX_RECORD_BYTES)?;
        record.validate()?;
        Ok(record)
    }
    pub(super) fn encode(&self) -> Result<(Vec<u8>, Head)> {
        let (bytes, hash) = encode_hashed_object(RECORD_DOMAIN, self)
            .map_err(|e| BootstrapError::encoding("record", e))?;
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(BootstrapError::ManagementOnly(
                "encoded locator exceeds byte bound",
            ));
        }
        let record_hash = hash
            .parse()
            .expect("invariant: canonical object hash is lowercase BLAKE3 hex");
        Ok((
            bytes,
            Head {
                format: HeadFormat::RestrictiveHead,
                pool_id: self.pool_id.clone(),
                generation: self.generation,
                record_hash,
            },
        ))
    }
    pub(super) fn root(&self, disk_id: &str) -> &RootRecord {
        self.roots
            .iter()
            .find(|root| root.disk_id == disk_id)
            .expect("invariant: retained roots are validated against recorded roots")
    }
    fn validate(&self) -> Result<()> {
        storage_contract::validate_id(&self.pool_id, "pool")
            .map_err(|e| BootstrapError::encoding("record pool", e))?;
        if self.generation == 0
            || self.roots.is_empty()
            || self.roots.len() > MAX_ROOTS
            || self.stops.len() >= self.roots.len()
        {
            return Err(BootstrapError::ManagementOnly(
                "locator counts or generation exceed bounds",
            ));
        }
        match &self.previous {
            None if self.generation == 1 && self.stops.is_empty() => (),
            Some(head)
                if head.pool_id == self.pool_id
                    && head.generation.checked_add(1) == Some(self.generation)
                    && self.stops.len() as u64 + 1 == self.generation =>
            {
                ()
            }
            _ => {
                return Err(BootstrapError::ManagementOnly(
                    "locator predecessor or stop sequence differs",
                ));
            }
        }
        self.validate_roots()?;
        let mut previous = None;
        let mut operations = std::collections::BTreeSet::new();
        for stop in &self.stops {
            if previous.is_some_and(|id: &str| id >= stop.disk_id.as_str())
                || !operations.insert(stop.operation_id)
                || !self.roots.iter().any(|root| root.disk_id == stop.disk_id)
            {
                return Err(BootstrapError::ManagementOnly(
                    "stop bindings are duplicated, unsorted or unknown",
                ));
            }
            previous = Some(stop.disk_id.as_str());
        }
        let retained: Vec<_> = self
            .roots
            .iter()
            .filter(|root| {
                root.metadata_member && !self.stops.iter().any(|stop| stop.disk_id == root.disk_id)
            })
            .map(|root| root.disk_id.clone())
            .collect();
        if retained.is_empty() || retained != self.retained {
            return Err(BootstrapError::ManagementOnly(
                "retained roots differ from monotonic stop set",
            ));
        }
        Ok(())
    }
    fn validate_roots(&self) -> Result<()> {
        for (index, root) in self.roots.iter().enumerate() {
            root.validate()?;
            if index > 0 && self.roots[index - 1].disk_id >= root.disk_id {
                return Err(BootstrapError::ManagementOnly(
                    "root identities are duplicated or unsorted",
                ));
            }
            for old in &self.roots[..index] {
                if old.slot_id.eq_ignore_ascii_case(&root.slot_id)
                    || overlaps(&old.path, &root.path)
                    || (old.device, old.inode) == (root.device, root.inode)
                {
                    return Err(BootstrapError::ManagementOnly(
                        "root paths, bindings or observed identities alias",
                    ));
                }
            }
        }
        Ok(())
    }
    pub(super) fn follows(&self, older: &LoadedLocator) -> Result<()> {
        if self.previous.as_ref() != Some(&older.head)
            || self.roots != older.record.roots
            || self.stops.len() != older.record.stops.len() + 1
            || !older
                .record
                .stops
                .iter()
                .all(|stop| self.stops.contains(stop))
        {
            return Err(BootstrapError::ManagementOnly(
                "ledger changes bindings, removes a stop or breaks its hash chain",
            ));
        }
        Ok(())
    }
}
fn decode_canonical<T: data_serialization::de::DeserializeOwned + Serialize>(
    bytes: &[u8],
    maximum: usize,
) -> Result<T> {
    if bytes.len() > maximum {
        return Err(BootstrapError::ManagementOnly(
            "locator input exceeds byte bound",
        ));
    }
    let value = decode_strict(bytes).map_err(|e| BootstrapError::encoding("decode", e))?;
    if canonical_json(&value)
        .map_err(|e| BootstrapError::encoding("canonical representation", e))?
        != bytes
    {
        return Err(BootstrapError::ManagementOnly(
            "locator bytes are not canonical",
        ));
    }
    Ok(value)
}
