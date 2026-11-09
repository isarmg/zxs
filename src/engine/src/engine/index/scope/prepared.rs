//! Verify every prepared membership against the same frozen source State.
use super::*;

pub(in crate::engine::index) async fn verify_prepared(
    connection: &mut sqlite_access::SqliteConnection,
    state: &State,
    checkpoint: &impl Fn() -> Result<()>,
) -> Result<()> {
    let delta = Delta::between(None, state);
    let mut expected = 0usize;
    for kind in 0..6 {
        for id in delta.keys(state, kind) {
            for (position, file, history) in members(state, kind, id) {
                checkpoint()?;
                let count: i64 = sqlite_access::query_scalar(
                    "SELECT count(*) FROM history_scope_members WHERE kind=? AND id=? AND entry_key=? AND file_id=? AND history_tree_id=?",
                )
                .bind(kind as i64)
                .bind(id)
                .bind(position)
                .bind(file)
                .bind(history)
                .fetch_one(&mut *connection)
                .await?;
                if count != 1 {
                    return Err(Error::new(
                        ErrorCode::ContainerLayoutInvalid,
                        "prepared SYSTEM membership differs from its source State",
                    ));
                }
                expected += 1;
            }
        }
    }
    let count: i64 =
        sqlite_access::query_scalar("SELECT count(*) FROM history_scope_members NOT INDEXED")
            .fetch_one(&mut *connection)
            .await?;
    // Input State, including commit entries, is bounded to 64 MiB here.
    if count != expected as i64 {
        return Err(Error::new(
            ErrorCode::ContainerLayoutInvalid,
            "prepared SYSTEM membership contains omitted or extra rows",
        ));
    }
    Ok(())
}
