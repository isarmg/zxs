use super::*;
#[test]
fn only_bounded_unambiguous_browser_messages_reach_authentication() {
    let base = "POST /api/disks HTTP/1.1\r\nHost: 127.0.0.1:9999\r\n";
    let make = |field: &str| format!("{base}{field}\r\n");
    let (s, n) = parse(make("Origin: http://127.0.0.1:9999\r\n").as_bytes()).unwrap();
    assert_eq!(s.path, "/api/disks");
    assert_eq!(n, 0);
    for bad in [
        "Host: elsewhere\r\n",
        "Content-Length: 64\r\nContent-Length: 0\r\n",
        "Content-Length: 65\r\n",
        "Transfer-Encoding: chunked\r\n",
        "Authorization: Bearer a\r\nAuthorization: Bearer b\r\n",
        "X-Forwarded-Host: 127.0.0.1:9999\r\n",
        "Origin: a\nOrigin: b\r\n",
    ] {
        assert!(parse(make(bad).as_bytes()).is_err());
    }
    assert!(parse(b"GET / HTTP/1.1\r\n\r\n").is_err());
}
