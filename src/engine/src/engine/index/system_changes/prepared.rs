//! Complete non-secret SYSTEM rows, not a page, self digest or row-count proof.
use super::*;

pub(in crate::engine::index) async fn verify_prepared(
    connection: &mut sqlite_access::SqliteConnection,
    state: &State,
    checkpoint: &impl Fn() -> Result<()>,
) -> Result<()> {
    let mut order = Order::default();
    order.update(None, state);
    let kinds = [Kind::Operation, Kind::FilePath]
        .into_iter()
        .chain(SystemCatalogKind::ALL.iter().copied().map(Kind::Catalog))
        .chain(
            [
                audit::Kind::Principal,
                audit::Kind::Event,
                audit::Kind::State,
            ]
            .into_iter()
            .map(Kind::Audit),
        );
    let mut expected = 0usize;
    for kind in kinds {
        for (sequence, id) in order.rows(kind) {
            checkpoint()?;
            let payload = match kind {
                Kind::Operation => {
                    let operation = id
                        .parse::<OperationId>()
                        .expect("invariant: source order contains canonical operation IDs");
                    encode(&state.operations[&operation])?
                }
                Kind::FilePath => encode(&state.file_path_nodes[id])?,
                Kind::Catalog(kind) => kind.payload(state, id)?,
                Kind::Audit(kind) => kind.payload(state, id)?,
            };
            verify_payload(connection, kind.sql(), id, *sequence, &payload).await?;
            expected += 1;
        }
    }
    let count: i64 = sqlite_access::query_scalar("SELECT count(*) FROM system_changes NOT INDEXED")
        .fetch_one(&mut *connection)
        .await?;
    // All expected keys originate from the bounded input State, not SQLite.
    if count != expected as i64 {
        return Err(invalid(
            "prepared SYSTEM change projection contains omitted or extra rows",
        ));
    }
    Ok(())
}

fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::ContainerLayoutInvalid, message)
}

async fn verify_payload(
    connection: &mut sqlite_access::SqliteConnection,
    kind: i64,
    id: &str,
    sequence: u64,
    expected: &[u8],
) -> Result<()> {
    // Read length first. A corrupt stored value cannot determine allocation.
    let row = sqlite_access::query(
        "SELECT substr(sequence_key,1,9),length(CAST(payload AS BLOB)) FROM system_changes WHERE kind=? AND id=?",
    )
    .bind(kind).bind(id).fetch_optional(&mut *connection).await?
    .ok_or_else(|| invalid("prepared SYSTEM change projection lacks an original row"))?;
    if row.try_get::<Vec<u8>, _>(0)? != sequence.to_be_bytes()
        || row.try_get::<i64, _>(1)? != expected.len() as i64
    {
        return Err(invalid(
            "prepared SYSTEM change row sequence or length differs",
        ));
    }
    let actual: Vec<u8> = sqlite_access::query_scalar(
        "SELECT substr(CAST(payload AS BLOB),1,?) FROM system_changes WHERE kind=? AND id=?",
    )
    .bind(expected.len() as i64 + 1)
    .bind(kind)
    .bind(id)
    .fetch_one(&mut *connection)
    .await?;
    if actual != expected {
        return Err(invalid(
            "prepared SYSTEM change row differs from its source State",
        ));
    }
    Ok(())
}
