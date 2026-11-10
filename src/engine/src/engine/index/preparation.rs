//! Encode the existing system projection without opening a live database path.
//! This is a derived material of an admitted State, never a recovery authority.
use super::*;
use crate::history_package::{AdmittedState, preparation_checkpoint};
use sqlite_access::sqlite::{SqliteConnectOptions, SqliteJournalMode};
use std::sync::atomic::AtomicBool;

pub(super) mod readback;

pub(in crate::engine) const MAX_BYTES: usize = 64 * 1024 * 1024;

pub(in crate::engine) fn encode(admitted: &AdmittedState, stop: &AtomicBool) -> Result<Vec<u8>> {
    encode_observed(admitted, &|| preparation_checkpoint(stop))
}

pub(in crate::engine) fn encode_observed(
    admitted: &AdmittedState,
    checkpoint: &impl Fn() -> Result<()>,
) -> Result<Vec<u8>> {
    encode_state(admitted.state(), checkpoint)
}

/// A private unaccepted move draft is not an admitted snapshot. Keep this
/// staging-only entry distinct from the ordinary admitted-State encoder.
pub(in crate::engine) fn encode_move_draft(
    draft: &crate::business::NativeMoveDraft,
    cancellation: &HistoryWindowCancellation,
) -> Result<Vec<u8>> {
    encode_state(draft.proposed_state(), &|| {
        if cancellation.is_cancelled() {
            return Err(Error::new(
                ErrorCode::OperationAborted,
                "move SYSTEM preparation cancelled",
            ));
        }
        Ok(())
    })
}

/// Candidate outcome remains private and unaccepted. Encoding it does not
/// create a published index, final operation receipt or journal decision.
pub(in crate::engine) fn encode_move_candidate(
    draft: &crate::business::NativeMoveCandidate,
    checkpoint: &impl Fn() -> Result<()>,
) -> Result<Vec<u8>> {
    encode_state(draft.state(), checkpoint)
}

fn encode_state(state: &State, checkpoint: &impl Fn() -> Result<()>) -> Result<Vec<u8>> {
    checkpoint()?;
    // Bound the entire input before SQL rows duplicate parts of it. This is a
    // preparation budget, not a limit silently truncating the serving index.
    crate::disk::encode_metadata(state, MAX_BYTES)?;
    let bytes = crate::sqlite_connection::run(
        SqliteConnectOptions::new()
            .in_memory(true)
            .journal_mode(SqliteJournalMode::Memory),
        async |connection| {
            connection.execute("PRAGMA page_size=4096").await?;
            connection.execute("PRAGMA max_page_count=16384").await?;
            let mut tx = connection.begin().await?;
            recreate_projection(&mut tx).await.map_err(Error::from)?;
            for (id, version) in state.versions.iter() {
                checkpoint()?;
                sqlite_access::query("INSERT INTO file_history_versions VALUES(?,?,?,?)")
                    .bind(id)
                    .bind(&version.file_id)
                    .bind(&version.history_tree_id)
                    .bind(version.version_sequence.to_be_bytes().as_slice())
                    .execute(&mut *tx)
                    .await?;
            }
            scope::Delta::between(None, state)
                .project(&mut tx, state)
                .await
                .map_err(Error::from)?;
            checkpoint()?;
            system_changes::Delta::between(None, state)
                .project(&mut tx, state)
                .await
                .map_err(Error::from)?;
            sqlite_access::query("INSERT INTO current_state VALUES(1,?)")
                .bind(state.sequence.to_string())
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            checkpoint()?;
            let bytes = connection.serialize(None).await?;
            if bytes.len() > MAX_BYTES {
                return Err(Error::new(
                    ErrorCode::ResourceLimit,
                    "prepared system index exceeds 64 MiB",
                ));
            }
            Ok(bytes.to_vec())
        },
    )?;
    // Close the encoding connection before decoding the actual serialized
    // image. No stored self-digest can replace the original source State.
    readback::verify(&bytes, state, checkpoint)?;
    Ok(bytes)
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/index/preparation_readback_tests.rs"]
mod tests;
