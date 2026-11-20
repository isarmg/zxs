//! First-install handoff on its own already isolated local listener. It has
//! only a label and count, never a path, device descriptor or root command.
use super::{Panel,Reply,Request,digest,http,matches,token};
use std::{io,net::TcpListener,thread,time::{Duration,Instant}};
const TICKET:Duration=Duration::from_secs(90);
pub(super) struct Offer{
    fresh:bool,count:u8,ticket:Option<([u8;32],Instant)>,pub(super) requested:bool,
}
impl Offer{
    pub(super) fn route(&mut self,r:Request,now:Instant)->(Reply,bool){
        if r.path=="/api/install/request"{
            if r.content_type.as_deref()!=Some("application/octet-stream")||r.body.len()!=64{
                return (Reply::plain(400,"这里只接受本机预览票。"),false);
            }
            if self.requested||self.ticket.is_none_or(|(h,until)|now>=until||!matches(&r.body,&h)){
                return (Reply::plain(403,"预览票不符或已失效。尚未写入。"),false);
            }
            self.ticket=None;self.requested=true;
            return (Reply::plain(200,"请求已交回本机终端。网页将关闭；尚未启用、设密或写盘。"),true);
        }
        if !r.body.is_empty()||r.content_type.is_some(){
            return (Reply::plain(400,"这里不接受路径、口令、操作 ID 或工具选项。"),false);
        }
        match r.path.as_str(){
            "/api/install/preview" if !self.requested=>{
                let Ok(ticket)=token()else{return (Reply::plain(503,"暂不能准备本机确认票。"),false);};
                self.ticket=Some((digest(ticket.as_bytes()),now+TICKET));
                (Reply::plain(200,format!("{}\n{}\n{ticket}",if self.fresh{"new"}else{"resume"},self.count)),false)
            }
            "/api/close"=>(Reply::plain(200,"安装请求已取消。未写入。"),true),
            _=>(Reply::plain(404,"此安装页面只提供原范围预览和发起。"),false),
        }
    }
}
pub(crate) fn serve_first(listener:TcpListener,fresh:bool,count:u8,code:[u8;32],end:Instant,
    check:impl Fn()->io::Result<()>)->io::Result<bool>{
    if !(1..=62).contains(&count)||code==[0;32]{return Err(io::Error::other("invalid fixed panel scope"));}
    let code=code.iter().map(|b|format!("{b:02x}")).collect::<String>();
    let mut panel=Panel::new(listener.local_addr()?.to_string(),code.as_bytes(),Instant::now());
    drop(code);
    panel.install=Some(Offer{fresh,count,ticket:None,requested:false});
    let mut served=0;
    while !panel.closed&&Instant::now()<panel.expires.min(end)&&served<256{
        check()?;
        match listener.accept(){
            Ok((mut sock,peer)) if peer.ip().is_loopback()=>{
                served+=1;sock.set_nonblocking(false)?;
                let r=http::read(&mut sock);check()?;
                if Instant::now()>=end{break;}
                let result=match r{Ok(r)=>panel.handle(r,Instant::now()),
                    Err(_)=>Reply::plain(400,"请求无效或超过时限。")};
                let _=result.send(&mut sock);
            }
            Ok(_)=>(),
            Err(e) if e.kind()==io::ErrorKind::WouldBlock=>thread::sleep(Duration::from_millis(30)),
            Err(e) if e.kind()==io::ErrorKind::Interrupted=>(),
            Err(e)=>return Err(e),
        }
    }
    drop(listener);
    check()?;
    Ok(panel.install.is_some_and(|o|o.requested))
}

#[cfg(test)]
mod tests{
    use super::*;
    fn post(path:&str,body:&[u8])->Request{
        Request{method:"POST".into(),path:path.into(),host:"127.0.0.1:1900".into(),
            origin:Some("http://127.0.0.1:1900".into()),auth:None,csrf:None,
            content_type:(!body.is_empty()).then(||"application/octet-stream".into()),body:body.into()}
    }
    #[test]
    fn request_is_one_existing_ticket_not_a_path_or_a_disk_command(){
        let now=Instant::now();let token="a".repeat(64);
        let mut offer=Offer{fresh:true,count:2,ticket:Some((digest(token.as_bytes()),now+TICKET)),requested:false};
        assert_eq!(offer.route(post("/api/install/request",b"/host/disk"),now).0.code,400);
        assert_eq!(offer.route(post("/api/copy/preview",b""),now).0.code,404);
        assert_eq!(offer.route(post("/api/install/request",token.as_bytes()),now+TICKET).0.code,403);
        let (message,close)=offer.route(post("/api/install/request",token.as_bytes()),now);
        assert_eq!(message.code,200);assert!(close&&offer.requested);
        assert_eq!(offer.route(post("/api/install/request",token.as_bytes()),now).0.code,403);
    }
}
