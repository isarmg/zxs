//! Typed read request parsing and response projection at the HTTP boundary.
use crate::{ApiError, AppState, auth::Identity, http_boundary::ApiPath};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};

struct ResolveBody {
    path: name_policy::LookupPath,
    commit_id: Option<String>,
}
impl ResolveBody {
    // Preserve unknown-field, path, then supplied-identity error precedence.
    // JSON bytes/duplicate keys have already been decoded at the HTTP boundary.
    fn parse(body: &json_codec::Map<String, json_codec::Value>) -> Result<Self, ApiError> {
        if let Some(key) = body
            .keys()
            .find(|key| !["branch_id", "path", "commit_id"].contains(&key.as_str()))
        {
            return Err(ApiError::invalid(format!("unknown field {key}")));
        }
        let path = body
            .get("path")
            .and_then(json_codec::Value::as_str)
            .filter(|path| !path.is_empty())
            .ok_or_else(|| ApiError::invalid("missing string field path"))?;
        let path = name_policy::LookupPath::parse(path).map_err(ApiError::invalid_source)?;
        let commit_id = body
            .get("commit_id")
            .map(|value| {
                value
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        ApiError::invalid("commit_id must be a nonempty string when supplied")
                    })
            })
            .transpose()?;
        Ok(Self { path, commit_id })
    }
}
pub(crate) async fn resolve(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(branch): ApiPath<String>,
    request: Request,
) -> Result<Json<crate::backend_read::ResolveResponse>, ApiError> {
    let value = crate::with_id(
        crate::json_body(request).await?,
        "branch_id",
        branch.clone(),
    )?;
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let body = ResolveBody::parse(
            value
                .as_object()
                .expect("invariant: with_id binds an object body"),
        )?;
        state
            .backend
            .resolve_entry(
                &identity,
                crate::ResolveEntry {
                    branch: &branch,
                    path: &body.path,
                    commit: body.commit_id.as_deref(),
                },
            )
            .map(crate::backend_read::ResolveResponse::from)
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}

#[cfg(test)]
#[path = "../../../test/server/unit/http_read.rs"]
mod tests;
