use crate::logger::QueryLogger;
use crate::sql_str::SqlSafeStr;
use crate::sqlite::connection::{ConnectionHandle, ConnectionState};
use crate::sqlite::error::Error;
use crate::sqlite::statement::{StatementHandle, VirtualStatement};
use crate::sqlite::{SqliteArguments, SqliteError, SqliteQueryResult, SqliteRow};
use crate::Either;

enum ResetState {
    Pending,
    Complete,
}

pub struct ExecuteIter<'a> {
    handle: &'a mut ConnectionHandle,
    statement: &'a mut VirtualStatement,
    logger: QueryLogger,
    args: Option<SqliteArguments>,

    /// since a `VirtualStatement` can encompass multiple actual statements,
    /// this keeps track of the number of arguments so far
    args_used: usize,

    goto_next: bool,
    reset_state: ResetState,
}

pub(crate) fn iter(
    conn: &mut ConnectionState,
    query: impl SqlSafeStr,
    args: Option<SqliteArguments>,
    persistent: bool,
) -> Result<ExecuteIter<'_>, Error> {
    let query = query.into_sql_str();
    // fetch the cached statement or allocate a new one
    let statement = conn.statements.get(query.as_str(), persistent)?;

    let logger = QueryLogger::new(query, conn.log_settings.clone());

    Ok(ExecuteIter {
        handle: &mut conn.handle,
        statement,
        logger,
        args,
        args_used: 0,
        goto_next: true,
        reset_state: ResetState::Pending,
    })
}

fn bind(
    statement: &mut StatementHandle,
    arguments: &Option<SqliteArguments>,
    offset: usize,
) -> Result<usize, Error> {
    let mut n = 0;

    if let Some(arguments) = arguments {
        n = arguments.bind(statement, offset)?;
    }

    Ok(n)
}

impl ExecuteIter<'_> {
    fn reset(&mut self) -> Result<(), SqliteError> {
        self.reset_state = ResetState::Complete;
        self.statement.reset()
    }

    pub(crate) fn finish(mut self) -> Result<(), Error> {
        self.reset().map_err(Into::into)
    }

    fn reset_after_error(&mut self, error: &Error) {
        if let Err(reset_error) = self.reset() {
            // SQLite reset can repeat a step diagnostic; different cleanup failures remain visible.
            let repeated = error
                .as_database_error()
                .and_then(|error| error.try_downcast_ref::<SqliteError>())
                .is_some_and(|error| error.same_diagnostic(&reset_error));
            if !repeated {
                tracing::warn!(%reset_error, "error occurred while resetting the SQLite statement");
            }
        }
    }
}

impl Iterator for ExecuteIter<'_> {
    type Item = Result<Either<SqliteQueryResult, SqliteRow>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if matches!(self.reset_state, ResetState::Complete) {
            return None;
        }
        let statement = if self.goto_next {
            let statement = match self.statement.prepare_next(self.handle) {
                Ok(Some(statement)) => statement,
                Ok(None) => return self.reset().err().map(|error| Err(error.into())),
                Err(error) => {
                    self.reset_after_error(&error);
                    return Some(Err(error));
                }
            };

            self.goto_next = false;

            // sanity check: ensure the VM is reset and the bindings are cleared
            if let Err(error) = statement.handle.reset() {
                let error = error.into();
                self.reset_after_error(&error);
                return Some(Err(error));
            }

            statement.handle.clear_bindings();

            match bind(statement.handle, &self.args, self.args_used) {
                Ok(args_used) => self.args_used += args_used,
                Err(error) => {
                    self.reset_after_error(&error);
                    return Some(Err(error));
                }
            }

            statement
        } else {
            self.statement.current()?
        };

        match statement.handle.step() {
            Ok(true) => {
                self.logger.increment_rows_returned();

                Some(Ok(Either::Right(SqliteRow::current(
                    statement.handle,
                    statement.columns,
                    statement.column_names,
                ))))
            }
            Ok(false) => {
                let last_insert_rowid = self.handle.last_insert_rowid();

                let changes = statement.handle.changes();
                self.logger.increase_rows_affected(changes);

                let done = SqliteQueryResult {
                    changes,
                    last_insert_rowid,
                };

                self.goto_next = true;

                Some(Ok(Either::Left(done)))
            }
            Err(error) => {
                let error = error.into();
                self.reset_after_error(&error);
                Some(Err(error))
            }
        }
    }
}

impl Drop for ExecuteIter<'_> {
    fn drop(&mut self) {
        if matches!(self.reset_state, ResetState::Pending) {
            if let Err(error) = self.reset() {
                tracing::warn!(%error, "error occurred while resetting the SQLite statement");
            }
        }
    }
}
