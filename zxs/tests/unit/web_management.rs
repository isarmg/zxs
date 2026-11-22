//! Private SQLite and one HTTP request per accepted TCP connection. Not run.
use super::*;
use crate::{system::reader::web_pool,test_support::PrivateTemp};
use std::{io::{Read,Write},net::{Shutdown,TcpListener,TcpStream}};
const DIRECT_WARNING:&str="HTTP 未加密";
fn wire(method:&str,uri:&str,host:&str,cookie:&str,origin:&str,body:&str,extra:&str)->String{
    let mut q=format!("{method} {uri} HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\n",body.len());
    if !origin.is_empty(){q.push_str(&format!("Origin: {origin}\r\n"));}
    if !cookie.is_empty(){q.push_str(&format!("Cookie: {cookie}\r\n"));}
    if method=="POST"{q.push_str("Content-Type: application/x-www-form-urlencoded\r\n");}
    format!("{q}{extra}\r\n{body}")
}
fn exchange(app:&mut App<'_>,listener:&TcpListener,q:String)->String{
    let addr=listener.local_addr().unwrap();
    let client=std::thread::spawn(move||{let mut c=TcpStream::connect(addr).unwrap();c.write_all(q.as_bytes()).unwrap();
        c.shutdown(Shutdown::Write).unwrap();let mut out=String::new();c.read_to_string(&mut out).unwrap();out});
    let(server,_)=listener.accept().unwrap();
    transport::respond(server,|q|Ok(app.handle(q,Instant::now()))).unwrap();client.join().unwrap()
}
fn hit(app:&mut App<'_>,listener:&TcpListener,method:&str,uri:&str,cookie:&str,origin:&str,body:&str)->String{
    exchange(app,listener,wire(method,uri,&listener.local_addr().unwrap().to_string(),cookie,origin,body,""))
}
fn hidden<'a>(page:&'a str,action:&str,key:&str)->&'a str{
    let needle=format!("action=\"{action}\"");let form=page.split_once(needle.as_str()).unwrap().1.split_once("</form>").unwrap().0;
    let name=format!("name=\"{key}\" value=\"");form.split_once(name.as_str()).unwrap().1.split_once('"').unwrap().0
}
fn logged(app:&mut App<'_>,listener:&TcpListener)->String{
    let site=format!("http://{}",listener.local_addr().unwrap());
    let reply=hit(app,listener,"POST","/login","",&site,"login=first.admin&password=correct+horse+battery+staple");
    assert!(reply.starts_with("HTTP/1.1 303")&&reply.contains("Path=/; HttpOnly; SameSite=Strict"));
    assert!(!reply.contains("; Secure")&&!reply.contains("Domain="));
    reply.lines().find_map(|v|v.strip_prefix("Set-Cookie: ")).unwrap().split(';').next().unwrap().to_owned()
}
#[test]
#[ignore="source-only private SQLite/Argon2 and real loopback HTTP fixture; not deployment, TLS or browser qualification"]
fn login_change_installed_folder_and_cold_overview_do_not_open_offline_file_roots(){
    let t=PrivateTemp::under(&std::env::temp_dir());let (p,id)=web_pool(&t);
    let source=InitialReader::open(&p,id).unwrap();
    // Seed the actual immutable kind3/4 rows in the first Web epoch, as
    // the separately reviewed native installer does in its first txn.
    let setup=source.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let v=crate::VolumeId([0x13;16]);let installed=setup.publish_root(OperationId([0xa4;16]),v,"R&D").unwrap();
    setup.set_share_mode(OperationId([0xa5;16]),installed.share,installed.op,
        crate::system::reader::account::share_mode::Mode::ReadOnly).unwrap();
    let outside=setup.publish_root(OperationId([0xa6;16]),v,"not selected").unwrap();
    drop(setup);let mut app=App::new(&source);
    let listener=TcpListener::bind(("127.0.0.1",0)).unwrap();
    let site=format!("http://{}",listener.local_addr().unwrap());
    let anon=hit(&mut app,&listener,"GET","/","","","");assert!(anon.contains("Location: /login")&&!anon.contains("first.admin"));
    let form=hit(&mut app,&listener,"GET","/login","","","");
    assert!(form.contains("Referrer-Policy: same-origin\r\n"));
    assert!(form.contains("autocomplete=\"current-password\"")&&!form.contains("files.smb")&&!form.contains("本池登记卷")&&form.contains(DIRECT_WARNING));
    for who in ["files.smb","anonymous","guest"]{
        let out=hit(&mut app,&listener,"POST","/login","",&site,&format!("login={who}&password=correct+horse+battery+staple"));
        assert!(out.starts_with("HTTP/1.1 403")&&!out.contains("Set-Cookie: "));
    }
    assert!(hit(&mut app,&listener,"POST","/login","",&site,"login=first.admin&password=wrong").starts_with("HTTP/1.1 403"));
    assert!(hit(&mut app,&listener,"POST","/login","","https://other.example","login=first.admin&password=correct+horse+battery+staple").starts_with("HTTP/1.1 403"));
    for opaque in ["","null"]{assert!(hit(&mut app,&listener,"POST","/login","",opaque,"login=first.admin&password=correct+horse+battery+staple").starts_with("HTTP/1.1 403"));}
    let cookie=logged(&mut app,&listener);let page=hit(&mut app,&listener,"GET","/",&cookie,"","");
    assert!(page.contains("Referrer-Policy: same-origin\r\n"));
    assert!(page.matches("first.admin").count()==1&&page.contains("Web 与 SMB 共用账号")&&!page.contains("files.smb")&&!page.contains("unmounted-"));
    assert!(!page.contains("本池登记卷")&&!page.contains("总容量")&&!page.contains("文件系统 UUID")&&
        !page.contains("Web 所见挂载")&&!page.contains("检查此卷")&&!page.contains("zxs-status"));
    // Business modes still display the original logical volume, not a disk probe.
    assert!(page.contains(&format!("{v}</code>")));
    let removed="/volumes/observe";
    assert!(hit(&mut app,&listener,"GET",removed,&cookie,"","").starts_with("HTTP/1.1 404"));
    let old=format!("csrf={}&volume={v}",hidden(&page,"/shares/mode","csrf"));
    assert!(hit(&mut app,&listener,"POST",removed,&cookie,&site,&old).starts_with("HTTP/1.1 404"));
    assert_eq!(page.matches(">R&amp;D</bdi>").count(),1);
    assert!(page.contains("只读</span>")&&page.contains(r#"<option value="ro" selected>只读</option>"#));
    assert!(page.contains("SMB 共享名")&&page.contains("<code>ZXS</code>")&&!page.contains("not selected"));
    assert!(!page.contains("smb://")&&!page.contains("action=\"/shares/publish\""));
    assert!(hit(&mut app,&listener,"POST","/shares/publish",&cookie,&site,"").starts_with("HTTP/1.1 404"));
    assert!(!hit(&mut app,&listener,"GET","/login","","","").contains("R&amp;D"));
    let mode=format!("csrf={}&op={}&share={}&previous={}&mode=rw",hidden(&page,"/shares/mode","csrf"),
        hidden(&page,"/shares/mode","op"),hidden(&page,"/shares/mode","share"),hidden(&page,"/shares/mode","previous"));
    let forged=format!("csrf={}&op={}&share={}&previous={}&mode=rw",hidden(&page,"/shares/mode","csrf"),
        OperationId([0xa9;16]),outside.share,outside.op);
    assert!(hit(&mut app,&listener,"POST","/shares/mode",&cookie,&site,&forged).starts_with("HTTP/1.1 409"));
    assert!(hit(&mut app,&listener,"POST","/shares/mode",&cookie,"",&mode).starts_with("HTTP/1.1 403"));
    let csrf=hidden(&page,"/shares/mode","csrf");
    assert!(hit(&mut app,&listener,"POST","/shares/mode",&cookie,&site,&mode.replace(csrf,&"0".repeat(64))).starts_with("HTTP/1.1 403"));
    for _ in 0..2{assert!(hit(&mut app,&listener,"POST","/shares/mode",&cookie,&site,&mode).starts_with("HTTP/1.1 303"));}
    assert!(hit(&mut app,&listener,"GET","/file/download",&cookie,"","").starts_with("HTTP/1.1 404"));
    let absent=p.parent().unwrap().join("unmounted-r");assert!(!absent.exists());
    drop(app);drop(source);let source=InitialReader::open(&p,id).unwrap();let mut cold=App::new(&source);
    assert!(hit(&mut cold,&listener,"GET","/",&cookie,"","").contains("Location: /login"));
    let cookie=logged(&mut cold,&listener);let page=hit(&mut cold,&listener,"GET","/",&cookie,"","");
    assert!(page.contains(">R&amp;D</bdi>")&&page.contains("读写</span>")&&page.contains(r#"<option value="rw" selected>读写</option>"#));
    assert!(page.contains("SMB 共享名")&&page.contains("<code>ZXS</code>"));
    let exit=format!("csrf={}",hidden(&page,"/logout","csrf"));
    assert!(hit(&mut cold,&listener,"POST","/logout",&cookie,"https://other.example",&exit).starts_with("HTTP/1.1 403"));
    assert!(hit(&mut cold,&listener,"POST","/logout",&cookie,&site,&exit).contains("Max-Age=0"));
    assert!(hit(&mut cold,&listener,"GET","/",&cookie,"","").contains("Location: /login"));
}

#[test]
#[ignore="source-only direct HTTP, socket-selected origins and real Web auth"]
fn only_the_actual_http_origin_may_use_the_session(){
    let t=PrivateTemp::under(&std::env::temp_dir());let(path,id)=web_pool(&t);
    let source=InitialReader::open(&path,id).unwrap();let mut app=App::new(&source);
    let ls=TcpListener::bind(("127.0.0.1",0)).unwrap();let host=ls.local_addr().unwrap().to_string();
    let site=format!("http://{host}");let b="login=first.admin&password=correct+horse+battery+staple";
    let forwarded="Forwarded: proto=https;host=admin.example\r\nX-Forwarded-Proto: https\r\nX-Forwarded-Host: admin.example\r\n";
    let raw=exchange(&mut app,&ls,wire("GET","/login",&host,"","","",forwarded));
    assert!(raw.starts_with("HTTP/1.1 200")&&raw.contains(DIRECT_WARNING));
    let assumed_https=format!("https://{host}");
    for (shown_host,origin) in [("admin.example","https://admin.example"),
        (host.as_str(),"https://admin.example"),(host.as_str(),assumed_https.as_str())]{
        let r=exchange(&mut app,&ls,wire("POST","/login",shown_host,"",origin,b,forwarded));
        assert!(r.starts_with("HTTP/1.1 403")&&!r.contains("Set-Cookie: "));
    }
    let r=exchange(&mut app,&ls,wire("POST","/login",&host,"",&site,b,forwarded));
    assert!(r.starts_with("HTTP/1.1 303")&&r.contains("Set-Cookie: zxs-http=")&&!r.contains("; Secure"));
    assert!(r.contains("Path=/; HttpOnly; SameSite=Strict")&&!r.contains("Domain="));
    let c=r.lines().find_map(|h|h.strip_prefix("Set-Cookie: ")).unwrap().split(';').next().unwrap();
    let page=hit(&mut app,&ls,"GET","/",c,"","");assert!(page.contains("first.admin"));
    let kill=format!("csrf={}",hidden(&page,"/logout","csrf"));
    assert!(hit(&mut app,&ls,"POST","/logout",c,&assumed_https,&kill).starts_with("HTTP/1.1 403"));
    let other=TcpListener::bind(("127.0.0.1",0)).unwrap();
    assert!(hit(&mut app,&other,"GET","/",c,"","").starts_with("HTTP/1.1 403"));
    assert!(hit(&mut app,&ls,"GET","/",c,"","").contains("first.admin"));
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
            assert!(out.starts_with("HTTP/1.1 200")&&out.contains(DIRECT_WARNING));drop(slow);
        });
        transport::serve(&ls,&mut App::new(&source),||stop.load(Ordering::Acquire)).unwrap();
    });
}

#[test]
#[ignore="isolated real HTTP, SQLite and file OFDs, not deployment; source only"]
fn only_the_displayed_manual_form_can_publish_one_formal_version(){
    use crate::system::reader::{ordinary_pool,stage_managed_file};
    use std::fs::{File,OpenOptions};
    use std::os::unix::fs::OpenOptionsExt;
    let temp=PrivateTemp::under(&std::env::temp_dir());
    let(path,id,_,rw,ro)=ordinary_pool(&temp);
    let source=InitialReader::open(&path,id).unwrap();
    let mut data:File=OpenOptions::new().write(true).read(true).create_new(true).mode(0o600)
        .open(temp.0.join("packed")).unwrap();
    data.write_all(b"work").unwrap();data.sync_all().unwrap();
    let file=stage_managed_file(&source,rw,ro,"Web').xlsx",&data).unwrap();
    let issuer=InitialReader::open(&path,id).unwrap();
    let admin=issuer.authenticate_web("first.admin","correct horse battery staple").unwrap();
    let target=admin.manual_target(file).unwrap();
    use std::sync::atomic::{AtomicBool,Ordering};
    let ls=TcpListener::bind(("127.0.0.1",0)).unwrap();let addr=ls.local_addr().unwrap();
    let stop=AtomicBool::new(false);
    struct Stop<'a>(&'a AtomicBool);impl Drop for Stop<'_>{fn drop(&mut self){self.0.store(true,Ordering::Release);}}
    let (old_op,new_op,result,history_result)=std::thread::scope(|scope|{
        let client=scope.spawn(||{
            let _end=Stop(&stop);
            let host=addr.to_string();let origin=format!("http://{host}");
            let req=|method:&str,target:&str,c:&str,o:&str,body:&str|->String{
                let mut io=TcpStream::connect(addr).unwrap();
                io.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
                io.write_all(wire(method,target,&host,c,o,body,"").as_bytes()).unwrap();
                io.shutdown(Shutdown::Write).unwrap();let mut result=String::new();io.read_to_string(&mut result).unwrap();result
            };
            let login=req("POST","/login","",&origin,"login=first.admin&password=correct+horse+battery+staple");
            let c=login.lines().find_map(|v|v.strip_prefix("Set-Cookie: ")).unwrap().split(';').next().unwrap();
            let view=format!("/versions?file={file}");
            let history=format!("/versions/history?file={file}");
            assert!(req("GET",&history,"","","").starts_with("HTTP/1.1 303"));
            assert!(req("GET",&format!("{history}&before=01"),c,"","").starts_with("HTTP/1.1 400"));
            assert!(req("GET","/versions","","","").starts_with("HTTP/1.1 303"));
            let entries=req("GET","/versions",c,"","");
            assert!(entries.contains(format!("href=\"{view}\"").as_str())&&entries.contains("Web&#39;).xlsx"));
            assert!(entries.contains(format!("href=\"{history}\"").as_str()));
            let page=req("GET",&view,c,"","");
            assert!(page.contains("Web&#39;).xlsx")&&page.contains("数据卷"));
            let lookup="/versions/receipt";
            assert!(page.contains("formaction=\"/versions/receipt\"")&&page.contains("只查本次回执"));
            let previous=format!("csrf={}&op={}",hidden(&page,"/versions/commit","csrf"),hidden(&page,"/versions/commit","op"));
            let other=req("GET",&view,c,"","");
            let old_op=hidden(&page,"/versions/commit","op").parse::<crate::OperationId>().unwrap();
            let new_op=hidden(&other,"/versions/commit","op").parse::<crate::OperationId>().unwrap();
            assert_ne!(old_op,new_op);
            let good=format!("csrf={}&op={}",hidden(&other,"/versions/commit","csrf"),new_op);
            let stale=req("POST","/versions/commit",c,&origin,&previous);
            assert!(stale.starts_with("HTTP/1.1 409")&&stale.contains("这次没有提交，也未核对上一次的结果"));
            assert!(stale.contains("不要用新表单重试上一次提交"));
            assert!(req("POST","/versions/commit",c,"http://other.example",&good).starts_with("HTTP/1.1 403"));
            let bad=format!("csrf={}&op={}", "b".repeat(64),new_op);
            assert!(req("POST","/versions/commit",c,&origin,&bad).starts_with("HTTP/1.1 403"));
            assert!(req("GET","/versions/commit",c,"","").starts_with("HTTP/1.1 404"));
            assert!(req("GET",lookup,c,"","").starts_with("HTTP/1.1 404"));
            assert!(req("POST",lookup,"",&origin,&good).starts_with("HTTP/1.1 403"));
            assert!(req("POST",lookup,c,"http://other.example",&good).starts_with("HTTP/1.1 403"));
            assert!(req("POST",lookup,c,&origin,&bad).starts_with("HTTP/1.1 403"));
            assert!(req("POST",lookup,c,&origin,&format!("{good}&file={file}")).starts_with("HTTP/1.1 400"));
            assert!(req("POST",lookup,c,&origin,&previous).starts_with("HTTP/1.1 409"));
            let absent=req("POST",lookup,c,&origin,&good);
            assert!(absent.starts_with("HTTP/1.1 200")&&absent.contains("未找到这份表单的已确认提交回执"));
            assert!(!absent.contains("版本 ID："));
            let no_nodes=req("GET",&history,c,"","");
            assert!(no_nodes.starts_with("HTTP/1.1 200")&&no_nodes.contains("尚无已提交版本")); // lookup did not submit or consume the original form
            let result=req("POST","/versions/commit",c,&origin,&good);
            assert!(result.starts_with("HTTP/1.1 200")&&result.contains("正式版本已提交"));
            let checked=req("POST",lookup,c,&origin,&good);
            assert!(checked.starts_with("HTTP/1.1 200")&&checked.contains("已查到原提交回执")&&checked.contains("本查询没有提交或重试"));
            assert!(checked.contains(&format!("提交操作：<code>{new_op}</code>")));
            let version_text=result.split_once("版本 ID：").unwrap().1.split_once("</code>").unwrap().0;
            assert!(checked.contains(&format!("版本 ID：{version_text}</code>")));
            let history_result=req("GET",&history,c,"","");
            assert!(history_result.starts_with("HTTP/1.1 200")&&history_result.contains("Web&#39;).xlsx")&&history_result.contains("逻辑字节")&&!history_result.contains("/download"));
            assert_eq!(req("POST","/versions/commit",c,&origin,&good),result);
            let preview=req("GET",&view,c,"","");
            assert!(preview.starts_with("HTTP/1.1 409")&&preview.contains("工作稿已变化"));
            let stale=req("POST","/versions/commit",c,&origin,&good);
            assert!(stale.starts_with("HTTP/1.1 409")&&stale.contains("这次没有提交，也未核对上一次的结果"));
            assert!(req("POST",lookup,c,&origin,&good).starts_with("HTTP/1.1 409"));
            (old_op,new_op,result,history_result)
        });
        transport::serve(&ls,&mut App::new(&source),||stop.load(Ordering::Acquire)).unwrap();
        client.join().unwrap()
    });
    assert!(admin.manual_target(file).is_err());
    assert!(admin.freeze(old_op,target).is_err());
    let frozen=admin.freeze(new_op,target).unwrap();
    assert_eq!((frozen.target.file,frozen.ord),(file,1));
    assert!(result.contains(&format!("版本 ID：<code>{}</code>",frozen.version)));
    assert!(history_result.contains(&format!("<code>{}</code>",frozen.version))&&history_result.contains(&format!("<code>{}</code>",frozen.node)));
}


#[test]
#[ignore="isolated loopback TCP and fresh SQLite; tests no live port or deployed site"]
fn failed_private_web_ready_keeps_the_original_listener_unserved(){
    use crate::system::reader::ordinary_pool;
    let temp=PrivateTemp::under(&std::env::temp_dir());
    let (path,pool,_,_,_)=ordinary_pool(&temp);
    let source=InitialReader::open(&path,pool).unwrap();
    let listener=TcpListener::bind(("127.0.0.1",0)).unwrap();
    let _peer=TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let count=std::cell::Cell::new(0);
    let rejected=serve_ready_original(&source,&listener,||->io::Result<fn()->bool>{
        count.set(count.get()+1);
        Err(io::ErrorKind::PermissionDenied.into())
    });
    assert_eq!(count.get(),1);
    assert_eq!(rejected.unwrap_err().kind(),io::ErrorKind::PermissionDenied);
    // The already queued real connection still belongs to the exact same
    // listener; no gated worker accepted it during or after the failed cut.
    assert!(listener.accept().is_ok());
}
