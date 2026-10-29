//! One prepared publication and its root, borrowed for workspace admission then replayed.
use super::*;

pub(super) struct PreparedCandidate {
    pub evidence: PreparedPublication,
    pub receipts: PublishedReceiptRoot,
}

pub(super) struct PreparedPublication {
    pub candidate: Candidate,
    pub publication: Publication,
    pub seal: PublicationSeal,
}

pub(super) struct PublishedReceiptRoot {
    snapshot: SchemaValidatedSnapshot,
    operation_id: OperationId,
    request_digest: String,
    sequence: u64,
    recovered_result: Option<OperationResult>,
}

impl PublishedReceiptRoot {
    pub(super) fn into_snapshot(self) -> SchemaValidatedSnapshot {
        self.snapshot
    }
}

impl PreparedRecovery {
    pub(in crate::engine) fn visit_workspace_plans(
        &self,
        engine: &Engine,
        mut visit: impl FnMut(Plan) -> Result<()>,
    ) -> Result<()> {
        let Some((_, prepared)) = &self.candidates else {
            return Ok(());
        };
        let root = &prepared.receipts;
        let plans = root
            .snapshot
            .snapshot()
            .maps
            .get("plans")
            .expect("invariant: admitted snapshot schema includes plans");
        crate::metadata_map::visit_objects(
            plans,
            |kind, hash| {
                crate::publication_evidence::verified_graph_object_at(
                    &engine.disk_resources,
                    &self.online,
                    kind,
                    hash,
                )
            },
            |object| {
                if object.kind() == "state-page" {
                    for value in object
                        .value()
                        .as_object()
                        .expect("invariant: visited plan leaf passed PageShape")
                        .values()
                    {
                        visit(json_codec::from_value(value.clone())?)?;
                    }
                }
                Ok(())
            },
        )
    }

    pub(in crate::engine) fn workspace_receipt(
        &self,
        engine: &Engine,
        id: OperationId,
    ) -> Result<Option<storage_contract::OperationRecordEntry>> {
        let Some((_, prepared)) = &self.candidates else {
            return Ok(None);
        };
        let root = &prepared.receipts;
        let Some(mut entry) = super::super::operations::snapshot_receipt(
            engine,
            &self.online,
            root.snapshot.snapshot(),
            id,
            id == root.operation_id && root.recovered_result.is_some(),
        )?
        else {
            return Ok(None);
        };
        if entry.result.sequence > root.sequence
            || (id == root.operation_id
                && (entry.result.sequence != root.sequence
                    || entry.result.request_digest != root.request_digest))
        {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "workspace receipt differs from the published prefix",
            ));
        }
        if id == root.operation_id
            && let Some(result) = &root.recovered_result
        {
            entry.result = result.clone();
        }
        Ok(Some(entry))
    }
}

impl Engine {
    pub(super) fn prepare_published_receipts(
        &self,
        online: &[DiskConfig],
        records: &super::super::recovery_checkpoint::Records,
    ) -> Result<(Option<(u64, PreparedCandidate)>, Option<(u64, Error)>)> {
        let mut prepared = None;
        let mut barrier_failure = None;
        let mut previous = None;
        for (sequence, candidates) in records {
            let mut active = candidates.iter().filter(|(_, receipts)| {
                receipts.iter().any(|receipt| {
                    matches!(
                        receipt.record.record_type,
                        RecordType::Accept | RecordType::Publication
                    )
                })
            });
            let Some((hash, receipts)) = active.next() else {
                continue;
            };
            if active.next().is_some() {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "different accepted candidates occupy the same slot",
                ));
            }
            let candidate: Candidate =
                verified_object_at(&self.disk_resources, online, "candidate", hash)?;
            self.verify_candidate_membership(&candidate)?;
            if candidate.sequence != *sequence
                || candidate.pool_id != self.config.pool_id
                || candidate.previous_snapshot != previous
            {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "candidate does not extend the verified publication prefix",
                ));
            }
            let publication = match receipts
                .iter()
                .find(|receipt| receipt.record.record_type == RecordType::Publication)
            {
                Some(receipt) => Some(verified_object_at::<Publication>(
                    &self.disk_resources,
                    online,
                    "publication",
                    &receipt.record.object_hash,
                )?),
                None => crate::degraded_recovery::discover_publication(self, &candidate, hash)?,
            };
            let Some(publication) = publication else {
                break;
            };
            let seal = match published_barrier(self, online, receipts, &candidate, &publication) {
                Ok(seal) => seal,
                Err(source) => {
                    // Apply decides whether this barrier can be repaired.
                    barrier_failure = Some((*sequence, source));
                    break;
                }
            };
            previous = Some(candidate.snapshot_hash.clone());
            prepared = Some((
                *sequence,
                PreparedPublication {
                    candidate,
                    publication,
                    seal,
                },
            ));
        }
        let prepared = prepared
            .map(|(sequence, source)| {
                let snapshot: Snapshot = verified_object_at(
                    &self.disk_resources,
                    online,
                    "snapshot",
                    &source.candidate.snapshot_hash,
                )?;
                let snapshot = SchemaValidatedSnapshot::new(snapshot)?;
                if snapshot.snapshot().sequence != source.candidate.sequence {
                    return Err(Error::new(
                        ErrorCode::ObjectCorrupt,
                        "snapshot sequence differs",
                    ));
                }
                let publication = &source.publication;
                let receipts = PublishedReceiptRoot {
                    snapshot,
                    operation_id: source.candidate.operation_id,
                    request_digest: source.candidate.request_digest.clone(),
                    sequence: source.candidate.sequence,
                    recovered_result: publication
                        .recovery
                        .as_ref()
                        .map(|_| publication.result.clone()),
                };
                Ok((
                    sequence,
                    PreparedCandidate {
                        evidence: source,
                        receipts,
                    },
                ))
            })
            .transpose()?;
        Ok((prepared, barrier_failure))
    }
}

fn published_barrier(
    engine: &Engine,
    online: &[DiskConfig],
    receipts: &[Receipt],
    candidate: &Candidate,
    publication: &Publication,
) -> Result<PublicationSeal> {
    let publication_hash = object_hash("publication", publication)?;
    let complete = candidate
        .metadata_disk_ids
        .iter()
        .map(|id| {
            receipts
                .iter()
                .find(|receipt| {
                    receipt.disk_id == *id
                        && receipt.record.record_type == RecordType::Publication
                        && receipt.record.object_hash == publication_hash
                })
                .cloned()
        })
        .collect::<Option<Vec<_>>>();
    if let Some(receipts) = complete {
        let seal = PublicationSeal {
            candidate_hash: publication.candidate_hash.clone(),
            publication_hash,
            receipts,
        };
        verify_publication_seal(candidate, publication, &seal)?;
        Ok(seal)
    } else {
        verified_publication_seal_at(&engine.disk_resources, online, candidate, publication)
    }
}
