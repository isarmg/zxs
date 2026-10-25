//! Shared native client. Redirects are disabled and plain HTTP is restricted to loopback.
use data_serialization::{Deserialize, Serialize};
use json_codec::{Value, json};
use std::time::Duration;

#[cfg(target_os = "linux")]
mod download_target;
mod response;
mod transfer;
use response::decode;
pub use transfer::PreparedUpload;

#[derive(Debug, error_definition::Error)]
pub enum Error {
    #[error("invalid server address: {0}")]
    Address(String),
    #[error("{code}: {message}")]
    Api {
        status: u16,
        code: storage_contract::ErrorCode,
        message: String,
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
        let session: Self =
            json_codec::from_value(value).map_err(|error| Error::Response(error.to_string()))?;
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
        let base = url_encoding::address::Url::parse(address)
            .map_err(|error| Error::Address(error.to_string()))?;
        let loopback = base.host_str().is_some_and(|host| {
            host == "localhost" || host == "127.0.0.1" || host == "[::1]" || host == "::1"
        });
        if !matches!(base.scheme(), "https" | "http")
            || (base.scheme() == "http" && !loopback)
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || base.path() != "/"
        {
            return Err(Error::Address("use an HTTPS origin, or HTTP on loopback; paths, credentials and query strings are prohibited".into()));
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
        storage_contract::validate_api_path(path)
            .map_err(|error| Error::Address(error.message().into_owned()))?;
        let url = self
            .base
            .join(path)
            .map_err(|error| Error::Address(error.to_string()))?;
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
        .map_err(|error| Error::Response(error.to_string()))
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
    pub async fn directory_namespace_batch(
        &self,
        slot: &str,
        request: &storage_contract::DirectoryNamespaceBatch,
        operation_id: &str,
    ) -> Result<Value, Error> {
        let mut body =
            json_codec::to_value(request).map_err(|error| Error::Response(error.to_string()))?;
        body["operation_id"] = operation_id.into();
        self.request(
            "POST",
            &format!("/api/file-manager/disks/{}/namespace-batch", segment(slot)),
            Some(&body),
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
    pub async fn update_file_node_tags(
        &self,
        file: &str,
        targets: &[storage_contract::FileTagTarget],
        add: &[String],
        remove: &[String],
        operation_id: &str,
    ) -> Result<Value, Error> {
        self.request(
            "POST",
            &format!("/api/file-manager/files/{}/tags", segment(file)),
            Some(&json!({"targets":targets,"add":add,"remove":remove,"operation_id":operation_id})),
        )
        .await
    }
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
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|error| Error::Address(error.to_string()))?;
        let mut request = self.builder(method, path)?;
        if let Some(body) = body {
            request = request.json(body);
        }
        decode(request.send().await?).await
    }
}

pub fn segment(value: &str) -> String {
    url_encoding::form::byte_serialize(value.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

#[cfg(test)]
#[path = "../../../test/client/unit/lib/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../test/client/unit/transfer_tests.rs"]
mod transfer_tests;
