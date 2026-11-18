//! The two SQLite tables contain no hash of their own future enclosing
//! Snapshot. A physical row source is not a published business receipt.
use super::*;
use crate::journal::Candidate;
use storage_contract::working_tree::MaterialCommitment;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(in crate::engine) enum Predecessor {
    Genesis { anchor_hash: MaterialCommitment },
    Snapshot { snapshot_hash: MaterialCommitment },
}

pub(super) fn commitment(value: &str) -> Result<MaterialCommitment> {
    let hash = value.parse::<storage_contract::ContentHash>()
        .map_err(|_| invalid("invalid canonical publication commitment in DISK history"))?;
    Ok(MaterialCommitment::from_digest(hash.digest()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(in crate::engine) enum Through {
    Genesis {},
    Publication {
        #[serialization(with = "storage_contract::u64_string")]
        sequence: u64,
        operation_id: OperationId,
        request_digest: MaterialCommitment,
        predecessor: Predecessor,
    },
}
impl Through {
    pub(in crate::engine) fn planned(
        config: &ValidatedConfig, previous: Option<&str>, sequence: u64,
        operation_id: OperationId, request_digest: &str,
    ) -> Result<Self> {
        if sequence == 0 {
            return Err(invalid("a real publication cannot claim genesis coverage"));
        }
        Ok(Self::Publication {
            sequence, operation_id, request_digest: commitment(request_digest)?,
            predecessor: match previous {
                Some(hash) => Predecessor::Snapshot { snapshot_hash: commitment(hash)? },
                None => Predecessor::Genesis { anchor_hash: config.fixed_genesis()?.hash },
            },
        })
    }
    pub(super) fn from_candidate(config: &ValidatedConfig, candidate: &Candidate) -> Result<Self> {
        Self::planned(config, candidate.previous_snapshot.as_deref(), candidate.sequence,
            candidate.operation_id, &candidate.request_digest)
    }
    pub(in crate::engine) fn sequence(self) -> u64 {
        match self { Self::Genesis {} => 0, Self::Publication { sequence, .. } => sequence }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(in crate::engine) enum RowSource {
    Prior { candidate_hash: MaterialCommitment, snapshot_hash: MaterialCommitment },
    ThisPublication {},
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::engine) struct StoredRow {
    #[serialization(with = "storage_contract::u64_string")]
    pub sequence: u64,
    pub operation_id: OperationId,
    pub source: RowSource,
    pub change: DiskHistoryChange,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::engine) struct StoredHeader {
    pub identity: DiskRegistrationIdentity,
    pub through: Through,
    pub publications: u32,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::engine) struct StoredHistory {
    pub header: StoredHeader,
    pub changes: Vec<StoredRow>,
}

impl StoredHistory {
    pub(in crate::engine) fn genesis(identity: DiskRegistrationIdentity) -> Self {
        Self { header: StoredHeader { identity, through: Through::Genesis {}, publications: 0 }, changes: vec![] }
    }

    pub(in crate::engine) fn logical_root(&self) -> Result<MaterialCommitment> {
        Ok(MaterialCommitment::from_digest(storage_contract::canonical_object_digest(
            "disk-local-inventory-changes", self,
        )?))
    }

    /// Import a completed native replay. Earlier source hashes come from that
    /// replay, never from the SQL being admitted; the last slot keeps no hash.
    pub(super) fn from_replayed(view: &DiskHistorySnapshot, through: Through) -> Result<Self> {
        let header = StoredHeader { identity: view.header.identity.clone(), publications: view.header.publications, through };
        if through.sequence() != view.header.through_sequence ||
            matches!(through, Through::Genesis {}) != view.header.through_snapshot.is_none() {
            return Err(invalid("DISK history coverage does not match its original request"));
        }
        header.require_shape()?;
        let mut changes = Vec::with_capacity(view.changes.len());
        for row in &view.changes {
            let source = if row.sequence == through.sequence() {
                if view.header.through_snapshot.as_ref() != Some(&row.snapshot_hash) {
                    return Err(invalid("DISK history current row names a different original snapshot"));
                }
                RowSource::ThisPublication {}
            } else {
                RowSource::Prior { candidate_hash: commitment(&row.candidate_hash)?,
                    snapshot_hash: commitment(&row.snapshot_hash)? }
            };
            let stored = StoredRow { sequence: row.sequence, operation_id: row.operation_id, source, change: row.change.clone() };
            header.require_source(&stored)?;
            changes.push(stored);
        }
        Ok(Self { header, changes })
    }

    pub(super) fn resolved(self, header: &DiskHistoryHeader, binding: &Binding) -> Result<DiskHistorySnapshot> {
        if self.header.through != binding.through || self.header.identity != header.identity
            || self.header.through.sequence() != header.through_sequence || self.header.publications != header.publications
        { return Err(invalid("stored disk scope differs from its independent native boundary")); }
        let mut changes = Vec::with_capacity(self.changes.len());
        for row in self.changes {
            let (candidate_hash, snapshot_hash) = match row.source {
                RowSource::Prior { candidate_hash, snapshot_hash } => (candidate_hash.hash().to_string(), snapshot_hash.hash().to_string()),
                RowSource::ThisPublication {} => (
                    binding.candidate.clone().ok_or_else(|| invalid("current change has no original decision"))?,
                    header.through_snapshot.clone().ok_or_else(|| invalid("current change has no original snapshot"))?,
                ),
            };
            changes.push(DiskHistoryRecord { sequence: row.sequence, operation_id: row.operation_id, candidate_hash, snapshot_hash, change: row.change });
        }
        Ok(DiskHistorySnapshot { header: header.clone(), changes })
    }

    /// Source-independent salvage can present an empty genesis. The legacy
    /// *logical export DTO*, unlike canonical SQLite, needs final hash strings
    /// it cannot recover from an isolated image. Preserve it, do not invent them.
    pub(super) fn genesis_view(&self) -> Result<DiskHistorySnapshot> {
        if self.header.through != (Through::Genesis {}) || !self.changes.is_empty() {
            return Err(Error::new(ErrorCode::RecoveryRequired,
                "this disk history requires its original publication for a resolved export; preserve the image"));
        }
        Ok(DiskHistorySnapshot {
            header: DiskHistoryHeader { identity: self.header.identity.clone(),
                through_sequence: 0, through_snapshot: None, publications: 0 }, changes: vec![],
        })
    }
}
impl StoredHeader {
    pub(super) fn require_shape(&self) -> Result<()> {
        let n = self.publications;
        if n as usize > MAX_PUBLICATIONS
            || match self.through {
                Through::Genesis {} => n != 0,
                Through::Publication { sequence, predecessor, .. } =>
                    n == 0 || u64::from(n) > sequence
                    || (n == 1) != matches!(predecessor, Predecessor::Genesis { .. }),
            }
        { return Err(invalid("DISK history coverage does not match its genesis/predecessor")); }
        Ok(())
    }
    pub(super) fn require_source(&self, row: &StoredRow) -> Result<()> {
        match (self.through, row.source) {
            (Through::Publication { sequence, operation_id, .. }, RowSource::ThisPublication {})
                if row.sequence == sequence && row.operation_id == operation_id => Ok(()),
            (Through::Publication { sequence, operation_id, .. }, RowSource::Prior { .. })
                if row.sequence > 0 && row.sequence < sequence && row.operation_id != operation_id => Ok(()),
            _ => Err(invalid("DISK history row has no exact prior/current publication source")),
        }
    }
}
