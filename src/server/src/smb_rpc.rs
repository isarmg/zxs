//! Protected local Samba bridge. Every operation is authorized by Backend;
//! mutable files remain unpublished until the normal core commit succeeds.
use crate::{
    ApiError, Backend, Content,
    auth::{AuthStore, Identity},
};
use json_codec::{Value, json};
use smb_protocol::protocol::{
    FrameStream, OpenResult, PROTOCOL, REQUIRED_CAPABILITIES, Request, Response,
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
use storage_contract::{Entry, Workspace, new_id, parse_u64_decimal};

const MAX_CONNECTIONS: usize = 64;
const MAX_AUTHENTICATED_SESSIONS: usize = 8;
const MAX_HANDLES: usize = 1024;

mod budget;
mod directory;
mod handles;
mod namespace;
mod publication;
mod session;
mod transport;
use publication::recovery_record;
use session::Session;
#[cfg(all(test, feature = "core"))]
use transport::run_connection;
pub use transport::serve;
#[cfg(all(test, feature = "core"))]
#[path = "../../../test/server/unit/smb_rpc/tests.rs"]
mod tests;

struct Shared {
    backend: Arc<dyn Backend>,
    username: String,
    auth: Arc<Mutex<AuthStore>>,
    spool: PathBuf,
    budget: Arc<budget::Budget>,
    max_file_bytes: u64,
    authenticated_sessions: Mutex<usize>,
    file_writers: Mutex<BTreeSet<(String, String)>>,
    namespace_operations: Mutex<BTreeSet<String>>,
}

enum Handle {
    Read(Content, String),
    Directory(DirectoryHandle),
    Write(Box<Writer>),
}

struct DirectoryHandle {
    file: File,
    entry_id: String,
    path: String,
    cursor: Option<DirectoryCursor>,
}

#[derive(Clone)]
struct DirectoryCursor {
    commit_id: String,
    after: Option<String>,
}

struct Writer {
    reservation: budget::Lease,
    file: File,
    directory: platform_io::fs_safety::PrivateDirectory,
    source: PathBuf,
    path: String,
    item_id: String,
    workspace: Workspace,
    dirty: bool,
    committed: bool,
    operation_id: String,
    last_head: String,
    write_through: bool,
}

fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str, ApiError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::invalid(format!("missing {key}")))
}
fn contract(error: storage_contract::Error) -> ApiError {
    ApiError::invalid_source(error)
}
