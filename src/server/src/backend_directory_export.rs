//! One captured same-namespace history export. Replay uses the exact captured
//! head and never resolves a pathname again.
use name_policy::{LookupPath, PortablePath};
use storage_contract::OperationId;

pub struct PrepareDirectoryFileExport {
    pub branch_id: String,
    pub source_path: LookupPath,
    pub destination: PortablePath,
}

#[derive(Clone, Debug)]
pub struct DirectoryFileExport {
    pub repository_id: String,
    pub branch_id: String,
    pub source_path: PortablePath,
    pub destination: PortablePath,
    pub expected_head: String,
}

pub struct ExportDirectoryFile {
    pub request: DirectoryFileExport,
    pub operation_id: OperationId,
}
