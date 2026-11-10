//! Complete readback shared by bounded images and the installed WAL connection.
//! The caller's frozen State is the comparison source, not a new authority.
use super::{CURRENT_STATE_TABLE, HISTORY_INDEX, HISTORY_TABLE, MAX_BYTES, scope, system_changes};
use crate::{Error, ErrorCode, Result, State};
use sqlite_access::{Row, SqliteConnection};

fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::ContainerLayoutInvalid, message)
}

pub(super) fn verify(
    bytes: &[u8],
    state: &State,
    checkpoint: &impl Fn() -> Result<()>,
) -> Result<()> {
    // encode_state already bounded both the source and the serialized image.
    checkpoint()?;
    crate::sqlite_connection::read_image(bytes, MAX_BYTES as u32, async |connection| {
        verify_connection(connection, state, checkpoint).await
    })
}

/// The caller owns the snapshot: an immutable decoded image or a read
/// transaction on the installed writer. This does not change writer limits.
pub(in crate::engine::index) async fn verify_connection(
    connection: &mut SqliteConnection,
    state: &State,
    checkpoint: &impl Fn() -> Result<()>,
) -> Result<()> {
    schema(connection).await?;
    crate::sqlite_integrity::verify(connection).await?;
    checkpoint()?;
    marker(connection, state.sequence).await?;
    versions(connection, state, checkpoint).await?;
    scope::verify_prepared(connection, state, checkpoint).await?;
    system_changes::verify_prepared(connection, state, checkpoint).await?;
    checkpoint()
}

async fn schema(connection: &mut SqliteConnection) -> Result<()> {
    let expected = [
        (
            "current_state",
            "table",
            "current_state",
            Some(CURRENT_STATE_TABLE),
        ),
        (
            "file_history_lookup",
            "index",
            "file_history_versions",
            Some(HISTORY_INDEX),
        ),
        (
            "file_history_versions",
            "table",
            "file_history_versions",
            Some(HISTORY_TABLE),
        ),
        (
            "history_scope_lookup",
            "index",
            "history_scope_members",
            Some(scope::LOOKUP),
        ),
        (
            "history_scope_members",
            "table",
            "history_scope_members",
            Some(scope::TABLE),
        ),
        (
            "sqlite_autoindex_file_history_versions_1",
            "index",
            "file_history_versions",
            None,
        ),
        (
            "sqlite_autoindex_history_scope_members_1",
            "index",
            "history_scope_members",
            None,
        ),
        (
            "sqlite_autoindex_system_changes_1",
            "index",
            "system_changes",
            None,
        ),
        (
            "system_change_order",
            "index",
            "system_changes",
            Some(system_changes::LOOKUP),
        ),
        (
            "system_changes",
            "table",
            "system_changes",
            Some(system_changes::TABLE),
        ),
    ];
    crate::sqlite_connection::require_schema(connection, &expected, "prepared SYSTEM image").await
}

async fn marker(connection: &mut SqliteConnection, sequence: u64) -> Result<()> {
    let rows = sqlite_access::query(
        "SELECT singleton,substr(CAST(sequence AS BLOB),1,21) FROM current_state LIMIT 2",
    )
    .fetch_all(&mut *connection)
    .await?;
    if rows.len() != 1
        || rows[0].try_get::<i64, _>(0)? != 1
        || rows[0].try_get::<Vec<u8>, _>(1)? != sequence.to_string().as_bytes()
    {
        return Err(invalid("prepared SYSTEM image has another source sequence"));
    }
    Ok(())
}

async fn versions(
    connection: &mut SqliteConnection,
    state: &State,
    checkpoint: &impl Fn() -> Result<()>,
) -> Result<()> {
    for (id, version) in state.versions.iter() {
        checkpoint()?;
        let count: i64 = sqlite_access::query_scalar(
            "SELECT count(*) FROM file_history_versions WHERE version_id=? AND file_id=? AND history_tree_id=? AND version_sequence_key=?",
        )
        .bind(id)
        .bind(&version.file_id)
        .bind(&version.history_tree_id)
        .bind(version.version_sequence.to_be_bytes().as_slice())
        .fetch_one(&mut *connection)
        .await?;
        if count != 1 {
            return Err(invalid(
                "prepared SYSTEM history row differs from its source State",
            ));
        }
    }
    let count: i64 =
        sqlite_access::query_scalar("SELECT count(*) FROM file_history_versions NOT INDEXED")
            .fetch_one(&mut *connection)
            .await?;
    // The source was serialized within 64 MiB; its row count fits in i64.
    if count != state.versions.len() as i64 {
        return Err(invalid(
            "prepared SYSTEM history contains omitted or extra rows",
        ));
    }
    Ok(())
}
