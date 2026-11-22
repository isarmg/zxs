use super::*;
fn read(wire:&str)->io::Result<Request>{parse(wire.as_bytes(),"192.0.2.10:8443".parse().unwrap())}
const OK:&str="GET /login HTTP/1.1\r\nHost: 192.0.2.10:8443\r\n";
#[test]fn fixed_http_framing_and_separate_authority(){
    assert!(read(&format!("{OK}Accept: */*\r\n\r\n")).is_ok());
    let mut q=read("POST /login HTTP/1.1\r\nhOsT: 192.0.2.10:8443\r\nContent-Length: 17\r\n\r\nlogin=a&password=").unwrap();
    assert_eq!(fields(&mut q.body.0,["login","password"]),Some(["a",""]));assert_eq!(q.audience.origin,"http://192.0.2.10:8443");
    for header in ["Host: 192.0.2.10:8443","Content-Length: 0\r\nContent-Length: 0","Content-Length : 0",
        " folded","Transfer-Encoding: chunked","Upgrade: h2c","Expect: 100-continue","Content-Length: 4097","Origin: null\r\nOrigin: null",
        "Cookie: zxs-http=bad","Trailer: Origin","X-Test: value\nnext", "Content-Encoding: gzip"]{
        assert!(read(&format!("{OK}{header}\r\n\r\n")).is_err());}
    for w in ["GET / HTTP/1.0\r\nHost: 192.0.2.10:8443\r\n\r\n","POST /login HTTP/1.1\r\nHost: 192.0.2.10:8443\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: 0.0.0.0:8443\r\n\r\n","GET / HTTP/1.1\r\nHost: arbitrary.example\r\n\r\n"]{assert!(read(w).is_err());}
    assert!(read(&format!("{OK}{}\r\n","A: b\r\n".repeat(MAX_FIELDS))).is_err());
    assert!(read(&format!("{OK}A: {}\r\n\r\n","a".repeat(HEAD))).is_err());
    assert_eq!(read(&format!("{OK}\r\nGET /next HTTP/1.1\r\n\r\n")).unwrap().target,"/login");
    let mut a=b"password=%26%2B%25&login=manager".to_vec();assert_eq!(fields(&mut a,["login","password"]),Some(["manager","&+%"]));
    for b in [b"password=a&password=b".as_slice(),b"login=a&password=%GG",b"login=a&password=ok&extra=q"]{
        assert!(fields(&mut b.to_vec(),["login","password"]).is_none());}
}

#[test]
fn forwarded_values_do_not_establish_http_authority_or_cookie(){
    let local="192.0.2.10:8443".parse().unwrap();
    let forwarded="Forwarded: proto=https;host=admin.example\r\nX-Forwarded-Proto: https\r\nX-Forwarded-Host: admin.example\r\nZXS-Proxy-Key: obsolete\r\n";
    let get=|host:&str,extra:&str|format!("GET /login HTTP/1.1\r\nHost: {host}\r\n{forwarded}{extra}\r\n");
    for host in ["admin.example","127.0.0.1:8443","192.0.2.10","192.0.2.10:443"]{
        assert_eq!(parse(get(host,"").as_bytes(),local).err().unwrap().kind(),io::ErrorKind::PermissionDenied);
    }
    let raw=get("192.0.2.10:8443",&format!("Origin: https://admin.example\r\nCookie: __Host-zxs={}\r\n","a5".repeat(32)));
    let request=parse(raw.as_bytes(),local).unwrap();
    assert_eq!(request.audience.origin,"http://192.0.2.10:8443");
    assert_eq!(request.origin.as_deref(),Some("https://admin.example"));
    assert!(!request.same_origin_if_present()&&request.cookie.is_none());
    assert_eq!(session_cookie("fixture",false),"zxs-http=fixture; Path=/; HttpOnly; SameSite=Strict");
}

#[test]fn queued_call_is_not_admitted_on_stop_or_a_fault_still_in_the_channel(){
    let closed=AtomicBool::new(false);let stop=AtomicBool::new(false);
    let(tx,rx)=mpsc::sync_channel(2);let(out,back)=mpsc::sync_channel(1);
    assert!(tx.send(Job::Call(read(&format!("{OK}\r\n")).unwrap(),out)).is_ok());
    worker_fault(&closed,&tx,io::Error::other("listener fault"));
    let Job::Call(_,reply)=rx.recv().unwrap()else{panic!("queued call missing")};
    assert!(reply.send(admission(&closed,&||false,||panic!("a queued Call entered the application"))).is_ok());
    assert_eq!(back.recv().unwrap().status,"503 Service Unavailable");assert!(matches!(rx.recv(),Ok(Job::Fault(_))));
    let closed=AtomicBool::new(false);
    // Already admitted work may finish; the next call must not start.
    assert_eq!(admission(&closed,&||stop.load(Ordering::Acquire),||{
        stop.store(true,Ordering::Release);Reply::text("200 OK","finished")
    }).status,"200 OK");
    assert_eq!(admission(&closed,&||stop.load(Ordering::Acquire),||panic!("ran after stop")).status,"503 Service Unavailable");
}
