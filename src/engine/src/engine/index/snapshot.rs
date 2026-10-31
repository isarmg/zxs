//! A disposable SQLite read snapshot captured briefly under the Engine lock.
//! It retains no State and consumes its connection once, outside that lock.
use super::*;
use std::sync::Mutex;

pub(crate) struct OwnedScopeRead {
    connection: Mutex<Option<sqlite_access::SqliteConnection>>,
    failure: Mutex<Option<Error>>,
    sequence: u64,
    generation: Arc<()>,
    file: String,
    history: String,
    expected: [usize; 4],
    identity: std::fs::File,
    path: std::path::PathBuf,
}
impl OwnedScopeRead {
    fn check_identity(&self) -> Result<()> {
        use platform_io::fs_safety::linux::FileIdentity;
        if FileIdentity::from_fd(&self.identity)?
            != FileIdentity::from_fd(&crate::disk::open_regular(&self.path)?)?
        {
            return Err(Error::new(
                ErrorCode::UnsafePath,
                "scope snapshot index identity changed",
            ));
        }
        Ok(())
    }
    pub(crate) fn with_state<'state, T>(
        &self,
        state: &'state State,
        callback: impl FnOnce(&scope::ScopeReader<'state, '_>) -> Result<T>,
    ) -> Result<Option<T>> {
        let Some(mut c) = self
            .connection
            .lock()
            .expect("invariant: scope connection mutex poisoned")
            .take()
        else {
            return Ok(None);
        };
        let result = (|| {
            if state.sequence != self.sequence {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "scope snapshot State sequence mismatch",
                ));
            }
            self.check_identity()?;
            let result = scope::execute(
                state,
                &mut c,
                &self.file,
                &self.history,
                self.expected,
                callback,
            )?;
            self.check_identity()?;
            Ok(result)
        })();
        let closed =
            platform_io::sqlite::block_on_sqlite_connection(c.close()).map_err(Error::from);
        let result = match (result, closed) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), Ok(())) => Err(error),
            (Ok(Ok(_)), Err(close)) => Err(close),
            (Ok(Err(error)), Err(close)) | (Err(error), Err(close)) => {
                Err(preserve_close_failure(error, close))
            }
        };
        match result {
            Ok(value) => value.map(Some),
            Err(error) => {
                *self
                    .failure
                    .lock()
                    .expect("invariant: scope failure mutex poisoned") = Some(error);
                Ok(None)
            }
        }
    }
}
impl Drop for OwnedScopeRead {
    fn drop(&mut self) {
        if let Some(error) = self
            .failure
            .get_mut()
            .expect("invariant: scope failure mutex poisoned")
            .take()
        {
            tracing::warn!(%error, "history scope failure after read retired before recheck");
        }
        if let Some(c) = self
            .connection
            .get_mut()
            .expect("invariant: scope connection mutex poisoned")
            .take()
            && let Err(error) = platform_io::sqlite::block_on_sqlite_connection(c.close())
        {
            tracing::warn!(%error,"unused history scope snapshot shutdown failed");
        }
    }
}
impl Engine {
    pub(crate) fn capture_history_scope(
        &self,
        file: &str,
        history: &str,
    ) -> Result<Option<OwnedScopeRead>> {
        let mut proof = self
            .history_query_proof
            .lock()
            .expect("invariant: history query proof lock poisoned");
        let IndexAvailability::Ready(writer) = &proof.availability else {
            return Ok(None);
        };
        let result = (|| {
            checked_index_files(&self.root)?;
            let path = self.root.join("index.sqlite");
            writer.check_identity(&path)?;
            if proof.sequence != self.state.sequence {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "stale trusted scope proof",
                ));
            }
            let identity = writer.file.try_clone()?;
            let mut c = platform_io::sqlite::block_on_sqlite_connection(
                platform_io::sqlite::open_index_read_connection(&path),
            )
            .map_err(ProjectionFailure::from)
            .map_err(Error::from)?;
            let admitted = platform_io::sqlite::block_on_sqlite_connection(async {
                c.execute("BEGIN").await?;
                check_projection_marker(&mut c, self.state.sequence).await?;
                scope::check_schema(&mut c).await?;
                Ok::<_, ProjectionFailure>(())
            })
            .map_err(Error::from)
            .and_then(|()| writer.check_identity(&path));
            if let Err(error) = admitted {
                return match platform_io::sqlite::block_on_sqlite_connection(c.close()) {
                    Ok(()) => Err(error),
                    Err(close) => Err(preserve_close_failure(error, close.into())),
                };
            }
            Ok(OwnedScopeRead {
                connection: Mutex::new(Some(c)),
                failure: Mutex::new(None),
                sequence: self.state.sequence,
                generation: Arc::clone(&proof.read_generation),
                file: file.to_owned(),
                history: history.to_owned(),
                expected: proof
                    .scope_counts
                    .get(&(file.to_owned(), history.to_owned()))
                    .copied()
                    .unwrap_or_default(),
                identity,
                path,
            })
        })();
        match result {
            Ok(snapshot) => Ok(Some(snapshot)),
            Err(error) => {
                proof.disable(error, IndexFailureStage::Read, self.state.sequence);
                Ok(None)
            }
        }
    }
    pub(crate) fn record_scope_failure(&self, snapshot: &OwnedScopeRead) {
        let Some(error) = snapshot
            .failure
            .lock()
            .expect("invariant: scope failure mutex poisoned")
            .take()
        else {
            return;
        };
        let mut proof = self
            .history_query_proof
            .lock()
            .expect("invariant: history query proof lock poisoned");
        if !Arc::ptr_eq(&proof.read_generation, &snapshot.generation) {
            tracing::warn!(%error, "discarding failure from replaced history index generation");
            return;
        }
        if matches!(proof.availability, IndexAvailability::Ready(_)) {
            proof.disable(error, IndexFailureStage::Read, snapshot.sequence);
        }
    }
}
