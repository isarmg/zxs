//! Real SQLite failure at the projection marker, after durable publication.
use sqlite_access::{Connection, Executor};
use std::path::Path;

pub fn fail_projection(path: &Path) -> String {
    platform_io::sqlite::block_on_sqlite_connection(async {
        let mut connection = platform_io::sqlite::open_index_connection(path, false, None)
            .await
            .unwrap();
        let sequence = sqlite_access::query_scalar("SELECT sequence FROM current_state")
            .fetch_one(&mut connection)
            .await
            .unwrap();
        connection
            .execute(
                "CREATE TRIGGER fail_publication_projection BEFORE INSERT ON current_state \
                 BEGIN SELECT RAISE(ABORT,'injected post-publication projection failure'); END;",
            )
            .await
            .unwrap();
        connection.close().await.unwrap();
        sequence
    })
}

pub fn projection_sequence(path: &Path) -> String {
    platform_io::sqlite::block_on_sqlite_connection(async {
        let mut connection = platform_io::sqlite::open_index_read_connection(path)
            .await
            .unwrap();
        let sequence = sqlite_access::query_scalar("SELECT sequence FROM current_state")
            .fetch_one(&mut connection)
            .await
            .unwrap();
        connection.close().await.unwrap();
        sequence
    })
}

pub fn receipt_outcome(receipt: &json_codec::Value) -> json_codec::Value {
    assert_eq!(
        storage_contract::client_protocol::operation_status(receipt),
        Some(storage_contract::client_protocol::OperationStatus::Published),
        "{receipt}"
    );
    let mut receipt = receipt.clone();
    // Diagnostic queries may carry a fresh observation; immediate mutation
    // responses contain only the same permanent publication outcome.
    receipt.as_object_mut().unwrap().remove("current_health");
    receipt
}
