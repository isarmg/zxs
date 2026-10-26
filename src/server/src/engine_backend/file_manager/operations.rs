//! Strict wire requests for node adoption, inherited tags and backup policy.
use super::*;

fn location(input: &Value) -> Result<Option<storage_engine::FileLocation>, ApiError> {
    let requested_repo = input.get("repository_id");
    let requested_path = input.get("path");
    if requested_repo.is_some() != requested_path.is_some() {
        return Err(ApiError::invalid(
            "repository_id and path must be supplied together",
        ));
    }
    if requested_repo.is_none() {
        return Ok(None);
    }
    Ok(Some(storage_engine::FileLocation {
        repository_id: field(input, "repository_id")?.into(),
        path: field(input, "path")?.into(),
    }))
}

pub(super) fn call(
    engine: &mut Engine,
    identity: &Identity,
    action: crate::BackendAction,
    input: &Value,
) -> Result<Value, ApiError> {
    match action {
        crate::BackendAction::FileManagerMerge => {
            keys(
                input,
                &[
                    "file_id",
                    "inputs",
                    "chosen_node_id",
                    "surviving_branch_id",
                    "confirmed",
                    "repository_id",
                    "path",
                    "operation_id",
                ],
            )?;
            let file = field(input, "file_id")?;
            let inputs: Vec<storage_contract::FileMergeInput> = json_codec::from_value(
                input
                    .get("inputs")
                    .cloned()
                    .ok_or_else(|| ApiError::invalid("inputs are required"))?,
            )
            .map_err(ApiError::invalid_source)?;
            let result = Application::new(engine, identity).merge_file_branches(
                application::MergeFileBranches {
                    file_id: file.into(),
                    inputs,
                    chosen_node_id: field(input, "chosen_node_id")?.into(),
                    surviving_branch_id: field(input, "surviving_branch_id")?.into(),
                    confirmed: input.get("confirmed").and_then(Value::as_bool) == Some(true),
                    location: location(input)?,
                    operation_id: operation(input)?,
                },
            )?;
            operation_wire(result)
        }
        crate::BackendAction::FileManagerTags => {
            keys(
                input,
                &[
                    "file_id",
                    "targets",
                    "add",
                    "remove",
                    "repository_id",
                    "path",
                    "operation_id",
                ],
            )?;
            let file = field(input, "file_id")?;
            let location = location(input)?;
            Application::new(engine, identity).authorize_file(file)?;
            let targets: Vec<storage_contract::FileTagTarget> = json_codec::from_value(
                input
                    .get("targets")
                    .cloned()
                    .ok_or_else(|| ApiError::invalid("targets are required"))?,
            )
            .map_err(ApiError::invalid_source)?;
            let strings = |key: &str| -> Result<Vec<String>, ApiError> {
                json_codec::from_value(
                    input
                        .get(key)
                        .cloned()
                        .ok_or_else(|| ApiError::invalid(format!("{key} is required")))?,
                )
                .map_err(ApiError::invalid_source)
            };
            let result = engine
                .update_file_node_tags(storage_engine::FileNodeTagsUpdateRequest {
                    file_id: file,
                    targets: &targets,
                    add: &strings("add")?,
                    remove: &strings("remove")?,
                    location: location.as_ref(),
                    principal: &identity.principal,
                    operation_id: operation(input)?,
                })
                .map_err(error)?;
            operation_wire(result)
        }
        crate::BackendAction::FileManagerConfigureBackups => {
            keys(
                input,
                &[
                    "repository_id",
                    "path",
                    "target_copies",
                    "backup_prefixes",
                    "expected_generation",
                    "expected_scope_id",
                    "operation_id",
                ],
            )?;
            let repo = field(input, "repository_id")?;
            repository(engine, repo)?;
            let copies = input
                .get("target_copies")
                .and_then(Value::as_u64)
                .ok_or_else(|| ApiError::invalid("target_copies must be a positive integer"))?;
            let copies = u32::try_from(copies).map_err(ApiError::invalid_source)?;
            let prefixes: BTreeMap<String, String> = json_codec::from_value(
                input
                    .get("backup_prefixes")
                    .cloned()
                    .ok_or_else(|| ApiError::invalid("backup_prefixes are required"))?,
            )
            .map_err(ApiError::invalid_source)?;
            let generation =
                storage_contract::parse_u64_decimal(field(input, "expected_generation")?)
                    .map_err(ApiError::invalid_source)?;
            let result = engine
                .configure_backups_scoped(storage_engine::ScopedBackupConfigurationRequest {
                    repository_id: repo,
                    path: path(input)?,
                    expected_scope_id: Some(field(input, "expected_scope_id")?),
                    target_copies: copies,
                    backup_prefixes: &prefixes,
                    expected_generation: generation,
                    principal: &identity.principal,
                    operation_id: operation(input)?,
                })
                .map_err(|failure| match failure.code() {
                    storage_contract::ErrorCode::RouteIncomplete
                    | storage_contract::ErrorCode::ScopeChanged => {
                        ApiError::conflict(failure.code(), &failure.message()).with_source(failure)
                    }
                    _ => error(failure),
                })?;
            operation_wire(result)
        }
        _ => Err(ApiError::new(
            ::http::StatusCode::NOT_FOUND,
            storage_contract::ErrorCode::NotFound,
            "operation not found",
        )),
    }
}
