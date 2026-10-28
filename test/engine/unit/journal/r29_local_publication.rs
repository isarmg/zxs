// Frozen R29 (6da614ccb133b3f036f0be52d815d11519d06f76) owned schema.
// Independent of LocalPublication's Serialize delegation and envelope sizing.
use crate::{
    Result,
    disk::encode_metadata,
    journal::{Candidate, CommitProof, Publication, Receipt},
};

#[derive(data_serialization::Serialize)]
struct LocalPublication {
    candidate: Candidate,
    publication: Publication,
    commit_proof: CommitProof,
    publication_receipts: Vec<Receipt>,
}

pub(super) fn encode(local: &crate::journal::LocalPublication, maximum: usize) -> Result<Vec<u8>> {
    encode_metadata(
        &LocalPublication {
            candidate: local.candidate.clone(),
            publication: local.publication.clone(),
            commit_proof: local.commit_proof.clone(),
            publication_receipts: local.publication_receipts.clone(),
        },
        maximum,
    )
}
