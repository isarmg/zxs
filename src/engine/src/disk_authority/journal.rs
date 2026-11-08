//! Narrow offline consumer of the per-disk protocol. No Engine State, pool
//! membership, namespace publication, or online admission is inferred here.
use super::{model::Object, *};
use data_serialization::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};
use storage_contract::OperationId;

/// An explicit new-format selection. Its logical identity is never inferred
/// from the host directory or from a current-format managed-directory seal.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DiskSelection {
    pub path: PathBuf,
    pub identity: Identity,
    pub known_checkpoint: Option<Digest>,
}

pub struct IndependentDisk {
    pub(super) disk: Disk,
}

/// A complete local file write. Ordinary writes always provide their body.
/// A missing body is representable only when preserving a verified source.
pub struct FileWrite<'a> {
    pub operation: OperationId,
    pub expected_base: Option<Digest>,
    pub history: &'a Token,
    pub file: &'a Token,
    pub bytes: &'a [u8],
}

/// This value is a review, not publication or user/session authorization.
/// Only execute/resume can create a verified publication receipt.
pub struct ReviewedPublication {
    request: PublicationRequest,
}
impl ReviewedPublication {
    pub fn request_digest(&self) -> Digest {
        self.request.request_digest()
    }
    pub fn operation_id(&self) -> &Token {
        &self.request.intent().operation_id
    }
    pub fn base(&self) -> Option<Digest> {
        self.request.intent().base
    }
}

#[derive(Debug, Serialize)]
pub struct DiskReport {
    pub identity: Identity,
    pub checkpoint: Option<Digest>,
    pub sequence: u64,
    pub copies: Vec<CopyRow>,
}

#[derive(Debug, Serialize)]
#[serialization(tag = "stage", rename_all = "snake_case")]
pub enum OperationStatus {
    Absent,
    Incomplete {
        operation_id: Token,
        local_target_matches: Option<bool>,
    },
    Intent {
        request_digest: Digest,
        local_target_matches: bool,
        base_matches: bool,
    },
    Prepared {
        request_digest: Digest,
        prepared_digest: Digest,
        local_target_matches: bool,
        base_matches: bool,
        missing_required_bodies: Vec<Digest>,
    },
    Published {
        receipt: DiskPublicationReceipt,
    },
}

impl IndependentDisk {
    /// Explicit new-format initialization. Existing unrelated contents are
    /// never adopted, reset, upgraded, or interpreted as a usable disk.
    pub fn initialize(path: &Path, identity: Identity) -> Result<Self> {
        Ok(Self {
            disk: Disk::initialize(path, identity)?,
        })
    }
    pub fn open(selection: &DiskSelection) -> Result<Self> {
        let disk =
            Disk::open(&selection.path, &selection.identity)?.pinned(selection.known_checkpoint)?;
        Ok(Self { disk })
    }
    pub fn identity(&self) -> &Identity {
        self.disk.identity()
    }
    pub fn report(&self) -> Result<DiskReport> {
        let verified = self.disk.verify(None)?;
        Ok(DiskReport {
            identity: verified.identity.clone(),
            checkpoint: verified.checkpoint,
            sequence: verified.sequence,
            copies: verified.rows,
        })
    }
    /// Capture verified metadata once for bounded reads of retained versions.
    /// Stop all writers first. This interval observation grants no authority
    /// and proves neither global freshness nor the state of any other disk.
    pub fn content_snapshot(&self) -> Result<ContentSnapshot<'_>> {
        self.disk.content_snapshot()
    }
    /// Capture the selected publication's source graph after complete metadata
    /// verification. Bodies are not read and no content availability is implied.
    /// This historical observation neither registers a disk nor grants access.
    pub fn source_topology(&self) -> Result<SourceTopology> {
        self.disk.source_topology()
    }
    /// Inspect this disk as one candidate for the exact captured source node.
    /// Metadata errors fail the observation. A pin match proves neither unique
    /// physical presence, body availability, membership, nor permission to act.
    pub fn observe_source_node(&self, node: &SourceNode) -> Result<SourceCandidateObservation> {
        self.disk.observe_source_node(node)
    }
    /// Read the full body only after its published metadata and bytes verify.
    /// The cache is not consulted; absent content remains temporarily unavailable.
    pub fn read_content(&self, commit: Digest) -> Result<Vec<u8>> {
        self.disk.read_content(commit)
    }
    pub fn review_file(&self, write: FileWrite<'_>) -> Result<ReviewedPublication> {
        let before = self.disk.verify(None)?;
        if before.identity.role != Role::Primary {
            return Err(Error::Invalid(
                "only primary disks accept local file writes",
            ));
        }
        if before.checkpoint != write.expected_base {
            return Err(Error::StaleBase);
        }
        let mut heads = primary_heads(&before)?;
        let mut parents = Vec::new();
        for head in &heads {
            let raw = before
                .bundle
                .objects
                .get(head)
                .ok_or(Error::Missing(*head))?;
            let Object::Commit(commit) = codec::decode(raw)? else {
                return Err(Error::Invalid("primary head is not a commit"));
            };
            if &commit.history_id == write.history && &commit.file_id == write.file {
                parents.push(*head);
            }
        }
        let replaced: BTreeSet<_> = parents.iter().copied().collect();
        heads.retain(|head| !replaced.contains(head));
        let mut material = Bundle::default();
        let head = material.add_commit(
            Commit {
                owner: before.identity.clone(),
                history_id: write.history.clone(),
                file_id: write.file.clone(),
                parents,
                body: Body {
                    digest: Digest::body(write.bytes),
                    bytes: write.bytes.len() as u64,
                },
            },
            Some(write.bytes),
        )?;
        heads.push(head);
        heads.sort();
        Ok(ReviewedPublication {
            request: PublicationRequest::new(
                &before,
                operation_token(write.operation)?,
                Authority::Primary { heads },
                material,
            )?,
        })
    }
    /// Preserve all original source identities. A secondary must select primary
    /// checkpoints; a tertiary must select secondary checkpoints. No role change.
    pub fn review_backup(
        &self,
        operation: OperationId,
        expected_base: Option<Digest>,
        selections: &[DiskSelection],
    ) -> Result<ReviewedPublication> {
        let before = self.disk.verify(None)?;
        if before.checkpoint != expected_base {
            return Err(Error::StaleBase);
        }
        if selections.is_empty() || selections.len() > MAX_PUBLICATIONS {
            return Err(Error::Invalid(
                "backup requires bounded explicitly selected sources",
            ));
        }
        let mut material = Bundle::default();
        let mut sources = Vec::with_capacity(selections.len());
        let mut local_sources = Vec::with_capacity(selections.len());
        for selection in selections {
            let source = Disk::open(&selection.path, &selection.identity)?;
            if self.disk.path().starts_with(source.path())
                || source.path().starts_with(self.disk.path())
                || self.disk.location()?.overlaps(&source.location()?)?
            {
                return Err(Error::Invalid(
                    "backup target overlaps a selected source disk",
                ));
            }
            let verified = source.verify(selection.known_checkpoint)?;
            local_sources.push(source.selected_source(&verified)?);
            material.extend(&verified.portable_bundle())?;
            sources.push(verified.source_checkpoint()?);
        }
        sources.sort_by_key(|source| source.identity.origin.clone());
        let authority = match before.identity.role {
            Role::Primary => return Err(Error::Invalid("a primary cannot become a backup")),
            Role::Secondary => Authority::Secondary { sources },
            Role::Tertiary => Authority::Tertiary { sources },
        };
        Ok(ReviewedPublication {
            request: PublicationRequest::selected(
                &before,
                operation_token(operation)?,
                authority,
                material,
                local_sources,
            )?,
        })
    }
    /// Trusted offline caller boundary. This does not authorize online access.
    /// Current physical identity and publication base are checked under the lock.
    pub fn execute(&self, review: &ReviewedPublication) -> Result<DiskPublicationReceipt> {
        let writer = self.disk.writer()?;
        let prepared = writer.prepare(&review.request)?;
        writer.publish(&prepared)
    }
    pub fn resume(
        &self,
        operation: OperationId,
        expected_request: Digest,
    ) -> Result<DiskPublicationReceipt> {
        let operation = operation_token(operation)?;
        let writer = self.disk.writer()?;
        match self.disk.operation_evidence(&operation)? {
            Some(OperationEvidence::Published(receipt))
                if receipt.request_digest() == expected_request => {}
            Some(OperationEvidence::Prepared { intent, .. })
                if intent.request_digest == expected_request => {}
            Some(OperationEvidence::Published(_)) | Some(OperationEvidence::Prepared { .. }) => {
                return Err(Error::OperationConflict);
            }
            _ => {
                return Err(Error::Invalid(
                    "operation is not prepared; exact reviewed source material is still required",
                ));
            }
        }
        writer.publish(&writer.resume_prepared(&operation)?)
    }
    pub fn operation_status(&self, operation: OperationId) -> Result<OperationStatus> {
        Ok(
            match self.disk.operation_evidence(&operation_token(operation)?)? {
                None => OperationStatus::Absent,
                Some(OperationEvidence::Incomplete {
                    operation_id,
                    local_target_matches,
                }) => OperationStatus::Incomplete {
                    operation_id,
                    local_target_matches,
                },
                Some(OperationEvidence::Intent {
                    intent,
                    local_target_matches,
                    base_matches,
                }) => OperationStatus::Intent {
                    request_digest: intent.request_digest,
                    local_target_matches,
                    base_matches,
                },
                Some(OperationEvidence::Prepared {
                    intent,
                    prepared,
                    local_target_matches,
                    base_matches,
                    missing_required_bodies,
                    ..
                }) => OperationStatus::Prepared {
                    request_digest: intent.request_digest,
                    prepared_digest: Object::Prepared(prepared).digest()?,
                    local_target_matches,
                    base_matches,
                    missing_required_bodies,
                },
                Some(OperationEvidence::Published(receipt)) => {
                    OperationStatus::Published { receipt }
                }
            },
        )
    }
}

/// Inspect only these explicitly selected candidates. This is not a discovery
/// enumerator: zero or one match remains incomplete, never unique or absent.
/// Any failed capture, changed alias or unproved physical relation rejects the
/// whole batch. No partial successful batch or storage capability is returned.
pub fn observe_source_candidates(
    node: &SourceNode,
    candidates: &[&IndependentDisk],
) -> std::result::Result<SourceCandidateBatch, SourceCandidateBatchError> {
    let mut batch = candidate_set::CandidateBatchBuilder::new(node, candidates.len())?;
    for (index, candidate) in candidates.iter().enumerate() {
        let capture = candidate
            .disk
            .capture_source_candidate(node)
            .map_err(|source| SourceCandidateBatchError::Candidate {
                index,
                stage: SourceCandidateBatchStage::Capture,
                source,
            })?;
        batch.push(index, capture)?;
    }
    for (index, candidate) in candidates.iter().enumerate() {
        candidate
            .disk
            .location()
            .map_err(|source| SourceCandidateBatchError::Candidate {
                index,
                stage: SourceCandidateBatchStage::RecheckDirectory,
                source,
            })?;
    }
    Ok(batch.finish())
}

fn primary_heads(before: &VerifiedDisk) -> Result<Vec<Digest>> {
    let Some(checkpoint) = before.checkpoint else {
        return Ok(vec![]);
    };
    let publication = &before.publications[&checkpoint];
    let bytes = before
        .bundle
        .objects
        .get(&publication.manifest)
        .ok_or(Error::Missing(publication.manifest))?;
    let Object::Manifest(Manifest {
        authority: Authority::Primary { heads },
        ..
    }) = codec::decode(bytes)?
    else {
        return Err(Error::Invalid("primary checkpoint has no primary manifest"));
    };
    Ok(heads)
}
fn operation_token(operation: OperationId) -> Result<Token> {
    Token::new(operation.to_string())
}
