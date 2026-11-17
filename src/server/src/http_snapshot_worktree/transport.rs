//! One complete edit and explicit commit over the existing authenticated body
//! staging and verified response stream. Ordinary SMB save semantics are separate.
use crate::http_boundary::{ApiJson, ApiPath, ApiQuery, Authenticated, JsonBody};
use crate::{ApiError, AppState};
use data_serialization::Deserialize;
use http::{HeaderMap, Method};
use http_routing::{Json, extract::{Request, State}, response::Response};
use storage_contract::snapshot_worktree::{
    CommitWorktree, EditWorktree, SnapshotHandle, WorktreeHandle, WorktreeId, WorktreeRevision,
};

pub(crate) struct CommitBody(CommitWorktree);
impl JsonBody for CommitBody {
    fn from_json(value: json_codec::Value) -> Result<Self, ApiError> {
        named_target_revision(&value)?;
        json_codec::from_value(value).map(Self).map_err(ApiError::invalid_source)
    }
}

fn named_target_revision(value: &json_codec::Value) -> Result<(), ApiError> {
    if !value.get("target").is_some_and(json_codec::Value::is_object)
        || !value.get("expected").is_some_and(json_codec::Value::is_object)
    {
        return Err(ApiError::invalid("target and expected must be named objects"));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct ContentQuery {
    publication: storage_contract::OperationId,
    path: name_policy::LookupPath,
}

pub(crate) async fn edit(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    ApiPath(worktree): ApiPath<WorktreeId>,
    request: Request,
) -> Result<Json<WorktreeRevision>, ApiError> {
    let budget = state.http_spool()?;
    let headers = request.headers();
    if headers.get_all("x-zxs-worktree-edit").iter().count() != 1 {
        return Err(ApiError::invalid("exactly one X-ZXS-Worktree-Edit is required"));
    }
    let encoded = headers.get("x-zxs-worktree-edit")
        .ok_or_else(|| ApiError::invalid("X-ZXS-Worktree-Edit is required"))?;
    if encoded.as_bytes().len() > 16_384 {
        return Err(ApiError::limit("worktree edit header exceeds its byte budget"));
    }
    let value = storage_contract::strict_json(encoded.as_bytes())
        .map_err(ApiError::contract_input)?;
    named_target_revision(&value)?;
    let command: EditWorktree = json_codec::from_value(value).map_err(ApiError::invalid_source)?;
    if command.target.worktree_id != worktree {
        return Err(ApiError::invalid("worktree edit target differs from route"));
    }
    crate::http_complete_upload::receive(state, identity, command, request, budget).await.map(Json)
}

impl crate::http_complete_upload::CompleteUpload for EditWorktree {
    type Output = WorktreeRevision;

    fn size(&self) -> u64 { self.size }
    fn hash(&self) -> storage_contract::ContentHash { self.content_hash }
    fn claim_key(&self) -> String {
        // The edit changes the whole instance checkpoint, not only this path.
        format!("worktree:{}:{}", self.target.publication, self.target.worktree_id)
    }
    fn authorize(&self, backend: &dyn crate::Backend, identity: &crate::Identity)
        -> Result<(), ApiError>
    {
        backend.authorize_directory_worktree_edit(identity, self)
    }
    fn publish(
        self,
        backend: &dyn crate::Backend,
        identity: &crate::Identity,
        source: &crate::UploadSource,
    ) -> Result<Self::Output, ApiError> {
        backend.edit_directory_worktree(identity, self, source)
    }
}

pub(crate) async fn commit(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    ApiPath(worktree): ApiPath<WorktreeId>,
    ApiJson(body): ApiJson<CommitBody>,
) -> Result<Json<SnapshotHandle>, ApiError> {
    if body.0.target.worktree_id != worktree {
        return Err(ApiError::invalid("worktree commit target differs from route"));
    }
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| backend.commit_directory_worktree(&identity, body.0))
        .await.map(Json)
}

pub(crate) async fn content(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    ApiPath(worktree_id): ApiPath<WorktreeId>,
    ApiQuery(query): ApiQuery<ContentQuery>,
    method: Method,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let limits = state.config.http_transfer_limits.clone();
    let deadline = tokio::time::Instant::now() + limits.total_timeout;
    let permit = state.read_slots.acquire()?;
    let target = WorktreeHandle { publication: query.publication, worktree_id };
    let content = tokio::time::timeout_at(
        deadline,
        crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
            .run_with_permit(move |backend, permit| {
                let mut content = backend.directory_worktree_content(
                    &identity, target, query.path, deadline.into_std(),
                )?;
                content.lease = Box::new((content.lease, permit));
                Ok(content)
            }),
    ).await.map_err(|source| crate::transfer_limits::timeout_error().with_source(source))??;
    crate::transfer::content_response_with_limits(content, method, headers, &limits, deadline)
}
