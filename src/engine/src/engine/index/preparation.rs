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
    let input = SystemEncodingInput::new(admitted, checkpoint)?;
    let bytes = input.encode(checkpoint)?;
    input.verify(&bytes, checkpoint)?;
    Ok(bytes)
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

/// One bounded immutable SYSTEM encoder input. This value contains no proof of
/// a live database or target binding. Reused bytes still require verify().
pub(in crate::engine) struct SystemEncodingInput<'a> {
    state: &'a State,
    input_hash: [u8; 32],
}

impl<'a> SystemEncodingInput<'a> {
    pub(in crate::engine) fn new(
        admitted: &'a AdmittedState,
        checkpoint: &impl Fn() -> Result<()>,
    ) -> Result<Self> {
        Self::from_state(admitted.state(), checkpoint)
    }

    fn from_state(state: &'a State, checkpoint: &impl Fn() -> Result<()>) -> Result<Self> {
        checkpoint()?;
        // Preserve the original complete input bound before SQL projection.
        let bytes = crate::disk::encode_metadata(state, MAX_BYTES)?;
        Ok(Self {
            state,
            input_hash: *blake3::hash(&bytes).as_bytes(),
        })
    }

    pub(in crate::engine) fn input_hash(&self) -> [u8; 32] {
        self.input_hash
    }

    /// Encode only. The caller must run verify() before returning or retaining
    /// these bytes, just as it must for bytes supplied by an encoding memo.
    pub(in crate::engine) fn encode(
        &self,
        checkpoint: &impl Fn() -> Result<()>,
    ) -> Result<Vec<u8>> {
        checkpoint()?;
        encode_sql(self.state, checkpoint)
    }

    pub(in crate::engine) fn verify(
        &self,
        bytes: &[u8],
        checkpoint: &impl Fn() -> Result<()>,
    ) -> Result<()> {
        checkpoint()?;
        require_output_size(bytes.len())?;
        readback::verify(bytes, self.state, checkpoint)
    }
}

fn require_output_size(bytes: usize) -> Result<()> {
    if bytes > MAX_BYTES {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "prepared system index exceeds 64 MiB",
        ));
    }
    Ok(())
}

fn encode_state(state: &State, checkpoint: &impl Fn() -> Result<()>) -> Result<Vec<u8>> {
    let input = SystemEncodingInput::from_state(state, checkpoint)?;
    let bytes = input.encode(checkpoint)?;
    input.verify(&bytes, checkpoint)?;
    Ok(bytes)
}

fn encode_sql(state: &State, checkpoint: &impl Fn() -> Result<()>) -> Result<Vec<u8>> {
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
            require_output_size(bytes.len())?;
            Ok(bytes.to_vec())
        },
    )?;
    Ok(bytes)
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/index/preparation_readback_tests.rs"]
mod tests;
