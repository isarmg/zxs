use super::*;
use crate::headless_http::head;

#[test]
fn login_has_exact_path_body_extent_and_no_ambiguous_framing() {
    for text in [
        "POST /zxs/v1/sessions HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\n\r\n",
        "POST /zxs/v1/sessions HTTP/1.1\r\nHost: x\r\nContent-Length:\t0\r\n\r\n",
    ] {
        let head = head(text.as_bytes()).ok().unwrap();
        assert!(profile(&head) == BackendProfile::Login);
        assert!(login_frame(&head).is_ok());
    }
    for text in [
        "POST /zxs/v1/sessions HTTP/1.1\r\nHost: x\r\n\r\n",
        "POST /zxs/v1/sessions HTTP/1.1\r\nHost: x\r\nContent-Length: 1\r\n\r\n",
        "POST /zxs/v1/sessions HTTP/1.1\r\nHost: x\r\nContent-Length: 00\r\n\r\n",
        "POST /zxs/v1/sessions HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n",
    ] {
        let head = head(text.as_bytes()).ok().unwrap();
        assert!(matches!(login_frame(&head), Err(Problem::Malformed)));
    }
    let other = head(b"POST /zxs/v1/sessions?x=1 HTTP/1.1\r\nHost: x\r\n\r\n")
        .ok()
        .unwrap();
    assert!(profile(&other) == BackendProfile::Legacy);
}

#[test]
fn reuse_budget_cannot_occupy_all_workers_and_is_released_on_socket_drop() {
    let budget = ReuseBudget::default();
    let mut leases = Vec::new();
    for _ in 0..MAX_REUSED_CONNECTIONS {
        leases.push(budget.try_hold().unwrap());
    }
    assert!(budget.try_hold().is_none());
    drop(leases.pop());
    assert!(budget.try_hold().is_some());
}
