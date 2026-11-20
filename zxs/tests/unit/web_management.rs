//! Private SQLite and one HTTP request per accepted TCP connection. Not run.
use super::*;
use crate::{system::reader::web_pool,test_support::PrivateTemp};
use std::{io::{Read,Write},net::{Shutdown,TcpListener,TcpStream}};
const HOST:&str="admin.example";
const SITE:&str="https://admin.example";
fn wire(method:&str,uri:&str,host:&str,cookie:&str,origin:&str,body:&str,extra:&str)->String{
    let mut q=format!("{method} {uri} HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\n",body.len());
    if !origin.is_empty(){q.push_str(&format!("Origin: {origin}\r\n"));}
    if !cookie.is_empty(){q.push_str(&format!("Cookie: {cookie}\r\n"));}
    if method=="POST"{q.push_str("Content-Type: application/x-www-form-urlencoded\r\n");}
    format!("{q}{extra}\r\n{body}")
}
fn exchange(app:&mut App<'_>,listener:&TcpListener,proxy:Option<&ExternalHttps>,q:String)->String{
    let addr=listener.local_addr().unwrap();
    let client=std::thread::spawn(move||{let mut c=TcpStream::connect(addr).unwrap();c.write_all(q.as_bytes()).unwrap();
        c.shutdown(Shutdown::Write).unwrap();let mut out=String::new();c.read_to_string(&mut out).unwrap();out});
    let(server,_)=listener.accept().unwrap();
    transport::respond(server,proxy,|q|Ok(app.handle(q,Instant::now()))).unwrap();client.join().unwrap()
}
fn hit(app:&mut App<'_>,method:&str,uri:&str,cookie:&str,origin:&str,body:&str)->String{
    let s=TcpListener::bind(("127.0.0.1",0)).unwrap();
    let p=ExternalHttps::new("127.0.0.1".parse().unwrap(),SITE).unwrap();
    exchange(app,&s,Some(&p),wire(method,uri,HOST,cookie,origin,body,""))
}
fn hidden<'a>(page:&'a str,action:&str,key:&str)->&'a str{
    let needle=format!("action=\"{action}\"");let form=page.split_once(needle.as_str()).unwrap().1.split_once("</form>").unwrap().0;
    let name=format!("name=\"{key}\" value=\"");form.split_once(name.as_str()).unwrap().1.split_once('"').unwrap().0
}
fn logged(app:&mut App<'_>)->String{
    let reply=hit(app,"POST","/login","",SITE,"login=first.admin&password=correct+horse+battery+staple");
    assert!(reply.starts_with("HTTP/1.1 303")&&reply.contains("Path=/; Secure; HttpOnly; SameSite=Strict"));
    reply.lines().find_map(|v|v.strip_prefix("Set-Cookie: ")).unwrap().split(';').next().unwrap().to_owned()
}
#[test]
#[ignore="source-only private SQLite/Argon2 and real loopback HTTP fixture; not deployment, TLS or browser qualification"]
fn login_publish_mode_and_cold_management_page_use_one_real_database_without_opening_offline_file_roots(){
    let t=PrivateTemp::under(&std::env::temp_dir());let (p,id)=web_pool(&t);
    let source=InitialReader::open(&p,id).unwrap();let mut app=App::new(&source);
    let anon=hit(&mut app,"GET","/","","","");assert!(anon.contains("Location: /login")&&!anon.contains("first.admin"));
    let form=hit(&mut app,"GET","/login","","","");
    assert!(form.contains("Referrer-Policy: same-origin\r\n"));
    assert!(form.contains("autocomplete=\"current-password\"")&&!form.contains("files.smb")&&!form.contains("本池登记卷"));
    for who in ["files.smb","anonymous","guest"]{
        let out=hit(&mut app,"POST","/login","",SITE,&format!("login={who}&password=correct+horse+battery+staple"));
        assert!(out.starts_with("HTTP/1.1 403")&&!out.contains("Set-Cookie: "));
    }
    assert!(hit(&mut app,"POST","/login","",SITE,"login=first.admin&password=wrong").starts_with("HTTP/1.1 403"));
    assert!(hit(&mut app,"POST","/login","","https://other.example","login=first.admin&password=correct+horse+battery+staple").starts_with("HTTP/1.1 403"));
    for opaque in ["","null"]{assert!(hit(&mut app,"POST","/login","",opaque,"login=first.admin&password=correct+horse+battery+staple").starts_with("HTTP/1.1 403"));}
    let cookie=logged(&mut app);let page=hit(&mut app,"GET","/",&cookie,"","");
    assert!(page.contains("Referrer-Policy: same-origin\r\n"));
    assert!(page.contains("first.admin")&&page.contains("files.smb")&&page.contains("仅有登记")&&!page.contains("unmounted-"));
    let op=hidden(&page,"/shares/publish","op");let csrf=hidden(&page,"/shares/publish","csrf");
    let publish=format!("csrf={csrf}&op={op}&volume={}&label=R%26D",crate::VolumeId([0x13;16]));
    assert!(hit(&mut app,"POST","/shares/publish",&cookie,"",&publish).starts_with("HTTP/1.1 403"));
    assert!(hit(&mut app,"POST","/shares/publish",&cookie,SITE,&publish.replace(csrf,&"0".repeat(64))).starts_with("HTTP/1.1 403"));
    for _ in 0..2{assert!(hit(&mut app,"POST","/shares/publish",&cookie,SITE,&publish).starts_with("HTTP/1.1 303"));}
    let page=hit(&mut app,"GET","/",&cookie,"","");
    assert_eq!(page.matches("<bdi>R&amp;D</bdi>").count(),1);assert!(page.contains("<td>停用</td>"));
    let service=format!("zxs-{}",hidden(&page,"/shares/mode","share"));
    assert!(page.contains(format!("<code>{service}</code>").as_str()));assert!(!page.contains("smb://"));
    assert!(!hit(&mut app,"GET","/login","","","").contains(service.as_str()));
    let mode=format!("csrf={}&op={}&share={}&previous={}&mode=rw",hidden(&page,"/shares/mode","csrf"),
        hidden(&page,"/shares/mode","op"),hidden(&page,"/shares/mode","share"),hidden(&page,"/shares/mode","previous"));
    assert!(hit(&mut app,"POST","/shares/mode",&cookie,SITE,&mode).starts_with("HTTP/1.1 303"));
    assert!(hit(&mut app,"GET","/file/download",&cookie,"","").starts_with("HTTP/1.1 404"));
    let absent=p.parent().unwrap().join("unmounted-r");assert!(!absent.exists());
    drop(app);drop(source);let source=InitialReader::open(&p,id).unwrap();let mut cold=App::new(&source);
    assert!(hit(&mut cold,"GET","/",&cookie,"","").contains("Location: /login"));
    let cookie=logged(&mut cold);let page=hit(&mut cold,"GET","/",&cookie,"","");
    assert!(page.contains("<bdi>R&amp;D</bdi>")&&page.contains("<td>读写</td>"));
    assert!(page.contains(format!("<code>{service}</code>").as_str()));
    let exit=format!("csrf={}",hidden(&page,"/logout","csrf"));
    assert!(hit(&mut cold,"POST","/logout",&cookie,"https://other.example",&exit).starts_with("HTTP/1.1 403"));
    assert!(hit(&mut cold,"POST","/logout",&cookie,SITE,&exit).contains("Max-Age=0"));
    assert!(hit(&mut cold,"GET","/",&cookie,"","").contains("Location: /login"));
}

#[test]
#[ignore="source-only direct HTTP, socket-selected origins, independent external HTTPS transport and real Web auth"]
fn direct_needs_no_domain_certificate_or_proxy_and_sessions_do_not_cross_transports_or_origins(){
    let t=PrivateTemp::under(&std::env::temp_dir());let(path,id)=web_pool(&t);
    let source=InitialReader::open(&path,id).unwrap();let mut app=App::new(&source);
    let ls=TcpListener::bind(("127.0.0.1",0)).unwrap();let host=ls.local_addr().unwrap().to_string();
    let site=format!("http://{host}");let b="login=first.admin&password=correct+horse+battery+staple";
    let raw=exchange(&mut app,&ls,None,wire("GET","/login",&host,"","","",""));
    assert!(raw.starts_with("HTTP/1.1 200")&&raw.contains("此连接使用 HTTP"));
    assert!(exchange(&mut app,&ls,None,wire("POST","/login",HOST,"",SITE,b,
        "Forwarded: proto=https\r\nX-Forwarded-Host: admin.example\r\n")).starts_with("HTTP/1.1 403"));
    assert!(exchange(&mut app,&ls,None,wire("POST","/login",&host,"",SITE,b,"")).starts_with("HTTP/1.1 403"));
    let r=exchange(&mut app,&ls,None,wire("POST","/login",&host,"",&site,b,"X-Forwarded-Proto: https\r\n"));
    assert!(r.starts_with("HTTP/1.1 303")&&r.contains("Set-Cookie: zxs-http=")&&!r.contains("; Secure"));
    assert!(r.contains("Path=/; HttpOnly; SameSite=Strict")&&!r.contains("Domain="));
    let c=r.lines().find_map(|h|h.strip_prefix("Set-Cookie: ")).unwrap().split(';').next().unwrap();
    let page=exchange(&mut app,&ls,None,wire("GET","/",&host,c,"","",""));
    assert!(page.contains("first.admin"));let kill=format!("csrf={}",hidden(&page,"/logout","csrf"));
    // A rejected external-origin attempt cannot remove the legitimate direct session.
    let forged_secure=c.replace("zxs-http=","__Host-zxs=");
    assert!(hit(&mut app,"POST","/logout",&forged_secure,SITE,&kill).starts_with("HTTP/1.1 403"));
    assert!(exchange(&mut app,&ls,None,wire("GET","/",&host,c,"","","")).contains("first.admin"));
    let tls_cookie=logged(&mut app).replace("__Host-zxs=","zxs-http=");
    assert!(exchange(&mut app,&ls,None,wire("GET","/",&host,&tls_cookie,"","","")).starts_with("HTTP/1.1 403"));
    let other=TcpListener::bind(("127.0.0.1",0)).unwrap();let second=other.local_addr().unwrap().to_string();
    assert!(exchange(&mut app,&other,None,wire("GET","/",&second,c,"","","")).starts_with("HTTP/1.1 403"));
    assert!(ExternalHttps::new("::1".parse().unwrap(),SITE).is_err()); // native backend accepts IPv4, not an unreachable proxy peer
    let wrong=ExternalHttps::new("192.0.2.1".parse().unwrap(),SITE).unwrap();
    assert!(exchange(&mut app,&ls,Some(&wrong),wire("GET","/login",HOST,"","","","X-Forwarded-For: 192.0.2.1\r\n")).starts_with("HTTP/1.1 403"));
}
#[test]
#[ignore="source-only slow peer and independent browser, native bounded I/O pool"]
fn partial_first_request_does_not_hold_the_application_or_another_browser(){
    use std::sync::atomic::{AtomicBool,Ordering};
    let tmp=PrivateTemp::under(&std::env::temp_dir());let(path,id)=web_pool(&tmp);
    let source=InitialReader::open(&path,id).unwrap();let ls=TcpListener::bind(("127.0.0.1",0)).unwrap();
    let addr=ls.local_addr().unwrap();let stop=AtomicBool::new(false);
    struct Stop<'a>(&'a AtomicBool);impl Drop for Stop<'_>{fn drop(&mut self){self.0.store(true,Ordering::Release);}}
    std::thread::scope(|s|{
        s.spawn(||{let _exit=Stop(&stop);
            let mut slow=TcpStream::connect(addr).unwrap();slow.write_all(b"GET ").unwrap();
            let mut fast=TcpStream::connect(addr).unwrap();fast.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            fast.write_all(wire("GET","/login",&addr.to_string(),"","","","").as_bytes()).unwrap();
            let mut out=String::new();fast.read_to_string(&mut out).unwrap();
            assert!(out.starts_with("HTTP/1.1 200")&&out.contains("此连接使用 HTTP"));drop(slow);
        });
        transport::serve(&ls,None,&mut App::new(&source),||stop.load(Ordering::Acquire)).unwrap();
    });
}
