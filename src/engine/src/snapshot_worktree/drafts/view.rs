use crate::engine::file_draft::SelectedWorkingSource;
use crate::snapshot_worktree::drafts::{DirectoryDraftError as Failure, Result};
use crate::working_tree::snapshot::{HeldSnapshotNode, SnapshotFile};
use crate::ReadFile;
use name_policy::PortablePath;
use std::io::Read;
use std::os::unix::fs::FileExt;
use storage_contract::FileId;

enum Body<'a> {
    Original { node: &'a HeldSnapshotNode, material: SnapshotFile },
    Selected(&'a SelectedWorkingSource),
}

/// A read-only occurrence, not a constructed FILE result. Unborn content has a
/// stable FileId but intentionally no historical node/VersionId at this phase.
pub(crate) struct DirectoryDraftFile<'a> {
    path: &'a PortablePath,
    body: Body<'a>,
}

impl<'a> DirectoryDraftFile<'a> {
    pub(super) fn original(
        path: &'a PortablePath,
        node: &'a HeldSnapshotNode,
        material: SnapshotFile,
    ) -> Self {
        Self { path, body: Body::Original { node, material } }
    }

    pub(super) fn selected(source: &'a SelectedWorkingSource) -> Self {
        Self { path: &source.target().path, body: Body::Selected(source) }
    }

    pub(crate) fn path(&self) -> &PortablePath {
        self.path
    }

    pub(crate) fn file(&self) -> FileId {
        match self.body {
            Body::Original { material, .. } => material.file_id,
            Body::Selected(source) => source.target().file,
        }
    }

    pub(crate) fn logical_size(&self) -> u64 {
        match self.body {
            Body::Original { material, .. } => material.size,
            Body::Selected(source) => source.logical_size(),
        }
    }

    pub(crate) fn open(&self) -> Result<DirectoryDraftReader<'a>> {
        let body = match self.body {
            Body::Original { node, material } => Reader::Original {
                file: node.open_payload(material.version_id).map_err(Failure::OriginalRead)?,
                size: material.size,
            },
            Body::Selected(source) => {
                source.require_source().map_err(Failure::Engine)?;
                Reader::Selected(source)
            }
        };
        Ok(DirectoryDraftReader { body, position: 0 })
    }
}

enum Reader<'a> {
    Original { file: ReadFile, size: u64 },
    Selected(&'a SelectedWorkingSource),
}

/// The selected merged reader stays intact; a sparse FD never escapes and is
/// never read as if its holes were the complete working file. Original reads
/// retain the immutable snapshot's gated complete BODY, not today's draft.
pub(crate) struct DirectoryDraftReader<'a> {
    body: Reader<'a>,
    position: u64,
}

/// The ordinary complete-copy primitive consumes Read, not a sparse raw FD.
/// Advance only by actual returned bytes; destination size/hash verification
/// remains mandatory even when every source is independently pinned.
impl Read for DirectoryDraftReader<'_> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        let count = self.read_at(output, self.position).map_err(std::io::Error::other)?;
        // read_at bounds count by the fixed logical length minus this position.
        self.position += count as u64;
        Ok(count)
    }
}

impl DirectoryDraftReader<'_> {
    pub(crate) fn read_at(&self, output: &mut [u8], offset: u64) -> Result<usize> {
        match &self.body {
            Reader::Selected(source) => source.read_at(output, offset).map_err(Failure::Engine),
            Reader::Original { file, size } => {
                file.check_running().map_err(Failure::Engine)?;
                if offset >= *size || output.is_empty() {
                    return Ok(0);
                }
                let count = (*size - offset).min(output.len() as u64) as usize;
                file.read_exact_at(&mut output[..count], offset).map_err(Failure::Read)?;
                file.check_running().map_err(Failure::Engine)?;
                Ok(count)
            }
        }
    }
}
