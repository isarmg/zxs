#![allow(clippy::rc_buffer)]

use std::sync::Arc;

use crate::column::ColumnIndex;
use crate::error::Error;
use crate::ext::ustr::UStr;
use crate::row::{debug_row, Row};
use crate::HashMap;

use crate::sqlite::statement::StatementHandle;
use crate::sqlite::{Sqlite, SqliteColumn, SqliteValue, SqliteValueRef};

/// Implementation of [`Row`] for SQLite.
///
/// Rows can move between threads, but their stateful values cannot be decoded
/// concurrently through shared references. See [`SqliteValue`] for details.
pub struct SqliteRow {
    pub(crate) values: Box<[SqliteValue]>,
    pub(crate) columns: Arc<Vec<SqliteColumn>>,
    pub(crate) column_names: Arc<HashMap<UStr, usize>>,
}

impl SqliteRow {
    pub(crate) fn current(
        statement: &StatementHandle,
        columns: &Arc<Vec<SqliteColumn>>,
        column_names: &Arc<HashMap<UStr, usize>>,
    ) -> Self {
        let size = statement.column_count();
        let mut values = Vec::with_capacity(size);

        for i in 0..size {
            values.push(unsafe {
                let raw = statement.column_value(i);

                SqliteValue::dup(raw, Some(columns[i].type_info.clone()))
            });
        }

        Self {
            values: values.into_boxed_slice(),
            columns: Arc::clone(columns),
            column_names: Arc::clone(column_names),
        }
    }
}

impl Row for SqliteRow {
    type Database = Sqlite;

    fn columns(&self) -> &[SqliteColumn] {
        &self.columns
    }

    fn try_get_raw<I>(&self, index: I) -> Result<SqliteValueRef<'_>, Error>
    where
        I: ColumnIndex<Self>,
    {
        let index = index.index(self)?;
        Ok(SqliteValueRef::value(&self.values[index]))
    }
}

impl ColumnIndex<SqliteRow> for &'_ str {
    fn index(&self, row: &SqliteRow) -> Result<usize, Error> {
        row.column_names
            .get(*self)
            .ok_or_else(|| Error::ColumnNotFound((*self).into()))
            .copied()
    }
}

impl std::fmt::Debug for SqliteRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        debug_row(self, f)
    }
}
