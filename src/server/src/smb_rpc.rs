//! Protected local Samba bridge. Every operation is authorized by Backend;
//! mutable files remain unpublished until the normal core commit succeeds.
use crate::{
    ApiError, Backend, Content, SmbRelativePath,
    auth::{AuthReader, AuthStore, Identity},
};
use data_serialization::Serialize;
use json_codec::{Value, json};
use smb_protocol::protocol::{
    AuthorizedMetadata, DirectoryPage, FrameStream, OpenResult, PROTOCOL, REQUIRED_CAPABILITIES,
    Request, Response, ResponsePayload, StatResult,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{self, Read, Write},
    os::{
        fd::{AsFd, OwnedFd},
        unix::fs::{FileExt, MetadataExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use storage_contract::{Entry, OperationId, Workspace, new_id};

const MAX_CONNECTIONS: usize = 64;
const MAX_AUTHENTICATED_SESSIONS: usize = 8;
const MAX_HANDLES: usize = 1024;

mod budget;
mod default_share;
mod directory;
mod handles;
mod metadata;
pub(crate) mod manual_recovery;
mod namespace;
mod object_move;
mod path_reservation;
mod publication;
mod recovery_record;
mod session;
mod transport;
mod workers;
use publication::recovery_record;
use session::Session;
#[cfg(all(test, feature = "core"))]
use transport::run_connection;
pub use transport::serve;
pub(crate) use transport::serve_owned;
pub(crate) use workers::{ConnectionRegistry, ConnectionShutdown};
#[cfg(all(test, feature = "core"))]
#[path = "../../../test/server/unit/smb_rpc/tests.rs"]
mod tests;

#[derive(Debug)]
enum Reply {
    Json(Value),
    Open {
        metadata: OpenResult,
        descriptor: OwnedFd,
    },
    Stat(StatResult),
    Metadata(AuthorizedMetadata),
    Directory(DirectoryPage),
    Read(bytes::Bytes),
}

impl Serialize for Reply {
    fn serialize<S: data_serialization::Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match self {
            Self::Json(value) => value.serialize(serializer),
            Self::Open { metadata, .. } => metadata.serialize(serializer),
            Self::Stat(value) => value.serialize(serializer),
            Self::Metadata(value) => value.serialize(serializer),
            Self::Directory(value) => value.serialize(serializer),
            Self::Read(bytes) => smb_protocol::protocol::ReadResult {
                bytes: bytes.as_ref(),
            }
            .serialize(serializer),
        }
    }
}

struct Shared {
    backend: Arc<dyn Backend>,
    account: Arc<crate::smb_account_binding::Authority>,
    auth: AuthReader,
    spool: PathBuf,
    budget: Arc<budget::Budget>,
    max_file_bytes: u64,
    authenticated_sessions: tokio::sync::Semaphore,
    file_writers: Mutex<BTreeSet<(String, String)>>,
    namespace_operations: Mutex<BTreeSet<OperationId>>,
}

impl Shared {
    fn workspace(&self, identity: &Identity, branch: &str) -> Result<Workspace, ApiError> {
        self.backend.create_workspace(
            identity,
            crate::CreateWorkspace {
                branch_id: branch.to_owned(),
                source_commit_id: None,
            },
        )
    }
}

enum ReadAuthority {
    Version(String),
    Draft(crate::ReadContainerEdit),
}

enum Handle {
    Read(Content, ReadAuthority),
    Directory(DirectoryHandle),
    // Every live writer has handed out a mutable FD. The peer can have
    // changed it before a Dirty request reached us, including after a
    // successful Flush. A disconnect therefore cannot prove clean bytes.
    // Retain both data and the recovery record without hashing in Drop or
    // publishing a new generation. Explicit successful Close still cleans up.
    // The handle owns its path reservation independently of the Session map.
    Write(Box<Writer>, path_reservation::PathReservation),
}

struct DirectoryHandle {
    file: File,
    entry_id: String,
    path: Option<name_policy::PortablePath>,
    cursor: Option<DirectoryCursor>,
}

struct DirectoryCursor {
    snapshot: crate::DirectoryReadSnapshot,
    after: Option<String>,
}

struct ContainerWriter {
    context: crate::ReadContainerEdit,
    base: storage_contract::container_edit::DraftBase,
    edit: storage_contract::container_edit::ContainerEdit,
    revision: Option<storage_contract::container_edit::DraftRevision>,
    persisted: bool,
}

struct Writer {
    recovery_record: recovery_record::ActiveRecord,
    reservation: budget::Lease,
    file: File,
    directory: platform_io::fs_safety::PrivateDirectory,
    source: PathBuf,
    path: name_policy::PortablePath,
    item_id: String,
    workspace: Workspace,
    dirty: bool,
    committed: bool,
    operation_id: OperationId,
    last_head: String,
    write_through: bool,
    container: Option<ContainerWriter>,
}

fn contract(error: storage_contract::Error) -> ApiError {
    ApiError::contract_input(error)
}
