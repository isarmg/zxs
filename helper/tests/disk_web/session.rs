use super::*;
fn req(path:&str)->Request{Request{method:"POST".into(),path:path.into(),host:"127.0.0.1:7654".into(),
    origin:Some("http://127.0.0.1:7654".into()),auth:None,csrf:None,content_type:None,body:Vec::new()}}
fn logged(now:Instant)->Panel{
    let mut p=Panel::new("127.0.0.1:7654".into(),"d".repeat(64).as_bytes(),now);
    p.pairing=None;p.session=Some(Session{bearer:digest("a".repeat(64).as_bytes()),csrf:digest("b".repeat(64).as_bytes())});
    p.expires=now+SESSION;p
}
fn approved(path:&str)->Request{let mut r=req(path);r.auth=Some(format!("Bearer {}","a".repeat(64)));r.csrf=Some("b".repeat(64));r}
#[test]
fn no_disk_observation_or_close_via_wrong_origin_missing_bearer_or_arbitrary_target(){
    let now=Instant::now();let mut p=logged(now);
    for (host,origin) in [("localhost:7654","http://127.0.0.1:7654"),("127.0.0.1:7654","http://127.0.0.1:7655"),
        ("127.0.0.1:7654","null"),("127.0.0.1:7654","https://example.invalid")]{
        let mut r=approved("/api/close");r.host=host.into();r.origin=Some(origin.into());
        assert_eq!(p.handle(r,now).code,403);assert!(!p.closed);
    }
    let mut bad=approved("/api/disks");bad.auth=None;assert_eq!(p.handle(bad,now).code,403);
    let mut bad=approved("/api/tools");bad.csrf=None;assert_eq!(p.handle(bad,now).code,403);
    let mut bad=approved("/api/rates");bad.body=b"/dev/example".to_vec();assert_eq!(p.handle(bad,now).code,400);
    assert_eq!(p.handle(approved("/api/tools?format=zfs"),now).code,404);
    assert_eq!(p.handle(approved("/api/run"),now).code,404);
    assert_eq!(p.handle(approved("/api/repair"),now).code,404);
    let ack=p.handle(approved("/api/close"),now);assert_eq!(ack.code,200);assert!(ack.body.contains("此回复不能作为升级许可"));assert!(p.closed);
}
#[test]
fn pairing_has_a_short_deadline_six_attempts_and_no_second_use(){
    let now=Instant::now();let mut p=Panel::new("127.0.0.1:7654".into(),"d".repeat(64).as_bytes(),now);
    let pair=|c:u8|{let mut r=req("/api/pair");r.content_type=Some("application/octet-stream".into());r.body=vec![c;64];r};
    assert_eq!(p.handle(pair(b'd'),now+PAIR).code,403);
    for _ in 0..5 {assert_eq!(p.handle(pair(b'e'),now).code,403);}
    assert_eq!(p.handle(pair(b'e'),now).code,429);assert!(p.closed);
    let mut active=logged(now);assert_eq!(active.handle(pair(b'd'),now).code,403);
    assert_eq!(active.handle(approved("/api/close"),now+SESSION).code,403);
}
#[test]
fn unknown_origins_cannot_obtain_credentials_and_site_assets_contain_no_private_report(){
    let now=Instant::now();let mut p=Panel::new("127.0.0.1:7654".into(),"d".repeat(64).as_bytes(),now);
    let mut r=req("/api/pair");r.body=vec![b'd';64];r.content_type=Some("application/octet-stream".into());
    r.origin=None;assert_eq!(p.handle(r,now).code,403);assert!(p.pairing.is_some()&&p.session.is_none());
    let mut r=req("/");r.method="GET".into();r.origin=None;
    let answer=p.handle(r,now);assert_eq!(answer.code,200);assert!(!answer.body.contains(&"d".repeat(64)));
    let mut r=req("/api/disks");r.method="GET".into();assert_eq!(p.handle(r,now).code,404);
}
#[test]
fn a_fresh_correct_pair_makes_one_session_and_only_both_new_secrets_can_close_it(){
    let now=Instant::now();let code="d".repeat(64);
    let mut p=Panel::new("127.0.0.1:7654".into(),code.as_bytes(),now);
    let mut r=req("/api/pair");r.content_type=Some("application/octet-stream".into());r.body=code.clone().into_bytes();
    let response=p.handle(r,now);assert_eq!(response.code,200);
    let (bearer,csrf)=response.body.split_once('\n').unwrap();
    assert_eq!((bearer.len(),csrf.len()),(64,64));assert_ne!(bearer,csrf);assert_ne!(bearer,code);assert!(p.pairing.is_none());
    let mut r=req("/api/close");r.auth=Some(format!("Bearer {bearer}"));r.csrf=Some(code);
    assert_eq!(p.handle(r,now).code,403);assert!(!p.closed);
    let mut r=req("/api/close");r.auth=Some(format!("Bearer {bearer}"));r.csrf=Some(csrf.into());
    assert_eq!(p.handle(r,now).code,200);assert!(p.closed);
}
#[test]
fn pairing_requires_a_supported_terminal_and_the_callers_actual_foreground_session(){
    use rustix::fs::makedev;
    assert!(supported_terminal(0o020620,makedev(136,4),0x1cd1));
    assert!(supported_terminal(0o020620,makedev(4,1),0));
    assert!(supported_terminal(0o020666,makedev(5,0),0));
    for (mode,dev,fs) in [(0o020666,makedev(5,2),0x1cd1),
        (0o100600,makedev(136,4),0x1cd1),(0o020620,makedev(136,4),0),(0o010600,0,0)] {
        assert!(!supported_terminal(mode,dev,fs));
    }
    assert!(foreground(31,31,47,47));
    assert!(!foreground(30,31,47,47));assert!(!foreground(31,31,42,47));
    assert!(!foreground(0,0,47,47));assert!(!foreground(31,31,0,0));
}

#[test]
fn pairing_or_a_user_supplied_format_path_or_flags_cannot_create_any_copy_grant(){
    let now=Instant::now();let mut p=logged(now);
    assert_eq!(p.handle(approved("/api/copy/preview"),now).code,403);
    let mut supplied=approved("/api/copy/preview");supplied.body=b"/dev/sda".to_vec();
    assert_eq!(p.handle(supplied,now).code,400);
    for endpoint in ["/api/copy/check","/api/copy/result"]{
        let mut r=approved(endpoint);r.content_type=Some("application/octet-stream".into());r.body=vec![b'f';64];
        assert_eq!(p.handle(r,now).code,403);
        let mut r=approved(endpoint);r.body=b"--fix".to_vec();assert_eq!(p.handle(r,now).code,400);
    }
}

#[test]
fn only_the_terminal_chooses_format_and_even_paired_clients_never_choose_a_tool(){
    let now=Instant::now();let mut p=logged(now);
    for path in ["/api/fat32/preview","/api/exfat/preview","/api/ext4/preview","/api/copy/preview?format=ext4","/api/copy/check?format=ext4","/api/exfat/repair"]{
        assert_eq!(p.handle(approved(path),now).code,404);
    }
    let mut q=approved("/api/copy/preview");q.content_type=Some("application/octet-stream".into());q.body=b"exFAT".to_vec();
    assert_eq!(p.handle(q,now).code,400);
    let submit=||{let mut q=approved("/api/copy/check");q.content_type=Some("application/octet-stream".into());q.body=vec![b'f';64];q};
    let mut q=submit();q.origin=Some("http://elsewhere.invalid".into());assert_eq!(p.handle(q,now).code,403);
    let mut q=submit();q.auth=None;assert_eq!(p.handle(q,now).code,403);
    let mut q=submit();q.csrf=None;assert_eq!(p.handle(q,now).code,403);
    assert!(p.check.is_none());
}

#[test]
fn the_install_guide_only_calls_the_fixed_probe_in_an_empty_approved_request(){
    use std::cell::Cell;
    let now=Instant::now();let mut panel=logged(now);let calls=Cell::new(0);
    let probe=||{calls.set(calls.get()+1);Reply::plain(200,"helper-install-guide-2\nequal\nunconfirmed\ndifferent")};

    let mut r=req("/api/install/guide");r.method="GET".into();r.origin=None;
    assert_eq!(panel.handle_with_guide(r,now,&probe).code,404);
    assert_eq!(panel.handle_with_guide(req("/api/install/guide"),now,&probe).code,403);
    let mut r=approved("/api/install/guide");r.csrf=None;
    assert_eq!(panel.handle_with_guide(r,now,&probe).code,403);
    let mut r=approved("/api/install/guide");r.origin=Some("https://elsewhere.invalid".into());
    assert_eq!(panel.handle_with_guide(r,now,&probe).code,403);
    let mut r=approved("/api/install/guide");r.body=b"pool path or approval".to_vec();
    assert_eq!(panel.handle_with_guide(r,now,&probe).code,400);
    let mut r=approved("/api/install/guide");r.content_type=Some("application/octet-stream".into());
    assert_eq!(panel.handle_with_guide(r,now,&probe).code,400);
    for path in ["/api/install/guide?system=/mnt/S","/api/install/pool","/api/install/confirm","/api/install/receipt","/api/install/start"]{
        assert_eq!(panel.handle_with_guide(approved(path),now,&probe).code,404);
    }
    assert_eq!(calls.get(),0);
    let report=panel.handle_with_guide(approved("/api/install/guide"),now,&probe);
    assert_eq!(report.code,200);assert_eq!(report.body,"helper-install-guide-2\nequal\nunconfirmed\ndifferent");
    assert_eq!(calls.get(),1);assert!(!panel.closed&&panel.check.is_none()&&panel.session.is_some());
    assert_eq!(panel.handle_with_guide(approved("/api/install/guide"),now+SESSION,&probe).code,403);
    assert_eq!(calls.get(),1);
}
