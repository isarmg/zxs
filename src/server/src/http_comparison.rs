//! Comparison identity parsing belongs to HTTP, before the typed backend call.
use crate::{ApiError, AppState, auth::Identity};
use http_routing::{
    Extension,
    extract::{Request, State},
};
struct CompareBody {
    left: String,
    right: String,
}
impl CompareBody {
    fn parse(value: json_codec::Value) -> Result<Self, ApiError> {
        let json_codec::Value::Object(mut body) = value else {
            return Err(ApiError::invalid("expected object"));
        };
        if let Some(key) = body
            .keys()
            .find(|key| !["left_commit_id", "right_commit_id"].contains(&key.as_str()))
        {
            return Err(ApiError::invalid(format!("unknown field {key}")));
        }
        let mut field = |key: &str| match body.remove(key) {
            Some(json_codec::Value::String(id)) if !id.is_empty() => Ok(id),
            _ => Err(ApiError::invalid(format!("missing string field {key}"))),
        };
        let left = field("left_commit_id")?;
        let right = field("right_commit_id")?;
        Ok(Self { left, right })
    }
}
pub(crate) async fn compare(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    request: Request,
) -> Result<http_routing::response::Response, ApiError> {
    let connection = request
        .extensions()
        .get::<crate::http_shutdown::ResponseConnection>()
        .cloned();
    let value = crate::json_body(request).await?;
    crate::history_read::run(
        state,
        connection,
        move |backend, control| {
            let body = CompareBody::parse(value)?;
            backend.compare_commits(
                &identity,
                crate::CompareCommits {
                    left: &body.left,
                    right: &body.right,
                },
                control,
            )
        },
        |snapshot| Ok(snapshot.into_encoded()),
    )
    .await
}
#[cfg(test)]
#[path = "../../../test/server/unit/http_comparison.rs"]
mod tests;
