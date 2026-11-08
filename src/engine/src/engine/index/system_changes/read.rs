use super::*;
use std::ops::Bound::{Excluded, Unbounded};

struct ExpectedPage {
    rows: Vec<((u64, String), Vec<u8>)>,
    following: Option<(u64, String)>,
}

fn expected_page(
    state: &State,
    order: &Order,
    generation: &Arc<()>,
    kind: Kind,
    cursor: Option<&SystemChangeCursor>,
    limit: usize,
) -> Result<ExpectedPage> {
    if !(1..=MAX_PAGE_ROWS).contains(&limit) {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "system change page size must be 1..=100",
        ));
    }
    let rows = order.rows(kind);
    if let Some(cursor) = cursor {
        if cursor.kind != kind
            || cursor.sequence != state.sequence
            || !Arc::ptr_eq(&cursor.generation, generation)
        {
            return Err(Error::new(
                ErrorCode::HistoryChanged,
                "system change cursor belongs to a different row family, State or projection instance",
            ));
        }
        if !rows.contains(&cursor.after) {
            return Err(Error::new(
                ErrorCode::InvalidCursor,
                "system change cursor is not a retained row",
            ));
        }
    }
    let lower = cursor.map_or(Unbounded, |cursor| Excluded(cursor.after.clone()));
    let mut page = ExpectedPage {
        rows: Vec::new(),
        following: None,
    };
    let mut bytes = 0usize;
    for key in rows.range((lower, Unbounded)).take(limit + 1) {
        if page.rows.len() == limit {
            page.following = Some(key.clone());
            break;
        }
        let payload = match kind {
            Kind::Catalog(kind) => kind.payload(state, &key.1)?,
            Kind::Audit(kind) => kind.payload(state, &key.1)?,
            Kind::Operation => {
                let id = key
                    .1
                    .parse::<OperationId>()
                    .expect("invariant: trusted operation order uses canonical IDs");
                encode(
                    state
                        .operations
                        .get(&id)
                        .expect("invariant: trusted order belongs to State"),
                )?
            }
            Kind::FilePath => encode(
                state
                    .file_path_nodes
                    .get(&key.1)
                    .expect("invariant: trusted order belongs to State"),
            )?,
        };
        if bytes + payload.len() > MAX_PAGE_BYTES {
            page.following = Some(key.clone());
            break;
        }
        bytes += payload.len();
        page.rows.push((key.clone(), payload));
    }
    Ok(page)
}

async fn read_page<T: DeserializeOwned>(
    connection: &mut sqlite_access::SqliteConnection,
    sequence: u64,
    generation: &Arc<()>,
    kind: Kind,
    cursor: Option<&SystemChangeCursor>,
    expected: &ExpectedPage,
) -> std::result::Result<SystemChangePage<T>, ProjectionFailure> {
    platform_io::sqlite::apply_connection_limits(
        connection,
        platform_io::sqlite::ConnectionLimits::new(MAX_PAGE_BYTES as u32)
            .with_max_sql_bytes(64 * 1024)
            .with_max_vm_operations(100_000)
            .with_max_bind_parameters(16),
    )
    .await
    .map_err(|source| ProjectionFailure {
        source: sqlite_access::Error::Io(std::io::Error::other(source)).into(),
        corrupt: false,
    })?;
    let mut tx = connection.begin().await?;
    check_projection_marker(&mut tx, sequence).await?;
    check_schema(&mut tx).await?;
    let limit = (expected.rows.len() + 1) as i64;
    let key_prefix = expected
        .rows
        .iter()
        .map(|(key, _)| key.1.len())
        .chain(expected.following.iter().map(|key| key.1.len()))
        .max()
        .unwrap_or(0) as i64
        + 1;
    // Read bounded keys and lengths before any payload allocation. The first
    // page has no synthetic empty-key cursor that could hide an anomalous row.
    let rows = if let Some(cursor) = cursor {
        sqlite_access::query("SELECT substr(id,1,?),length(CAST(id AS BLOB)),substr(sequence_key,1,9),length(sequence_key),length(CAST(payload AS BLOB)) FROM system_changes INDEXED BY system_change_order WHERE kind=? AND (sequence_key,id)>(?,?) ORDER BY sequence_key,id LIMIT ?")
            .bind(key_prefix).bind(kind.sql()).bind(cursor.after.0.to_be_bytes().as_slice()).bind(&cursor.after.1).bind(limit)
            .fetch_all(&mut *tx).await?
    } else {
        sqlite_access::query("SELECT substr(id,1,?),length(CAST(id AS BLOB)),substr(sequence_key,1,9),length(sequence_key),length(CAST(payload AS BLOB)) FROM system_changes INDEXED BY system_change_order WHERE kind=? ORDER BY sequence_key,id LIMIT ?")
            .bind(key_prefix).bind(kind.sql()).bind(limit).fetch_all(&mut *tx).await?
    };
    let keys: Vec<_> = expected
        .rows
        .iter()
        .map(|(key, _)| key)
        .chain(expected.following.iter())
        .collect();
    if rows.len() != keys.len() {
        return Err("system change page is incomplete or has extra rows".into());
    }
    let mut records = Vec::with_capacity(expected.rows.len());
    for (index, (row, key)) in rows.into_iter().zip(keys).enumerate() {
        let id: String = row.try_get(0)?;
        let id_bytes: i64 = row.try_get(1)?;
        let sequence_key: Vec<u8> = row.try_get(2)?;
        let sequence_bytes: i64 = row.try_get(3)?;
        if id != key.1
            || id_bytes != key.1.len() as i64
            || sequence_bytes != 8
            || sequence_key != key.0.to_be_bytes()
        {
            return Err("system change page key/order differs from admitted State".into());
        }
        let Some((_, expected_payload)) = expected.rows.get(index) else {
            continue;
        };
        let size: i64 = row.try_get(4)?;
        if size != expected_payload.len() as i64 {
            return Err("system change row length differs from admitted State".into());
        }
        let payload: Vec<u8> = sqlite_access::query_scalar(
            "SELECT substr(CAST(payload AS BLOB),1,?) FROM system_changes WHERE kind=? AND id=?",
        )
        .bind(size + 1)
        .bind(kind.sql())
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        if &payload != expected_payload {
            return Err("system change row differs from admitted State".into());
        }
        records.push(storage_contract::decode_strict(&payload).map_err(|source| {
            ProjectionFailure {
                source: source.into(),
                corrupt: false,
            }
        })?);
    }
    tx.commit().await?;
    let next = expected.following.as_ref().map(|_| SystemChangeCursor {
        sequence,
        kind,
        generation: Arc::clone(generation),
        after: expected
            .rows
            .last()
            .expect("invariant: a bounded encoded row fits an empty page")
            .0
            .clone(),
    });
    Ok(SystemChangePage {
        sequence,
        records,
        next,
    })
}

impl Engine {
    /// Read the complete stored receipt rows across all repositories. These are
    /// retained State results, not failed requests or live operation health.
    /// The caller owns external authorization. A cursor rejects any State change.
    pub fn system_operation_records(
        &self,
        cursor: Option<&SystemChangeCursor>,
        limit: usize,
    ) -> std::result::Result<SystemChangePage<OperationResult>, SystemChangeReadError> {
        self.system_change_page(Kind::Operation, cursor, limit)
    }

    /// Read retained explicit file path events, including previous/exterior
    /// paths, without filtering by current disk placement. This is not a feed
    /// of every content, directory or branch change.
    pub fn system_file_path_nodes(
        &self,
        cursor: Option<&SystemChangeCursor>,
        limit: usize,
    ) -> std::result::Result<SystemChangePage<FilePathNode>, SystemChangeReadError> {
        self.system_change_page(Kind::FilePath, cursor, limit)
    }

    pub(super) fn system_change_page<T: DeserializeOwned>(
        &self,
        kind: Kind,
        cursor: Option<&SystemChangeCursor>,
        limit: usize,
    ) -> std::result::Result<SystemChangePage<T>, SystemChangeReadError> {
        let mut proof = self
            .history_query_proof
            .lock()
            .expect("invariant: history query proof lock poisoned");
        if matches!(proof.availability, IndexAvailability::Ready(_))
            && proof.sequence != self.state.sequence
        {
            proof.disable(
                Error::new(
                    ErrorCode::IndexRebuildFailed,
                    "trusted system change order is stale",
                ),
                IndexFailureStage::Read,
                self.state.sequence,
            );
        }
        let writer = match &proof.availability {
            IndexAvailability::Ready(writer) => writer,
            IndexAvailability::Unavailable(failure) => {
                return Err(SystemChangeReadError::Unavailable(Arc::clone(failure)));
            }
            IndexAvailability::Unbuilt => {
                unreachable!("invariant: Engine admission builds the projection")
            }
        };
        let expected = expected_page(
            &self.state,
            &proof.system_order,
            &proof.read_generation,
            kind,
            cursor,
            limit,
        )?;
        let path = self.root.join("index.sqlite");
        let result = (|| -> Result<SystemChangePage<T>> {
            checked_index_files(&self.root)?;
            writer.check_identity(&path)?;
            let page = platform_io::sqlite::block_on_sqlite_connection(async {
                let mut connection = platform_io::sqlite::open_index_read_connection(&path)
                    .await
                    .map_err(ProjectionFailure::from)?;
                let result = read_page(
                    &mut connection,
                    self.state.sequence,
                    &proof.read_generation,
                    kind,
                    cursor,
                    &expected,
                )
                .await;
                let closed = connection.close().await.map_err(ProjectionFailure::from);
                match (result, closed) {
                    (Ok(page), Ok(())) => Ok(page),
                    (Err(error), closed) => Err(error.after_close(closed)),
                    (Ok(_), Err(error)) => Err(error),
                }
            })
            .map_err(Error::from)?;
            writer.check_identity(&path)?;
            Ok(page)
        })();
        match result {
            Ok(page) => Ok(page),
            Err(source) => {
                proof.disable(source, IndexFailureStage::Read, self.state.sequence);
                let IndexAvailability::Unavailable(failure) = &proof.availability else {
                    unreachable!("invariant: failed projection is disabled");
                };
                Err(SystemChangeReadError::Unavailable(Arc::clone(failure)))
            }
        }
    }
}
