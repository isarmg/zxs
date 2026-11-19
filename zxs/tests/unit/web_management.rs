//! Real private SQLite + separate Unix-stream endpoints. This fixture does not
//! start nginx/systemd, establish TLS, use actual user credentials, or open SMB.
use super::*;
use crate::{fs,system::reader::web_pool,test_support::PrivateTemp};
use std::{io::{Read,Write},net::Shutdown,os::unix::net::UnixStream};
const HOST:&str="admin.example";
const SITE:&str="https://admin.example";
fn hit(app:&mut App<'_>,method:&str,uri:&str,cookie:&str,origin:&str,body:&str)->String{
    let mut fields=Vec::new();
    for(k,v)in [("CONTENT_LENGTH",body.len().to_string()),("SCGI","1".into()),("REQUEST_METHOD",method.into()),("REQUEST_URI",uri.into()),
        ("HTTPS","on".into()),("HTTP_HOST",HOST.into()),("HTTP_COOKIE",cookie.into()),("HTTP_ORIGIN",origin.into()),
        ("CONTENT_TYPE",if method=="POST"{"application/x-www-form-urlencoded".into()}else{"".into()})]{
        fields.extend(k.bytes());fields.push(0);fields.extend(v.bytes());fields.push(0);
    }
    let mut wire=format!("{}:",fields.len()).into_bytes();wire.extend(fields);wire.push(b',');wire.extend(body.bytes());
    let (mut client,server)=UnixStream::pair().unwrap();let task=std::thread::spawn(move||{
        client.write_all(&wire).unwrap();client.shutdown(Shutdown::Write).unwrap();
        let mut out=String::new();client.read_to_string(&mut out).unwrap();out
    });
    // Only this socket-pair fixture selects its own observed uid for role of
    // proxy. Production startup requires a distinct configured non-root uid.
    transport::respond(app,server,fs::local_uid()).unwrap();task.join().unwrap()
}
fn hidden<'a>(page:&'a str,action:&str,key:&str)->&'a str{
    let needle=format!("action=\"{action}\"");let form=page.split_once(needle.as_str()).unwrap().1.split_once("</form>").unwrap().0;
    let name=format!("name=\"{key}\" value=\"");form.split_once(name.as_str()).unwrap().1.split_once('"').unwrap().0
}
fn logged(app:&mut App<'_>)->String{
    let reply=hit(app,"POST","/login","",SITE,"login=first.admin&password=correct+horse+battery+staple");
    assert!(reply.starts_with("Status: 303")&&reply.contains("Path=/; Secure; HttpOnly; SameSite=Strict"));
    reply.lines().find_map(|v|v.strip_prefix("Set-Cookie: ")).unwrap().split(';').next().unwrap().to_owned()
}
#[test]
#[ignore="source-only private SQLite/Argon2 and real Unix socket-pair qualification, not a TLS or browser run"]
fn login_publish_mode_and_cold_management_page_use_one_real_database_without_opening_offline_file_roots(){
    let t=PrivateTemp::under(&std::env::temp_dir());let (p,id)=web_pool(&t);
    let source=InitialReader::open(&p,id).unwrap();let mut app=App::new(&source,SITE).unwrap();
    let anon=hit(&mut app,"GET","/","","","");assert!(anon.contains("Location: /login")&&!anon.contains("first.admin"));
    let form=hit(&mut app,"GET","/login","","","");
    assert!(form.contains("Referrer-Policy: same-origin\r\n"));
    assert!(form.contains("autocomplete=\"current-password\"")&&!form.contains("files.smb")&&!form.contains("本池登记卷"));
    for who in ["files.smb","anonymous","guest"]{
        let out=hit(&mut app,"POST","/login","",SITE,&format!("login={who}&password=correct+horse+battery+staple"));
        assert!(out.starts_with("Status: 403")&&!out.contains("Set-Cookie: "));
    }
    assert!(hit(&mut app,"POST","/login","",SITE,"login=first.admin&password=wrong").starts_with("Status: 403"));
    assert!(hit(&mut app,"POST","/login","","https://other.example","login=first.admin&password=correct+horse+battery+staple").starts_with("Status: 403"));
    for opaque in ["","null"]{assert!(hit(&mut app,"POST","/login","",opaque,"login=first.admin&password=correct+horse+battery+staple").starts_with("Status: 403"));}
    let cookie=logged(&mut app);let page=hit(&mut app,"GET","/",&cookie,"","");
    assert!(page.contains("Referrer-Policy: same-origin\r\n"));
    assert!(page.contains("first.admin")&&page.contains("files.smb")&&page.contains("仅有登记")&&!page.contains("unmounted-"));
    let op=hidden(&page,"/shares/publish","op");let csrf=hidden(&page,"/shares/publish","csrf");
    let publish=format!("csrf={csrf}&op={op}&volume={}&label=R%26D",crate::VolumeId([0x13;16]));
    assert!(hit(&mut app,"POST","/shares/publish",&cookie,"",&publish).starts_with("Status: 403"));
    assert!(hit(&mut app,"POST","/shares/publish",&cookie,SITE,&publish.replace(csrf,&"0".repeat(64))).starts_with("Status: 403"));
    for _ in 0..2{assert!(hit(&mut app,"POST","/shares/publish",&cookie,SITE,&publish).starts_with("Status: 303"));}
    let page=hit(&mut app,"GET","/",&cookie,"","");
    assert_eq!(page.matches("<bdi>R&amp;D</bdi>").count(),1);assert!(page.contains("<td>停用</td>"));
    let service=format!("zxs-{}",hidden(&page,"/shares/mode","share"));
    assert!(page.contains(format!("<code>{service}</code>").as_str()));assert!(!page.contains("smb://"));
    assert!(!hit(&mut app,"GET","/login","","","").contains(service.as_str()));
    let mode=format!("csrf={}&op={}&share={}&previous={}&mode=rw",hidden(&page,"/shares/mode","csrf"),
        hidden(&page,"/shares/mode","op"),hidden(&page,"/shares/mode","share"),hidden(&page,"/shares/mode","previous"));
    assert!(hit(&mut app,"POST","/shares/mode",&cookie,SITE,&mode).starts_with("Status: 303"));
    assert!(hit(&mut app,"GET","/file/download",&cookie,"","").starts_with("Status: 404"));
    let absent=p.parent().unwrap().join("unmounted-r");assert!(!absent.exists());
    drop(app);drop(source);let source=InitialReader::open(&p,id).unwrap();let mut cold=App::new(&source,SITE).unwrap();
    assert!(hit(&mut cold,"GET","/",&cookie,"","").contains("Location: /login"));
    let cookie=logged(&mut cold);let page=hit(&mut cold,"GET","/",&cookie,"","");
    assert!(page.contains("<bdi>R&amp;D</bdi>")&&page.contains("<td>读写</td>"));
    assert!(page.contains(format!("<code>{service}</code>").as_str()));
    let exit=format!("csrf={}",hidden(&page,"/logout","csrf"));
    assert!(hit(&mut cold,"POST","/logout",&cookie,"https://other.example",&exit).starts_with("Status: 403"));
    assert!(hit(&mut cold,"POST","/logout",&cookie,SITE,&exit).contains("Max-Age=0"));
    assert!(hit(&mut cold,"GET","/",&cookie,"","").contains("Location: /login"));
}
#[test]
fn native_unix_peer_check_is_not_an_http_header(){
    let (other,endpoint)=UnixStream::pair().unwrap();
    use std::os::fd::AsRawFd;
    unsafe extern "C"{fn zxs_web_peer(fd:std::os::raw::c_int,uid:u32)->std::os::raw::c_int;}
    assert_eq!(unsafe{zxs_web_peer(endpoint.as_raw_fd(),fs::local_uid())},0);
    assert_ne!(unsafe{zxs_web_peer(endpoint.as_raw_fd(),fs::local_uid()^1)},0);
    drop(other);
}
