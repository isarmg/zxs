//! The engine reads original automatic-node evidence; the HTTP adapter binds
//! its result, including existence of the next row, to the current read authority.
use super::{
    EngineBackend, application::Application, current_protected_identity, error, repository,
};
use crate::{ApiError, AutomaticWindowRead, auth::Identity};
use storage_contract::automatic_observation::{Binding, Position, Reply, Request, Scope};
use storage_contract::snapshot_worktree::NamespaceDirectoryId;
use storage_contract::{NamespaceEntryKind, Principal};
use storage_engine::Engine;

fn current_scope(
    engine: &Engine,
    current: &Identity,
    read: &AutomaticWindowRead,
) -> Result<Scope, ApiError> {
    if current.smb_binding.is_some() {
        return Err(ApiError::forbidden());
    }
    let registered = repository(engine, &read.repository.to_string())?;
    if !registered.file_manager_root {
        return Err(ApiError::forbidden());
    }
    // The ordinary root and nested entries use the same registered main tree
    // as the file manager. A pathname, historical entry or FileId alone cannot
    // select another owner or prove authorization.
    let directory = match &read.path {
        None => NamespaceDirectoryId::Repository(read.repository),
        Some(path) => {
            let tree = engine
                .namespace_tree(&registered.main_branch_id)
                .map_err(error)?;
            let entry = tree
                .get_key(path.key())
                .filter(|entry| entry.kind == NamespaceEntryKind::Directory)
                .ok_or_else(ApiError::forbidden)?;
            let row = engine
                .state()
                .namespace_directories
                .get(&entry.entry_id)
                .filter(|row| {
                    row.repository == read.repository
                        && row.branch.to_string() == registered.main_branch_id
                })
                .ok_or_else(ApiError::forbidden)?;
            NamespaceDirectoryId::Entry(row.entry)
        }
    };
    Ok(Scope {
        repository: read.repository,
        directory,
        start: read.start,
        end: read.end,
    })
}

pub(super) fn read(
    backend: &EngineBackend,
    identity: &Identity,
    input: AutomaticWindowRead,
) -> Result<Reply, ApiError> {
    let (mut engine, current) = backend.lock_current(identity)?;
    let session = current
        .admission_session
        .as_deref()
        .ok_or_else(ApiError::forbidden)?;
    let request = Request {
        scope: current_scope(&engine, &current, &input)?,
        position: input.position,
    };
    let principal: Principal = current
        .principal
        .parse()
        .map_err(ApiError::invalid_authenticated_principal)?;
    request.validate().map_err(ApiError::invalid_source)?;
    match (request.position, input.binding) {
        (Position::First, None) => {}
        (Position::After { .. }, Some(binding))
            if binding
                == Binding::for_session(&principal, session, &request)
                    .map_err(ApiError::contract_input)? => {}
        _ => {
            return Err(ApiError::invalid(
                "continuation does not match this account and directory window",
            ));
        }
    }
    let (page, lookahead) = engine.automatic_node_page(&request).map_err(error)?;
    page.validate(&request).map_err(|source| {
        ApiError::new(
            ::http::StatusCode::INTERNAL_SERVER_ERROR,
            storage_contract::ErrorCode::StateInvalid,
            "automatic publication observation is incomplete",
        )
        .with_source(source)
    })?;
    if page.next.is_some() != lookahead.is_some() {
        return Err(ApiError::new(
            ::http::StatusCode::INTERNAL_SERVER_ERROR,
            storage_contract::ErrorCode::StateInvalid,
            "incomplete automatic publication lookahead",
        ));
    }
    // Verify the actual rows (not the remembered names/paths), under the same
    // engine ownership interval. On denial return no IDs, count or continuation.
    Application::new(&mut engine, &current)
        .authorize_automatic_page(&page, lookahead)
        .map_err(|_| ApiError::forbidden())?;
    let next_binding = page
        .continuation()
        .as_ref()
        .map(|next| Binding::for_session(&principal, session, next))
        .transpose()
        .map_err(ApiError::contract_input)?;
    // A bounded page can still take time on cold original receipts; revalidate
    // the original management session after the last row/next-row decision.
    current_protected_identity(&engine, identity)?;
    Ok(Reply {
        path: input.path,
        page,
        next_binding,
    })
}
