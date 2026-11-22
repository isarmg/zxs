//! Browser-only responses from the reviewed stage embedded in this ELF.
use super::{App,Csp,LOGIN_REQUIRED,REJECT,Reply,Request,pages,session_live_at};
use crate::system::reader::account::overview::Overview;
use std::{borrow::Cow,io,time::Instant};

struct Asset{path:&'static str,ty:&'static str,bytes:&'static [u8]}
include!(concat!(env!("OUT_DIR"),"/browser_assets.rs"));

fn asset(path:&str)->Option<&'static Asset>{ASSETS.iter().find(|a|a.path==path)}
impl Asset{
    fn reply(&self)->Reply{Reply::content("200 OK",self.ty,Cow::Borrowed(self.bytes))}
}

impl App<'_>{
    pub(super) fn browser_asset(&self,q:&Request)->Option<Reply>{
        let asset=asset(&q.target)?;
        if !q.same_origin_if_present(){return Some(REJECT());}
        let live=q.cookie.as_ref().and_then(|key|self.sessions.get(key)).is_some_and(|s|
            s.audience==q.audience&&session_live_at(s.born,s.last,Instant::now()));
        Some(if live{asset.reply()}else{LOGIN_REQUIRED()})
    }
}

pub(super) fn overview(data:&Overview,csrf:&str)->io::Result<Reply>{
    let mut page=Reply::html("200 OK",pages::browser_overview(data,csrf)?);
    page.csp=Csp::BrowserOverview;
    Ok(page)
}

#[cfg(test)]
mod tests{
    use super::*;
    use super::super::BAD;

    fn written(reply:Reply)->Vec<u8>{
        let mut out=Vec::new();
        reply.write(&mut out).unwrap();
        out
    }
    fn components(wire:&[u8])->(&str,&[u8]){
        let at=wire.windows(4).position(|p|p==b"\r\n\r\n").unwrap();
        (std::str::from_utf8(&wire[..at]).unwrap(),&wire[at+4..])
    }

    #[test]
    fn browser_assets_are_four_exact_binary_responses(){
        assert_eq!(ASSETS.len(),4);
        for (path,ty) in [
            ("/ui/v1/boot.js","text/javascript; charset=utf-8"),
            ("/ui/v1/folder.css","text/css; charset=utf-8"),
            ("/ui/v1/zxs_browser.js","text/javascript; charset=utf-8"),
            ("/ui/v1/zxs_browser_bg.wasm","application/wasm"),
        ]{
            let a=asset(path).unwrap();
            let wire=written(a.reply());
            let (head,body)=components(&wire);
            assert!(head.contains(&format!("Content-Type: {ty}\r\n")));
            assert!(head.contains(&format!("Content-Length: {}\r\n",a.bytes.len())));
            assert!(head.contains("Cache-Control: no-store\r\n"));
            assert!(head.contains("X-Content-Type-Options: nosniff\r\n"));
            assert!(!head.contains("wasm-unsafe-eval"));
            assert_eq!(body,a.bytes);
            if ty=="application/wasm"{assert!(body.starts_with(b"\0asm\x01\0\0\0"));}
        }
        for path in ["/ui/v1/","/ui/v1/provenance.json","/ui/v1/boot.js?x",
            "/ui/v1/BOOT.js","/ui/v1/%62oot.js","/ui/v1/other/boot.js"]{
            assert!(asset(path).is_none());
        }
    }

    #[test]
    fn browser_overview_alone_enables_local_modules_and_wasm(){
        let data=Overview{login:"Example".into(),shares:Vec::new(),next:None};
        let wire=written(overview(&data,"csrf").unwrap());
        let (head,body)=components(&wire);
        assert!(head.contains("script-src 'self' 'wasm-unsafe-eval'; connect-src 'self'; worker-src 'none';"));
        assert!(!head.contains("'unsafe-eval'")&&!head.contains("'unsafe-inline'"));
        let page=std::str::from_utf8(body).unwrap();
        assert!(page.contains("src=\"/ui/v1/boot.js\"")&&page.contains("href=\"/ui/v1/folder.css\""));
        assert!(page.contains("action=\"/logout\"")&&page.contains("name=\"csrf\" value=\"csrf\""));
        for reply in [Reply::html("200 OK",pages::sign_in(false)), BAD(), pages::manual_index(&[],None)]{
            let wire=written(reply);
            let (head,body)=components(&wire);
            assert!(!head.contains("script-src")&&!head.contains("connect-src"));
            assert!(!std::str::from_utf8(body).unwrap().contains("/ui/v1/"));
        }
    }

    #[test]
    fn browser_dispatch_uses_the_original_admin_session_and_metadata_cut(){
        use super::super::{ABSOLUTE,Audience,Token,transport::PrivateBytes};
        use crate::{OperationId,VolumeId,system::reader::{InitialReader,account::share_mode::Mode,web_pool},
            test_support::PrivateTemp};
        let temp=PrivateTemp::under(&std::env::temp_dir());
        let (system,pool)=web_pool(&temp);
        let source=InitialReader::open(&system,pool).unwrap();
        let setup=source.authenticate_web("first.admin","correct horse battery staple").unwrap();
        let row=setup.publish_root(OperationId([0xa4;16]),VolumeId([0x13;16]),"Documents").unwrap();
        setup.set_share_mode(OperationId([0xa5;16]),row.share,row.op,Mode::ReadOnly).unwrap();
        drop(setup);
        let mut app=App::new(&source);
        let audience=Audience{origin:"http://192.0.2.10:8443".into()};
        let request=|method:&str,target:&str,origin:Option<&str>,cookie:Option<&str>,body:&str|Request{
            method:method.into(),target:target.into(),origin:origin.map(str::to_owned),audience:audience.clone(),
            cookie:cookie.map(|s|Token::parse(s).unwrap()),content_type:"application/x-www-form-urlencoded".into(),
            body:PrivateBytes(body.as_bytes().to_vec()),
        };
        let path=format!("/ui/v1/d/{}/-/-",row.share);
        let boot="/ui/v1/boot.js";
        let start=Instant::now();
        for path in [&path,boot]{
            assert_eq!(app.handle(request("GET",path,None,None,""),start).status,"401 Unauthorized");
        }
        assert_eq!(app.handle(request("GET","/",None,None,""),start).location,Some("/login"));
        let login=app.handle(request("POST","/login",Some(&audience.origin),None,
            "login=first.admin&password=correct+horse+battery+staple"),start);
        assert_eq!(login.location,Some("/"));
        let cookie=login.set_cookie.as_deref().unwrap().strip_prefix("zxs-http=").unwrap().split(';').next().unwrap();
        let cookie=Some(cookie);
        let shell=app.handle(request("GET","/",None,cookie,""),Instant::now());
        assert!(matches!(shell.csp,Csp::BrowserOverview));
        assert!(std::str::from_utf8(&shell.body).unwrap().contains("Documents"));
        let key=Token::parse(cookie.unwrap()).unwrap();
        let last=app.sessions.get(&key).unwrap().last;
        assert_eq!(app.handle(request("GET",boot,None,cookie,""),Instant::now()).status,"200 OK");
        assert_eq!(app.sessions.get(&key).unwrap().last,last);
        for path in [&path,boot]{
            assert_eq!(app.handle(request("GET",path,Some("https://other.example"),cookie,""),Instant::now()).status,"403 Forbidden");
        }
        let page=app.handle(request("GET",&path,None,cookie,""),Instant::now());
        assert_eq!(page.status,"200 OK");
        assert_eq!(&*page.body,b"{\"ancestors\":[],\"entries\":[],\"next\":null}");
        assert_eq!(app.handle(request("GET","/ui/v1/provenance.json",None,cookie,""),Instant::now()).status,"404 Not Found");
        assert_eq!(app.handle(request("GET",&path,None,cookie,""),start+ABSOLUTE).status,"401 Unauthorized");
        assert!(app.sessions.is_empty());
    }
}
