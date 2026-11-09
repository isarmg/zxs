//! The exact local directory row set shares the current catalog's total quota.
use super::*;

pub(super) async fn read(
    connection: &mut SqliteConnection,
    versions: usize,
    total: &mut usize,
) -> Result<Vec<DiskCatalogDirectory>> {
    let mut directories = Vec::<DiskCatalogDirectory>::new();
    loop {
        let row = if directories.is_empty() {
            sqlite_access::query("SELECT ordinal,length(CAST(body AS BLOB)) FROM backup_directories ORDER BY ordinal LIMIT 1")
                .fetch_optional(&mut *connection).await?
        } else {
            sqlite_access::query("SELECT ordinal,length(CAST(body AS BLOB)) FROM backup_directories WHERE ordinal>? ORDER BY ordinal LIMIT 1")
                .bind(directories.len() as i64 - 1).fetch_optional(&mut *connection).await?
        };
        let Some(row) = row else {
            break;
        };
        if versions + directories.len() == MAX_ROWS {
            return Err(budget());
        }
        if row.try_get::<i64, _>(0)? != directories.len() as i64 {
            return Err(invalid("disk directory ordinals are not contiguous"));
        }
        let length = usize::try_from(row.try_get::<i64, _>(1)?)
            .map_err(|_| invalid("invalid disk directory row length"))?;
        if length > MAX_RECORD_BYTES - *total {
            return Err(budget());
        }
        let body: Vec<u8> = sqlite_access::query_scalar(
            "SELECT substr(CAST(body AS BLOB),1,?) FROM backup_directories WHERE ordinal=?",
        )
        .bind((length + 1) as i64)
        .bind(directories.len() as i64)
        .fetch_one(&mut *connection)
        .await?;
        let value: DiskCatalogDirectory = storage_contract::decode_strict(&body)?;
        if body.len() != length || crate::disk::encode_metadata(&value, MAX_RECORD_BYTES)? != body {
            return Err(invalid("disk directory encoding is not canonical"));
        }
        value.validate()?;
        if directories
            .last()
            .is_some_and(|previous| previous.key() >= value.key())
        {
            return Err(invalid(
                "disk directories are not unique and in identity/path order",
            ));
        }
        *total += length;
        directories.push(value);
    }
    Ok(directories)
}
