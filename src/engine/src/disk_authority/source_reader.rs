//! Descriptor-only metadata reader. There is deliberately no conversion into
//! IndependentDisk, writer, registration, content read, or membership authority.
use super::{
    IndependentDisk, Result, SourceCandidateBatch, SourceCandidateBatchError,
    SourceCandidateBatchStage, SourceNode, SourceTopology, candidate_set::MAX_CANDIDATES,
    observe_source_candidates, storage::Disk,
};
use platform_io::fs_safety::PrivateDirectory;

pub struct IndependentSourceReader {
    disk: IndependentDisk,
}
impl IndependentSourceReader {
    /// Admit the independent header and existing child handles without any
    /// creation, repair, cache, writer lock, or content reads. Header admission
    /// alone is not a verified graph; source_topology runs the real verifier.
    pub fn from_directory(root: PrivateDirectory) -> Result<Self> {
        Ok(Self {
            disk: IndependentDisk {
                disk: Disk::from_held_directory(root)?,
            },
        })
    }

    /// Verify all selected metadata and capture the real source graph.
    /// A successful historical capture does not prove current online presence.
    pub fn source_topology(&self) -> Result<SourceTopology> {
        self.disk.source_topology()
    }
}

pub(super) fn observe_held(
    node: &SourceNode,
    directories: Vec<PrivateDirectory>,
) -> std::result::Result<SourceCandidateBatch, SourceCandidateBatchError> {
    if directories.len() > MAX_CANDIDATES {
        return Err(SourceCandidateBatchError::CandidateBudget {
            actual: directories.len(),
            maximum: MAX_CANDIDATES,
        });
    }
    let readers = directories
        .into_iter()
        .enumerate()
        .map(|(index, root)| {
            IndependentSourceReader::from_directory(root).map_err(|source| {
                SourceCandidateBatchError::Candidate {
                    index,
                    stage: SourceCandidateBatchStage::OpenDirectory,
                    source,
                }
            })
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let disks: Vec<_> = readers.iter().map(|reader| &reader.disk).collect();
    observe_source_candidates(node, &disks)
}
