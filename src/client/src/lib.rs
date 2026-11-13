//! Shared native HTTP/HTTPS client. Redirects are disabled.
use data_serialization::{Deserialize, Serialize};
use json_codec::{Value, json};
use std::time::Duration;

#[cfg(target_os = "linux")]
mod container_edit;
mod download_target;
mod response;
mod transfer;
use response::decode;
pub use transfer::PreparedUpload;

#[derive(Debug, error_definition::Error)]
pub enum Error {
    #[error("invalid server address: {0}")]
    Address(String),
    #[error("invalid server address: {0}")]
    Url(#[from] url_encoding::address::ParseError),
    #[error("invalid API request: {0}")]
    Contract(#[from] storage_contract::Error),
    #[error("invalid JSON: {0}")]
    Json(#[from] json_codec::Error),
    #[error("request task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("invalid HTTP method: {0}")]
    Method(#[from] http::method::InvalidMethod),
    #[error("{code}: {message}{}", ApiRequestContext(request_id.as_deref()))]
    Api {
        status: u16,
        code: storage_contract::ErrorCode,
        message: String,
        request_id: Option<String>,
        retryable: bool,
        #[source]
        source: Option<storage_contract::client_protocol::ResponseDecodeError>,
    },
    #[error("transport failed: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("local file operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid response: {0}")]
    Response(String),
    #[error("invalid download ETag header: {0}")]
    DownloadEtagEncoding(#[source] http::header::ToStrError),
    #[error("invalid download content hash: {0}")]
    DownloadContentHash(#[source] storage_contract::ParseContentHashError),
    #[error("CHECKSUM_MISMATCH: download did not match the immutable version hash")]
    ChecksumMismatch,
}
// Presentation borrows structured context; callers never need to parse it back
// out of the server's message, and adapting an error does not format a new String.
struct ApiRequestContext<'a>(Option<&'a str>);
impl std::fmt::Display for ApiRequestContext<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(request_id) => write!(formatter, " (request {request_id})"),
            None => Ok(()),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Session {
    pub token: String,
    pub csrf_token: String,
    pub principal: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub expires_at: u64,
}
impl Session {
    fn decode(value: Value) -> Result<Self, Error> {
        let session: Self = json_codec::from_value(value)?;
        if session.token.is_empty()
            || session.csrf_token.is_empty()
            || session.principal.is_empty()
            || session.expires_at == 0
        {
            return Err(Error::Response("incomplete authentication response".into()));
        }
        Ok(session)
    }
}
impl std::fmt::Debug for Session {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Session")
            .field("principal", &self.principal)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

/// A backup update whose policy generation, scope identity, and operation remain explicit.
/// `expected_generation` is the read response's own `generation`, not `effective_generation`.
#[derive(Clone, Copy, Debug)]
pub struct BackupConfigurationRequest<'a> {
    pub repository_id: &'a str,
    pub path: &'a str,
    pub target_copies: u32,
    pub backup_prefixes: &'a std::collections::BTreeMap<String, String>,
    pub expected_generation: u64,
    pub expected_scope_id: &'a str,
    pub operation_id: &'a str,
}

#[derive(Clone)]
pub struct Client {
    client: reqwest::Client,
    base: url_encoding::address::Url,
    token: Option<String>,
}
impl Client {
    pub fn connect(address: &str) -> Result<Self, Error> {
        let base = url_encoding::address::Url::parse(address)?;
        if !matches!(base.scheme(), "https" | "http")
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || base.path() != "/"
        {
            return Err(Error::Address("use an HTTP or HTTPS origin; paths, credentials, query strings and fragments are prohibited".into()));
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(3600))
            .build()?;
        Ok(Self {
            client,
            base,
            token: None,
        })
    }
    pub fn set_token(&mut self, token: Option<String>) {
        self.token = token;
    }
    pub fn origin(&self) -> &str {
        self.base.as_str()
    }
    fn url(&self, path: &str) -> Result<url_encoding::address::Url, Error> {
        storage_contract::validate_api_path(path)?;
        let url = self.base.join(path)?;
        if url.origin() != self.base.origin() || !url.path().starts_with("/api/") {
            return Err(Error::Address("API route changed origin".into()));
        }
        Ok(url)
    }
    fn builder(
        &self,
        method: reqwest::Method,
        path: &str,
    ) -> Result<reqwest::RequestBuilder, Error> {
        let mut builder = self.client.request(method, self.url(path)?);
        if let Some(token) = &self.token {
            builder = builder.bearer_auth(token);
        }
        Ok(builder)
    }
    pub async fn login(&mut self, username: &str, password: &str) -> Result<Session, Error> {
        let value = self
            .request(
                "POST",
                "/api/auth/login",
                Some(&json!({"username":username,"password":password})),
            )
            .await?;
        let session = Session::decode(value)?;
        self.token = Some(session.token.clone());
        Ok(session)
    }
    pub async fn logout(&mut self) -> Result<(), Error> {
        let value = self
            .request("POST", "/api/auth/logout", Some(&json!({})))
            .await?;
        if !storage_contract::client_protocol::logout_confirmed(&value) {
            return Err(Error::Response(
                "logout was not confirmed; retain the session for a revocation retry".into(),
            ));
        }
        self.token = None;
        Ok(())
    }
    /// Freeze the workspace, bind the public envelope digest, and commit exactly that manifest.
    pub async fn commit_workspace(
        &self,
        workspace: &str,
        operation_id: &str,
        message: &str,
        expected_head: Option<&str>,
    ) -> Result<Value, Error> {
        let body = self
            .prepare_commit(workspace, operation_id, message, expected_head)
            .await?;
        self.commit_prepared(&body).await
    }
    pub async fn prepare_commit(
        &self,
        workspace: &str,
        operation_id: &str,
        message: &str,
        expected_head: Option<&str>,
    ) -> Result<Value, Error> {
        let body = self
            .request(
                "POST",
                &format!("/api/workspaces/{}/seal", segment(workspace)),
                Some(&json!({})),
            )
            .await?;
        storage_contract::client_protocol::prepare_commit(
            body,
            workspace,
            operation_id,
            message,
            expected_head,
        )
        .map_err(Error::from)
    }

    pub async fn commit_prepared(&self, body: &Value) -> Result<Value, Error> {
        let workspace = body
            .get("workspace_id")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Response("prepared request has no workspace identity".into()))?;
        self.request(
            "POST",
            &format!("/api/workspaces/{}/commit", segment(workspace)),
            Some(body),
        )
        .await
    }
    /// Inspect frozen directory compositions without changing the shared default.
    /// `None` selects all scopes; `Some("")` selects the root scope only.
    pub async fn directory_branches(
        &self,
        repository: &str,
        path: Option<&str>,
    ) -> Result<Value, Error> {
        let mut route = format!(
            "/api/repositories/{}/directory-branches",
            segment(repository)
        );
        if let Some(path) = path {
            route.push_str("?path=");
            route.push_str(&segment(path));
        }
        self.request("GET", &route, None).await
    }

    /// Submit explicit source/target branch mappings as one idempotent operation.
    /// Preserve the operation ID and every frozen source/target identity for a retry.
    pub async fn directory_namespace_batch(
        &self,
        slot: &str,
        request: &storage_contract::DirectoryNamespaceBatch,
        operation_id: &str,
    ) -> Result<Value, Error> {
        let mut body = json_codec::to_value(request)?;
        body["operation_id"] = operation_id.into();
        self.request(
            "POST",
            &format!("/api/file-manager/disks/{}/namespace-batch", segment(slot)),
            Some(&body),
        )
        .await
    }
    /// Read the exact active directory marker before an explicit restore.
    pub async fn directory_deletions(
        &self,
        repository: &str,
        owner: &str,
        continuation: Option<(&str, &str)>,
    ) -> Result<storage_contract::directory_git::DirectoryDeletionsPage, Error> {
        let mut path = format!(
            "/api/repositories/{}/directory-deletions/{}",
            segment(repository),
            segment(owner)
        );
        if let Some((after, snapshot)) = continuation {
            path.push_str(&format!(
                "?after={}&snapshot={}",
                segment(after),
                segment(snapshot)
            ));
        }
        Ok(json_codec::from_value(
            self.request("GET", &path, None).await?,
        )?)
    }

    pub async fn directory_deletion(
        &self,
        repository: &str,
        owner: &str,
        directory: &str,
    ) -> Result<Option<storage_contract::directory_git::DirectoryDeletionNode>, Error> {
        Ok(json_codec::from_value(
            self.request(
                "GET",
                &format!(
                    "/api/repositories/{}/directory-deletions/{}/{}",
                    segment(repository),
                    segment(owner),
                    segment(directory)
                ),
                None,
            )
            .await?,
        )?)
    }

    /// Restore one exact directory D as one operation. Preserve this full body
    /// and operation ID after an unknown response; do not issue per-file undos.
    pub async fn undo_directory_deletion(
        &self,
        repository: &str,
        request: &storage_contract::directory_git::UndoDirectoryDeletionBody,
    ) -> Result<Value, Error> {
        self.request(
            "POST",
            &format!(
                "/api/repositories/{}/directory-deletions/undo",
                segment(repository)
            ),
            Some(&json_codec::to_value(request)?),
        )
        .await
    }

    /// Rename a public file branch label without changing content or selection.
    pub async fn rename_file_branch(
        &self,
        file: &str,
        branch: &str,
        name: &str,
        operation_id: &str,
    ) -> Result<Value, Error> {
        self.request(
            "PATCH",
            &format!(
                "/api/file-manager/files/{}/branches/{}",
                segment(file),
                segment(branch)
            ),
            Some(&json!({"name":name,"operation_id":operation_id})),
        )
        .await
    }
    /// Create a branch from the explicit node and its body version, without selecting it.
    /// The server checks that the node belongs to the file and references `version`.
    pub async fn create_file_branch_from_node(
        &self,
        file: &str,
        version: &str,
        node: &str,
        name: &str,
        operation_id: &str,
    ) -> Result<Value, Error> {
        self.request("POST",&format!("/api/file-manager/files/{}/branches",segment(file)),
            Some(&json!({"source_version_id":version,"source_node_id":node,"name":name,"operation_id":operation_id}))).await
    }
    /// Set a path, tag, or merge node's independent note; an empty note clears it.
    /// This does not edit a content version's note or create a new node.
    pub async fn set_file_path_note(
        &self,
        file: &str,
        node: &str,
        note: &str,
        operation_id: &str,
    ) -> Result<Value, Error> {
        self.request(
            "PATCH",
            &format!(
                "/api/file-manager/files/{}/nodes/{}",
                segment(file),
                segment(node)
            ),
            Some(&json!({"note":note,"operation_id":operation_id})),
        )
        .await
    }
    /// Merge reviewed public file-branch heads into one explicitly owned result.
    /// Calling this method submits `confirmed: true`; obtain user confirmation first.
    /// No repository/path precondition is sent. Preserve all inputs for a retry.
    pub async fn merge_file_branches(
        &self,
        file: &str,
        inputs: &[storage_contract::FileMergeInput],
        chosen_node: &str,
        surviving_branch: &str,
        operation_id: &str,
    ) -> Result<Value, Error> {
        self.request("POST", &format!("/api/file-manager/files/{}/merge", segment(file)),
            Some(&json!({"inputs":inputs,"chosen_node_id":chosen_node,"surviving_branch_id":surviving_branch,"confirmed":true,"operation_id":operation_id}))).await
    }
    /// Read one immutable directory-history page without changing its selection.
    pub async fn directory_history(
        &self,
        request: &storage_contract::directory_history::DirectoryHistoryRequest,
    ) -> Result<storage_contract::directory_history::DirectoryHistoryPage, Error> {
        let mut query = url_encoding::form::Serializer::new(String::new());
        for (key, value) in request.query_fields() {
            query.append_pair(&key, &value);
        }
        let route = format!("{}?{}", request.route(), query.finish());
        let page: storage_contract::directory_history::DirectoryHistoryPage =
            json_codec::from_value(self.request("GET", &route, None).await?)?;
        page.validate(request)?;
        Ok(page)
    }
    pub async fn file_history_source(
        &self,
        request: &storage_contract::file_history_merge::FileHistorySourceRequest,
    ) -> Result<storage_contract::file_history_merge::FileHistorySource, Error> {
        let query = url_encoding::form::Serializer::new(String::new())
            .extend_pairs(request.query_fields())
            .finish();
        let result: storage_contract::file_history_merge::FileHistorySource =
            json_codec::from_value(
                self.request("GET", &format!("{}?{query}", request.route()), None)
                    .await?,
            )?;
        result.validate(request)?;
        Ok(result)
    }
    /// Merge reviewed historical nodes onto a new named branch, preserving all source heads.
    /// Calling this method submits `confirmed: true`; obtain user confirmation first.
    /// No repository/path precondition is sent. Preserve all inputs for a retry.
    pub async fn merge_file_history(
        &self,
        file: &str,
        inputs: &[storage_contract::FileHistoryMergeInput],
        chosen_node: &str,
        name: &str,
        operation_id: &str,
    ) -> Result<Value, Error> {
        self.request("POST", &format!("/api/file-manager/files/{}/merge-history", segment(file)),
            Some(&json!({"inputs":inputs,"chosen_node_id":chosen_node,"name":name,"confirmed":true,"operation_id":operation_id}))).await
    }
    /// Read the labels of this exact current file-container occurrence.
    pub async fn file_container_tags(
        &self,
        request: &storage_contract::file_container_tags::ReadRequest,
    ) -> Result<storage_contract::file_container_tags::Snapshot, Error> {
        let query = url_encoding::form::Serializer::new(String::new())
            .append_pair("repository_id", &request.context.repository_id.to_string())
            .append_pair("path", request.context.path.as_str())
            .finish();
        let route = format!("/api/file-manager/files/{}/tags?{query}", request.file_id);
        let snapshot: storage_contract::file_container_tags::Snapshot =
            json_codec::from_value(self.request("GET", &route, None).await?)?;
        snapshot
            .validate(request)
            .map_err(|source| Error::Contract(source.into_error()))?;
        Ok(snapshot)
    }

    /// Replace the complete container label set against its observed generation.
    /// Preserve the entire command and operation ID after an uncertain response.
    pub async fn replace_container_tags(
        &self,
        command: &storage_contract::file_container_tags::ReplaceCommand,
        operation_id: storage_contract::OperationId,
    ) -> Result<Value, Error> {
        self.request(
            "POST",
            &format!("/api/file-manager/files/{}/tags", command.file_id),
            Some(&json!({
                "repository_id": command.context.repository_id,
                "path": command.context.path,
                "expected_generation": command.expected_generation,
                "tags": command.tags,
                "operation_id": operation_id,
            })),
        )
        .await
    }
    /// Read backup policy and scope identities; an empty path selects the repository root.
    pub async fn backup_configuration(&self, repository: &str, path: &str) -> Result<Value, Error> {
        self.request(
            "GET",
            &format!(
                "/api/repositories/{}/backup-configuration?path={}",
                segment(repository),
                segment(path)
            ),
            None,
        )
        .await
    }
    /// Submit the caller's scope and own-policy generation without refreshing either.
    /// Preserve the operation ID and the full request after an uncertain response.
    pub async fn configure_backups(
        &self,
        request: BackupConfigurationRequest<'_>,
    ) -> Result<Value, Error> {
        let BackupConfigurationRequest {
            repository_id: repository,
            path,
            target_copies,
            backup_prefixes,
            expected_generation,
            expected_scope_id,
            operation_id,
        } = request;
        self.request("PUT", &format!("/api/repositories/{}/backup-configuration",segment(repository)),
            Some(&json!({"path":path,"target_copies":target_copies,"backup_prefixes":backup_prefixes,"expected_generation":expected_generation.to_string(),"expected_scope_id":expected_scope_id,"operation_id":operation_id}))).await
    }
    pub async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, Error> {
        let method = reqwest::Method::from_bytes(method.as_bytes())?;
        let mut request = self.builder(method, path)?;
        if let Some(body) = body {
            request = request.json(body);
        }
        decode(request.send().await?).await
    }
}

pub fn segment(value: &str) -> String {
    url_encoding::form::byte_serialize(value.as_bytes())
        .map(|part| if part == "+" { "%20" } else { part })
        .collect()
}

#[cfg(test)]
#[path = "../../../test/client/unit/lib/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../test/client/unit/transfer_tests.rs"]
mod transfer_tests;

#[cfg(test)]
#[path = "../../../test/client/unit/container_tags.rs"]
mod container_tags_tests;
