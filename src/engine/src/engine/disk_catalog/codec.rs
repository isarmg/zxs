//! Bounded in-memory storage shared by the current and history disk catalogs.
use super::*;
use sqlite_access::{Row, SqliteConnection};

pub(super) struct OrdinalTable {
    first: &'static str,
    next: &'static str,
    body: &'static str,
    label: &'static str,
}

pub(super) const DIRECTORY_ROWS: OrdinalTable = OrdinalTable {
    first: "SELECT ordinal,length(CAST(body AS BLOB)) FROM backup_directories ORDER BY ordinal LIMIT 1",
    next: "SELECT ordinal,length(CAST(body AS BLOB)) FROM backup_directories WHERE ordinal>? ORDER BY ordinal LIMIT 1",
    body: "SELECT substr(CAST(body AS BLOB),1,?) FROM backup_directories WHERE ordinal=?",
    label: "disk directory",
};

pub(super) const HISTORY_ROWS: OrdinalTable = OrdinalTable {
    first: "SELECT ordinal,length(CAST(body AS BLOB)) FROM changes ORDER BY ordinal LIMIT 1",
    next: "SELECT ordinal,length(CAST(body AS BLOB)) FROM changes WHERE ordinal>? ORDER BY ordinal LIMIT 1",
    body: "SELECT substr(CAST(body AS BLOB),1,?) FROM changes WHERE ordinal=?",
    label: "disk history",
};

impl OrdinalTable {
    pub(super) async fn read<T: Serialize + for<'de> Deserialize<'de>>(
        &self,
        connection: &mut SqliteConnection,
        limit: usize,
        total: &mut usize,
        mut admit: impl FnMut(Option<&T>, &T) -> Result<()>,
    ) -> Result<Vec<T>> {
        let mut records = Vec::new();
        loop {
            // No empty/small sentinel: inspect even a malformed first key.
            let query = if records.is_empty() {
                sqlite_access::query(self.first)
            } else {
                sqlite_access::query(self.next).bind(records.len() as i64 - 1)
            };
            let Some(row) = query.fetch_optional(&mut *connection).await? else {
                break;
            };
            if records.len() == limit {
                return Err(budget());
            }
            if row.try_get::<i64, _>(0)? != records.len() as i64 {
                return Err(invalid(&format!(
                    "{} row ordinals are not contiguous",
                    self.label
                )));
            }
            let length = usize::try_from(row.try_get::<i64, _>(1)?)
                .map_err(|_| invalid(&format!("invalid {} row length", self.label)))?;
            if length > MAX_RECORD_BYTES - *total {
                return Err(budget());
            }
            let body: Vec<u8> = sqlite_access::query_scalar(self.body)
                .bind((length + 1) as i64)
                .bind(records.len() as i64)
                .fetch_one(&mut *connection)
                .await?;
            let value: T = storage_contract::decode_strict(&body)?;
            if body.len() != length
                || crate::disk::encode_metadata(&value, MAX_RECORD_BYTES)? != body
            {
                return Err(invalid(&format!(
                    "{} row encoding is not canonical",
                    self.label
                )));
            }
            admit(records.last(), &value)?;
            *total += length;
            records.push(value);
        }
        Ok(records)
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/disk_catalog_codec_tests.rs"]
mod tests;
