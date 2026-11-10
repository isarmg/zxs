//! Shared bounded record storage for file and directory SQLite materializations.
use crate::{Error, ErrorCode, Result};
use data_serialization::Serialize;
use sqlite_access::{Executor, Row, SqliteConnection};

pub(crate) const MAX_BYTES: usize = 16 * 1024 * 1024;
pub(super) const MAX_RECORD_BYTES: usize = 12 * 1024 * 1024;
const MAX_ROWS: usize = 65_536;

#[derive(Serialize)]
pub(super) struct Record {
    pub(super) kind: &'static str,
    pub(super) id: String,
    pub(super) body: String,
}

pub(super) struct Prepared {
    pub(super) bytes: Vec<u8>,
}

pub(super) struct Schema {
    pub(super) name: &'static str,
    pub(super) format: &'static str,
    pub(super) identity: &'static str,
    pub(super) records: &'static str,
    pub(super) kinds: &'static [&'static str],
}

impl Schema {
    fn invalid(&self, message: &str) -> Error {
        Error::new(
            ErrorCode::ContainerLayoutInvalid,
            format!("{} database {message}", self.name),
        )
    }

    pub(super) fn budget(&self) -> Error {
        Error::new(
            ErrorCode::ResourceLimit,
            format!(
                "{} database exceeds its bounded record or byte budget",
                self.name
            ),
        )
    }

    pub(super) fn records(&self) -> Records<'_> {
        Records {
            schema: self,
            rows: Vec::new(),
            bytes: 0,
        }
    }
    pub(super) fn encode(
        &self,
        records: Vec<Record>,
        owners: [&str; 2],
        sequence: u64,
        hash: &str,
    ) -> Result<Prepared> {
        let bytes = crate::sqlite_connection::materialize(
            MAX_BYTES,
            || self.budget(),
            async |connection| {
                connection.execute(self.identity).await?;
                connection.execute(self.records).await?;
                sqlite_access::query("INSERT INTO identity VALUES(1,?,?,?,?,?)")
                    .bind(self.format)
                    .bind(owners[0])
                    .bind(owners[1])
                    .bind(sequence.to_string())
                    .bind(hash)
                    .execute(&mut *connection)
                    .await?;
                for record in records {
                    sqlite_access::query("INSERT INTO records VALUES(?,?,?)")
                        .bind(record.kind)
                        .bind(record.id)
                        .bind(record.body)
                        .execute(&mut *connection)
                        .await?;
                }
                Ok(())
            },
        )?;
        Ok(Prepared { bytes })
    }

    pub(super) async fn verify_schema(&self, connection: &mut SqliteConnection) -> Result<()> {
        let expected = [
            ("identity", "table", "identity", Some(self.identity)),
            ("records", "table", "records", Some(self.records)),
            ("sqlite_autoindex_records_1", "index", "records", None),
        ];
        crate::sqlite_connection::require_schema(
            connection,
            &expected,
            &format!("{} database", self.name),
        )
        .await
    }
    pub(super) async fn read_records(
        &self,
        connection: &mut SqliteConnection,
        mut admit: impl FnMut(&Record) -> Result<()>,
    ) -> Result<Vec<Record>> {
        let mut records = Vec::<Record>::new();
        let mut bytes = 0;
        loop {
            // One bounded row at a time: a corrupt database must not multiply a
            // shared overflow payload across an unbounded fetch_all allocation.
            // No sentinel can precede every untrusted key: enumerate the first row
            // without a predicate, then continue from an actually observed key.
            let row = match records.last() {
            Some(last) => sqlite_access::query(
                "SELECT CAST(substr(CAST(kind AS BLOB),1,17) AS TEXT) AS kind,CAST(substr(CAST(id AS BLOB),1,257) AS TEXT) AS id,length(CAST(id AS BLOB)) AS id_bytes,length(CAST(body AS BLOB)) AS body_bytes FROM records WHERE (records.kind,records.id)>(?,?) ORDER BY records.kind,records.id LIMIT 1",
            )
            .bind(last.kind)
            .bind(&last.id)
            .fetch_optional(&mut *connection)
            .await?,
            None => {
                sqlite_access::query("SELECT CAST(substr(CAST(kind AS BLOB),1,17) AS TEXT) AS kind,CAST(substr(CAST(id AS BLOB),1,257) AS TEXT) AS id,length(CAST(id AS BLOB)) AS id_bytes,length(CAST(body AS BLOB)) AS body_bytes FROM records ORDER BY records.kind,records.id LIMIT 1")
                    .fetch_optional(&mut *connection)
                    .await?
            }
        };
            let Some(row) = row else {
                break;
            };
            if records.len() == MAX_ROWS {
                return Err(self.budget());
            }
            let kind: String = row.try_get("kind")?;
            let kind = self
                .kinds
                .iter()
                .copied()
                .find(|candidate| *candidate == kind)
                .ok_or_else(|| self.invalid("record kind is unsupported"))?;
            let id: String = row.try_get("id")?;
            if id.is_empty()
                || id.len() > 256
                || row.try_get::<i64, _>("id_bytes")? != id.len() as i64
            {
                return Err(self.invalid("record identity is not bounded exact text"));
            }
            let length = usize::try_from(row.try_get::<i64, _>("body_bytes")?)
                .map_err(|_| self.invalid("record length is invalid"))?;
            let remaining = MAX_RECORD_BYTES - bytes;
            if id.len() > remaining || length > remaining - id.len() {
                return Err(self.budget());
            }
            // Check the aggregate budget before copying an untrusted record body.
            let body: String = sqlite_access::query_scalar(
            "SELECT CAST(substr(CAST(body AS BLOB),1,?) AS TEXT) FROM records WHERE kind=? AND id=?",
        )
        .bind((length + 1) as i64)
        .bind(kind)
        .bind(&id)
        .fetch_one(&mut *connection)
        .await?;
            if body.len() != length {
                return Err(self.invalid("record length differs from its bytes"));
            }
            bytes += id.len() + length;
            let record = Record { kind, id, body };
            admit(&record)?;
            records.push(record);
        }
        Ok(records)
    }
}

pub(super) struct Records<'a> {
    schema: &'a Schema,
    rows: Vec<Record>,
    bytes: usize,
}

impl Records<'_> {
    pub(super) fn add<T: Serialize>(
        &mut self,
        kind: &'static str,
        id: &str,
        value: &T,
    ) -> Result<()> {
        if self.rows.len() == MAX_ROWS {
            return Err(self.schema.budget());
        }
        let encoded = crate::disk::encode_metadata(value, MAX_RECORD_BYTES - self.bytes)?;
        self.bytes += encoded.len() + id.len();
        if self.bytes > MAX_RECORD_BYTES {
            return Err(self.schema.budget());
        }
        self.rows.push(Record {
            kind,
            id: id.to_owned(),
            body: String::from_utf8(encoded).expect("invariant: metadata encoder emits UTF-8"),
        });
        Ok(())
    }

    pub(super) fn finish(mut self) -> Vec<Record> {
        self.rows
            .sort_by(|a, b| (a.kind, &a.id).cmp(&(b.kind, &b.id)));
        self.rows
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/container_storage/local_database_tests.rs"]
mod tests;
