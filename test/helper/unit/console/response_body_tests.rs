use super::*;
use http_routing::{Json, body::to_bytes};
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::Semaphore;

async fn same_response<T: Serialize + Clone>(value: T) {
    let expected = Json(value.clone()).into_response();
    let jobs = Arc::new(Semaphore::new(1));
    let permit = Arc::clone(&jobs).try_acquire_owned().unwrap();
    let actual = json(value, permit);
    assert_eq!(actual.status(), expected.status());
    assert_eq!(actual.headers(), expected.headers());
    assert_eq!(jobs.available_permits(), 0);
    let bytes = to_bytes(actual.into_body(), 1 << 20).await.unwrap();
    assert_eq!(
        bytes,
        to_bytes(expected.into_body(), 1 << 20).await.unwrap()
    );
    assert_eq!(jobs.available_permits(), 0);
    let queued = bytes.clone();
    drop(bytes);
    assert_eq!(jobs.available_permits(), 0);
    drop(queued);
    assert_eq!(jobs.available_permits(), 1);
}

#[tokio::test]
async fn json_success_and_encoding_failure_keep_bytes_headers_and_owned_frames() {
    same_response(json_codec::json!({"number":7,"text":"完整结果"})).await;
    same_response(BTreeMap::from([((1_u8, 2_u8), true)])).await;
}

#[tokio::test]
async fn dropping_an_unread_json_body_releases_the_slot() {
    let jobs = Arc::new(Semaphore::new(1));
    let permit = Arc::clone(&jobs).try_acquire_owned().unwrap();
    let response = json(json_codec::json!({"ready":true}), permit);
    assert_eq!(jobs.available_permits(), 0);
    drop(response);
    assert_eq!(jobs.available_permits(), 1);
}
