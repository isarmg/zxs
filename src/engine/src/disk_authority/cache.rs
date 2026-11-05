//! A disposable system-disk SQLite projection. No cache value is accepted by
//! Disk::writer, verification, publication, identity admission, or recovery.
use super::*;
use data_serialization::Serialize;
use platform_io::{
    fs_safety::{
        AdvisoryLock, AtomicFile, EntryName, PayloadOpenMode, PrivateDirectory, RelativePath,
    },
    sqlite::block_on_sqlite_connection,
};
use sqlite_access::{
    Connection, Executor, Row, SqliteConnection,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteOwnedBuf},
};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    sync::atomic::{AtomicU64, Ordering},
};

const DATABASE: &str = "cache.db";
static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CachedCopy {
    pub observer_identity: String,
    pub owner_identity: String,
    pub origin: Origin,
    pub history_id: Token,
    pub file_id: Token,
    pub commit: String,
    pub body: String,
    pub availability: Availability,
}

pub struct PreparedCache {
    directory: PrivateDirectory,
    _lock: AdvisoryLock,
    staging: EntryName,
    file: File,
    bytes: Vec<u8>,
}
pub struct PublishedCache {
    directory: PrivateDirectory,
    _lock: AdvisoryLock,
}
impl PublishedCache {
    pub fn read(&self) -> Result<Vec<CachedCopy>> {
        read_held(&self.directory)
    }
}
impl PreparedCache {
    /// The replacement affects only cache.db. Data disks are not even held by
    /// this type, so a cache failure cannot authorize a disk mutation.
    pub fn publish(self) -> Result<PublishedCache> {
        self.directory.verify_identity()?;
        self.directory.verify_payload(&self.staging, &self.file)?;
        AtomicFile::replace(&self.directory, &RelativePath::new(DATABASE)?, &self.bytes)?;
        self.directory.remove_file(&self.staging)?;
        Ok(PublishedCache {
            directory: self.directory,
            _lock: self._lock,
        })
    }
}

pub fn prepare(directory: PrivateDirectory, disks: &[VerifiedDisk]) -> Result<PreparedCache> {
    validate_observations(disks)?;
    directory.verify_identity()?;
    let lock = AdvisoryLock::acquire(&directory, &RelativePath::new("cache.lock")?)?;
    let (staging, mut file) = staging_file(&directory)?;
    // SQLite builds only in memory. It cannot follow a changed target path or
    // create journals in another directory before a post-I/O identity check.
    let options = SqliteConnectOptions::new()
        .in_memory(true)
        .journal_mode(SqliteJournalMode::Memory);
    let bytes = block_on_sqlite_connection(async {
        let mut connection = SqliteConnection::connect_with(&options).await?;
        let result = async {
            connection.execute("PRAGMA page_size=4096").await?;
            connection.execute("PRAGMA max_page_count=4096").await?;
            build(&mut connection, disks).await?;
            let bytes = connection.serialize(None).await?;
            if bytes.len() > MAX_BYTES {
                return Err(Error::Budget);
            }
            Ok(bytes.to_vec())
        }
        .await;
        let closed = connection.close().await;
        finish_connection(result, closed)
    })?;
    directory.verify_payload(&staging, &file)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    directory.verify_payload(&staging, &file)?;
    directory.sync()?;
    Ok(PreparedCache {
        directory,
        _lock: lock,
        staging,
        file,
        bytes,
    })
}

fn validate_observations(disks: &[VerifiedDisk]) -> Result<()> {
    if disks.len() > MAX_PUBLICATIONS
        || disks.iter().map(|d| d.copies().len()).sum::<usize>() > MAX_OBJECTS
    {
        return Err(Error::Budget);
    }
    let mut observers = BTreeMap::new();
    let mut origins = BTreeMap::new();
    let mut decisions = BTreeMap::new();
    for disk in disks {
        let observer = disk.identity().digest()?;
        if observers.insert(observer, disk.checkpoint()).is_some() {
            return Err(Error::Invalid("duplicate or conflicting disk observation"));
        }
        if origins
            .insert(disk.identity().origin.clone(), observer)
            .is_some_and(|old| old != observer)
        {
            return Err(Error::Identity);
        }
        for (hash, publication) in &disk.publications {
            let identity = publication.identity.digest()?;
            if origins
                .insert(publication.identity.origin.clone(), identity)
                .is_some_and(|old| old != identity)
            {
                return Err(Error::Identity);
            }
            if decisions
                .insert((identity, publication.sequence), *hash)
                .is_some_and(|old| old != *hash)
            {
                return Err(Error::Invalid(
                    "conflicting source forks across observed disks",
                ));
            }
        }
    }
    Ok(())
}

async fn build(connection: &mut SqliteConnection, disks: &[VerifiedDisk]) -> Result<()> {
    let mut tx = connection.begin().await?;
    tx.execute("CREATE TABLE observations (identity TEXT PRIMARY KEY, checkpoint TEXT, sequence TEXT NOT NULL) STRICT").await?;
    tx.execute("CREATE TABLE copies (observer TEXT NOT NULL, owner TEXT NOT NULL, pool TEXT NOT NULL, disk TEXT NOT NULL, slot TEXT NOT NULL, binding TEXT NOT NULL, history TEXT NOT NULL, file TEXT NOT NULL, commit_hash TEXT NOT NULL, body_hash TEXT NOT NULL, available INTEGER NOT NULL CHECK(available IN (0,1)), PRIMARY KEY(observer,owner,commit_hash)) STRICT").await?;
    for disk in disks {
        let observer = disk.identity().digest()?.to_string();
        sqlite_access::query("INSERT INTO observations VALUES (?,?,?)")
            .bind(&observer)
            .bind(disk.checkpoint().map(|h| h.to_string()))
            .bind(disk.sequence().to_string())
            .execute(&mut *tx)
            .await?;
        for row in disk.copies() {
            let origin = &row.owner.origin;
            sqlite_access::query("INSERT INTO copies VALUES (?,?,?,?,?,?,?,?,?,?,?)")
                .bind(&observer)
                .bind(row.owner.digest()?.to_string())
                .bind(origin.pool_id.as_str())
                .bind(origin.disk_id.as_str())
                .bind(origin.slot_id.as_str())
                .bind(origin.binding_generation.to_string())
                .bind(row.history_id.as_str())
                .bind(row.file_id.as_str())
                .bind(row.commit.to_string())
                .bind(row.body.to_string())
                .bind(i64::from(row.availability == Availability::Available))
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
pub fn read(directory: &std::path::Path) -> Result<Vec<CachedCopy>> {
    read_held(&PrivateDirectory::open_existing(directory)?)
}

pub(super) fn read_held(directory: &PrivateDirectory) -> Result<Vec<CachedCopy>> {
    let name = EntryName::new(DATABASE)?;
    let mut file = directory.open_payload(&name, PayloadOpenMode::Existing)?;
    if file.metadata()?.len() > MAX_BYTES as u64 {
        return Err(Error::Budget);
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES {
        return Err(Error::Budget);
    }
    directory.verify_payload(&name, &file)?;
    // Deserialize the bounded held-FD snapshot, never a pathname (including
    // /proc aliases that SQLite may resolve back into mutable filesystem names).
    let database = SqliteOwnedBuf::try_from(bytes.as_slice())?;
    let options = SqliteConnectOptions::new().in_memory(true);
    let result = block_on_sqlite_connection(async {
        let mut connection = SqliteConnection::connect_with(&options).await?;
        let queried = async {
            connection.deserialize(None, database, true).await?;
            platform_io::sqlite::enable_defensive(&mut connection)
                .await
                .map_err(|error| Error::Io(std::io::Error::other(error)))?;
            platform_io::sqlite::apply_connection_limits(
                &mut connection,
                platform_io::sqlite::ConnectionLimits::new(4096)
                    .with_max_sql_bytes(4096)
                    .with_max_vm_operations(10_000)
                    .with_max_bind_parameters(16),
            )
            .await
            .map_err(|error| Error::Io(std::io::Error::other(error)))?;
            let kind: String =
                sqlite_access::query("SELECT type FROM sqlite_schema WHERE name='copies'")
                    .fetch_one(&mut connection)
                    .await?
                    .try_get("type")?;
            if kind != "table" {
                return Err(Error::Invalid("cached copies must be a real bounded table"));
            }
            let rows = sqlite_access::query(
                "SELECT * FROM copies ORDER BY observer,owner,commit_hash LIMIT ?",
            )
            .bind((MAX_OBJECTS + 1) as i64)
            .fetch_all(&mut connection)
            .await?;
            if rows.len() > MAX_OBJECTS {
                return Err(Error::Budget);
            }
            rows.into_iter()
                .map(|row| {
                    let binding: String = row.try_get("binding")?;
                    let binding_generation = binding
                        .parse()
                        .map_err(|_| Error::Invalid("invalid cached binding"))?;
                    if binding_generation == 0 {
                        return Err(Error::Invalid("invalid cached binding"));
                    }
                    let available: i64 = row.try_get("available")?;
                    let digest = |column: &str| -> Result<String> {
                        let text: String = row.try_get(column)?;
                        Ok(text.parse::<Digest>()?.to_string())
                    };
                    Ok(CachedCopy {
                        observer_identity: digest("observer")?,
                        owner_identity: digest("owner")?,
                        origin: Origin {
                            pool_id: Token::new(row.try_get::<String, _>("pool")?)?,
                            disk_id: Token::new(row.try_get::<String, _>("disk")?)?,
                            slot_id: Token::new(row.try_get::<String, _>("slot")?)?,
                            binding_generation,
                        },
                        history_id: Token::new(row.try_get::<String, _>("history")?)?,
                        file_id: Token::new(row.try_get::<String, _>("file")?)?,
                        commit: digest("commit_hash")?,
                        body: digest("body_hash")?,
                        availability: match available {
                            0 => Availability::TemporarilyUnavailable,
                            1 => Availability::Available,
                            _ => return Err(Error::Invalid("invalid cached availability")),
                        },
                    })
                })
                .collect::<Result<Vec<_>>>()
        }
        .await;
        let closed = connection.close().await;
        finish_connection(queried, closed)
    })?;
    directory.verify_payload(&name, &file)?;
    Ok(result)
}

fn staging_file(directory: &PrivateDirectory) -> Result<(EntryName, File)> {
    for _ in 0..MAX_PUBLICATIONS {
        let name = EntryName::new(format!(
            "cache-build-{}-{}.db",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ))?;
        match directory.open_payload(&name, PayloadOpenMode::CreateNew) {
            Ok(file) => return Ok((name, file)),
            Err(platform_io::fs_safety::Error::Io(error))
                if error.kind() == std::io::ErrorKind::AlreadyExists =>
            {
                continue;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Err(Error::Budget)
}

fn finish_connection<T>(
    operation: Result<T>,
    close: std::result::Result<(), sqlite_access::Error>,
) -> Result<T> {
    match (operation, close) {
        (Err(operation), Err(close)) => Err(Error::CacheOperationAndClose {
            operation: Box::new(operation),
            close,
        }),
        (Err(operation), Ok(())) => Err(operation),
        (Ok(_), Err(close)) => Err(close.into()),
        (Ok(value), Ok(())) => Ok(value),
    }
}
