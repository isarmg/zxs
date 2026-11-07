//! Real layout jobs queued before a lost response or a published logout.
use super::*;
use std::{
    future::{Future, poll_fn},
    pin::Pin,
    sync::mpsc,
    task::Poll,
    time::Duration,
};

type WaitingResponse = Pin<Box<dyn Future<Output = (StatusCode, Value)> + Send>>;

struct BlockingGate {
    release: Option<mpsc::Sender<()>>,
    worker: Option<tokio::task::JoinHandle<()>>,
}

impl BlockingGate {
    async fn occupy() -> Self {
        let (release, released) = mpsc::channel();
        let (started, entered) = tokio::sync::oneshot::channel();
        let worker = tokio::task::spawn_blocking(move || {
            started.send(()).unwrap();
            // Dropping the sender also opens the gate when a test assertion
            // unwinds, so runtime shutdown cannot strand this fixture worker.
            let _release = released.recv();
        });
        tokio::time::timeout(Duration::from_secs(5), entered)
            .await
            .unwrap()
            .unwrap();
        Self {
            release: Some(release),
            worker: Some(worker),
        }
    }

    async fn open(mut self) {
        self.release.take().unwrap().send(()).unwrap();
        self.worker.take().unwrap().await.unwrap();
    }
}

async fn fixture(root: &Path) -> (Router, String, Value) {
    let app = configured_application(root, Config::development([root.join("d1")]).unwrap());
    let token = login(&app, "admin", "integration-test-password").await;
    manager_upload(&app, &token, "docs/a.txt", b"queued layout payload").await;
    manager_initialize_git(&app, &token, "docs").await;
    let listing = list(&app, &token).await;
    (app, token, listing)
}

async fn saturated(app: &Router, token: &str, listing: &Value) {
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        send(
            app,
            Some(token),
            "POST",
            &route(listing),
            Some(body(listing)),
        ),
    )
    .await
    .expect("layout admission must reject without waiting for a blocking worker");
    assert_eq!(response.0, StatusCode::TOO_MANY_REQUESTS, "{}", response.1);
    assert_eq!(response.1["error"]["code"], "RESOURCE_LIMIT_EXCEEDED");
    assert_eq!(
        response.1["error"]["message"],
        "HTTP backend capacity reached"
    );
}

async fn queued_requests(app: &Router, token: &str, listing: &Value) -> Vec<WaitingResponse> {
    let mut waiting = Vec::new();
    // The public HTTP backend pool admits eight jobs. A ninth real request
    // proves these futures reached job admission, rather than merely being
    // unpolled futures or requests blocked in the authentication middleware.
    for _ in 0..8 {
        let app = app.clone();
        let token = token.to_owned();
        let endpoint = route(listing);
        let request = body(listing);
        let mut response: WaitingResponse =
            Box::pin(
                async move { send(&app, Some(&token), "POST", &endpoint, Some(request)).await },
            );
        poll_fn(|context| {
            assert!(response.as_mut().poll(context).is_pending());
            Poll::Ready(())
        })
        .await;
        waiting.push(response);
    }
    saturated(app, token, listing).await;
    waiting
}

async fn cold_content(root: &Path, listing: &Value) -> Engine {
    let engine = reopen_after_router_drop(&root.join("system")).await;
    let version = listing["entries"][0]["version_id"].as_str().unwrap();
    let mut content = engine.open_content(version).unwrap();
    let mut bytes = Vec::new();
    content.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"queued layout payload");
    drop(content);
    engine
}

fn assert_moved_once(root: &Path, before: &BTreeMap<String, (PathBuf, u64)>) {
    assert_eq!(before.len(), 1);
    let after = containers(&root.join("d1"));
    assert_eq!(after.len(), before.len());
    for (id, (old, inode)) in before {
        let (path, actual) = &after[id];
        assert_ne!(path, old);
        assert_eq!(
            actual, inode,
            "a cancelled waiter must not duplicate its container"
        );
        assert!(path.to_str().unwrap().contains("/directory-repositories/"));
        assert!(!old.exists());
    }
}

#[test]
fn directory_layout_queued_cancellation_retains_capacity_auth_and_real_work() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let root = tempfile::tempdir().unwrap();
        let (app, token, listing) = fixture(root.path()).await;
        let before = containers(&root.path().join("d1"));
        assert_eq!(before.len(), 1);
        let gate = BlockingGate::occupy().await;
        let mut waiting = queued_requests(&app, &token, &listing).await;
        assert_eq!(containers(&root.path().join("d1")), before);
        drop(waiting.pop().unwrap());
        saturated(&app, &token, &listing).await;
        drop(waiting);
        saturated(&app, &token, &listing).await;
        // No response waiter or Router remains to own the credentials. The
        // already-enqueued jobs must retain both auth and admission themselves.
        drop(app);
        assert_eq!(
            AuthStore::open(&root.path().join("system"))
                .err()
                .unwrap()
                .code,
            storage_contract::ErrorCode::AuthStoreBusy,
            "a cancelled queued layout job must retain the real credential writer lock"
        );
        gate.open().await;
        let engine = cold_content(root.path(), &listing).await;
        let auth = AuthStore::open(&root.path().join("system")).unwrap();
        assert_eq!(auth.authenticate(&token).unwrap().principal, "admin");
        assert_moved_once(root.path(), &before);
        let repository =
            &engine.state().repositories[listing["context"]["repository_id"].as_str().unwrap()];
        assert_eq!(
            engine.state().branches[&repository.main_branch_id].head_commit_id,
            listing["context"]["base_head"]
        );
    });
}

#[test]
fn directory_layout_queued_before_logout_rechecks_auth_before_physical_work() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let root = tempfile::tempdir().unwrap();
        let (app, token, listing) = fixture(root.path()).await;
        let before = containers(&root.path().join("d1"));
        assert_eq!(before.len(), 1);
        let gate = BlockingGate::occupy().await;
        let waiting = queued_requests(&app, &token, &listing).await;
        // Use a second actual HTTP runtime to publish logout while the first
        // runtime's layout workers are queued, not a fake auth snapshot/hook.
        let (completed, response) = tokio::sync::oneshot::channel();
        let logout = std::thread::spawn({
            let app = app.clone();
            let token = token.clone();
            move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let result =
                    runtime.block_on(send(&app, Some(&token), "POST", "/api/auth/logout", None));
                completed.send(result).unwrap();
            }
        });
        let response = tokio::time::timeout(Duration::from_secs(10), response)
            .await
            .unwrap()
            .unwrap();
        logout.join().unwrap();
        assert_eq!(response.0, StatusCode::OK, "{}", response.1);
        assert_eq!(response.1["logged_out"], true);
        gate.open().await;
        for response in waiting {
            let response = tokio::time::timeout(Duration::from_secs(5), response)
                .await
                .unwrap();
            assert_eq!(response.0, StatusCode::UNAUTHORIZED, "{}", response.1);
            assert_eq!(response.1["error"]["code"], "UNAUTHORIZED");
        }
        assert_eq!(containers(&root.path().join("d1")), before);
        let fresh = login(&app, "admin", "integration-test-password").await;
        let result = success(&app, &fresh, "POST", &route(&listing), Some(body(&listing))).await;
        assert_eq!(result["containers"], "1");
        assert_moved_once(root.path(), &before);
        drop(app);
        drop(cold_content(root.path(), &listing).await);
    });
}
