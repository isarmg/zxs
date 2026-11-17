//! Current authenticated management reads. No route here can start a save,
//! publish a node, or resume the automatic controller.
use crate::http_boundary::{ApiPath, ApiQuery, Authenticated};
use crate::{ApiError, AppState};
use data_serialization::Deserialize;
use http_routing::{Json, extract::State};
use name_policy::PortablePath;
use storage_contract::automatic_observation::{Binding, NodeKey, Position, Reply};
use storage_contract::{FileId, FileNodeId, RepositoryId, Sequence, Timestamp};

#[derive(Debug, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Query {
    path: Option<PortablePath>,
    start: Timestamp,
    end: Timestamp,
    observed: Option<Sequence>,
    last_file: Option<FileId>,
    last_node: Option<FileNodeId>,
    binding: Option<Binding>,
}

/// A client path is only a lookup input. Inside the held backend context it
/// must resolve to the currently visible registered directory for this repo.
#[derive(Clone, Debug)]
pub struct AutomaticWindowRead {
    pub repository: RepositoryId,
    pub path: Option<PortablePath>,
    pub start: Timestamp,
    pub end: Timestamp,
    pub position: Position,
    pub binding: Option<Binding>,
}
impl Query {
    fn request(self, repository: RepositoryId) -> Result<AutomaticWindowRead, ApiError> {
        if self.start >= self.end {
            return Err(ApiError::invalid("start must precede end"));
        }
        let position = match (self.observed, self.last_file, self.last_node, self.binding) {
            (None, None, None, None) => Position::First,
            (Some(observed), Some(file), Some(node), Some(_)) if observed.get() != 0 => {
                Position::After {
                    observed,
                    last: NodeKey { file, node },
                }
            }
            _ => {
                return Err(ApiError::invalid(
                    "incomplete automatic publication continuation",
                ));
            }
        };
        Ok(AutomaticWindowRead {
            repository,
            path: self.path,
            start: self.start,
            end: self.end,
            position,
            binding: self.binding,
        })
    }
}

pub(crate) async fn read(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    ApiPath(repository): ApiPath<RepositoryId>,
    ApiQuery(query): ApiQuery<Query>,
) -> Result<Json<Reply>, ApiError> {
    let request = query.request(repository)?;
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| backend.automatic_node_observation(&identity, request))
        .await
        .map(Json)
}

#[cfg(test)]
#[path = "../../../test/server/unit/http_automatic_observation.rs"]
mod tests;
