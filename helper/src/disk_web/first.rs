//! A one-shot browser handoff on its own already isolated listener. It has
//! a typed phase and count, fixed repair format and size, or opt-in parent-rendered
//! setup unit text. The browser supplies no paths, executable, or unit contents.
use super::{Panel,Reply,Request,digest,http,matches,token};
use crate::volume_images::repair::RepairFormat;
use std::{io,net::TcpListener,thread,time::{Duration,Instant}};
const TICKET:Duration=Duration::from_secs(90);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InstallPhase { Resume, Initialize, Account }
impl InstallPhase {
    pub(crate) fn wire(self) -> u8 {
        match self { Self::Resume => 0, Self::Initialize => 1, Self::Account => 2 }
    }
    pub(crate) fn from_wire(value: u8) -> Option<Self> {
        match value { 0 => Some(Self::Resume), 1 => Some(Self::Initialize), 2 => Some(Self::Account), _ => None }
    }
    fn name(self) -> &'static str {
        match self { Self::Resume => "resume", Self::Initialize => "new", Self::Account => "setup" }
    }
}
enum Purpose{Headless,HeadlessUnit(String),Install{phase:InstallPhase,count:u8},CopyRepair{format:RepairFormat,bytes:u64}}
pub(super) struct Offer{
    purpose:Purpose,ticket:Option<([u8;32],Instant)>,requested:bool,
}
impl Offer{
    pub(super) fn is_repair(&self)->bool{matches!(self.purpose,Purpose::CopyRepair{..})}
    pub(super) fn route(&mut self,r:Request,now:Instant)->(Reply,bool){
        let (preview,request,requested,cancelled)=match self.purpose{
            Purpose::Headless | Purpose::HeadlessUnit(_) | Purpose::Install{..}=>("/api/install/preview","/api/install/request",
                "请求已交回本机终端。网页将关闭；尚未启用、设密或写盘。","安装请求已取消。未写入。"),
            Purpose::CopyRepair{..}=>("/api/copy-repair/preview","/api/copy-repair/request",
                "请求已交回本机终端。网页将关闭；尚未读取或修复镜像。","工作副本请求已取消。未写入。"),
        };
        if r.path==request{
            if r.content_type.as_deref()!=Some("application/octet-stream")||r.body.len()!=64{
                return (Reply::plain(400,"这里只接受本机预览票。"),false);
            }
            if self.requested||self.ticket.is_none_or(|(h,until)|now>=until||!matches(&r.body,&h)){
                return (Reply::plain(403,"预览票不符或已失效。尚未写入。"),false);
            }
            self.ticket=None;self.requested=true;
            return (Reply::plain(200,requested),true);
        }
        if !r.body.is_empty()||r.content_type.is_some(){
            return (Reply::plain(400,"这里不接受路径、口令、操作 ID 或工具选项。"),false);
        }
        match r.path.as_str(){
            p if p==preview && !self.requested=>{
                let Ok(ticket)=token()else{return (Reply::plain(503,"暂不能准备本机确认票。"),false);};
                self.ticket=Some((digest(ticket.as_bytes()),now+TICKET));
                let page=match self.purpose{
                    Purpose::Headless=>format!("headless\n1\n{ticket}"),
                    Purpose::HeadlessUnit(ref unit)=>format!("headless\n1\n{ticket}\n{unit}"),
                    Purpose::Install{phase,count}=>format!("{}\n{count}\n{ticket}",phase.name()),
                    Purpose::CopyRepair{format,bytes}=>format!("{}\n{bytes}\n{ticket}",format.panel_label()),
                };
                (Reply::plain(200,page),false)
            }
            "/api/close"=>(Reply::plain(200,cancelled),true),
            _=>(Reply::plain(404,"此页面只提供本次原范围预览和发起。"),false),
        }
    }
}
pub(crate) fn serve_first(listener:TcpListener,phase:InstallPhase,count:u8,code:[u8;32],end:Instant,
    check:impl Fn()->io::Result<()>)->io::Result<bool>{
    if !(1..=62).contains(&count){return Err(io::Error::other("invalid fixed installation panel scope"));}
    serve(listener,Purpose::Install{phase,count},zeroize::Zeroizing::new(code),end,check)
}
pub(super) fn serve_headless(listener: TcpListener, code: zeroize::Zeroizing<[u8; 32]>, end: Instant,
    unit: Option<String>, check: impl Fn() -> io::Result<()>) -> io::Result<bool> {
    let purpose = unit.map_or(Purpose::Headless, Purpose::HeadlessUnit);
    serve(listener, purpose, code, end, check)
}

pub(crate) fn serve_repair(listener:TcpListener,format:RepairFormat,bytes:u64,code:[u8;32],end:Instant,
    check:impl Fn()->io::Result<()>)->io::Result<bool>{
    if !(1..=i64::MAX as u64).contains(&bytes){return Err(io::Error::other("invalid fixed copy-repair panel scope"));}
    serve(listener,Purpose::CopyRepair{format,bytes},zeroize::Zeroizing::new(code),end,check)
}
fn serve(listener:TcpListener,purpose:Purpose,code:zeroize::Zeroizing<[u8;32]>,end:Instant,
    check:impl Fn()->io::Result<()>)->io::Result<bool>{
    if *code==[0;32]{return Err(io::Error::other("invalid fixed panel pairing code"));}
    let hex = b"0123456789abcdef";
    let mut text = zeroize::Zeroizing::new([0; 64]);
    for (byte, pair) in code.iter().zip(text.chunks_exact_mut(2)) {
        pair[0] = hex[usize::from(*byte >> 4)];
        pair[1] = hex[usize::from(*byte & 15)];
    }
    drop(code);
    let mut panel=Panel::new(listener.local_addr()?.to_string(),&text[..],Instant::now());
    drop(text);
    panel.handoff=Some(Offer{purpose,ticket:None,requested:false});
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
    Ok(panel.handoff.is_some_and(|o|o.requested))
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
    fn headless_preview_can_only_return_one_ticket_to_its_parent() {
        let now = Instant::now();
        let mut offer = Offer { purpose: Purpose::Headless, ticket: None, requested: false };
        let (preview, closed) = offer.route(post("/api/install/preview", b""), now);
        assert!(!closed && !offer.requested);
        let fields: Vec<_> = preview.body.lines().collect();
        assert_eq!(&fields[..2], &["headless", "1"]);
        assert_eq!(offer.route(post("/api/install/request", b"password"), now).0.code, 400);
        let (reply, closed) = offer.route(post("/api/install/request", fields[2].as_bytes()), now);
        assert_eq!(reply.code, 200);
        assert!(closed && offer.requested);
        assert_eq!(offer.route(post("/api/install/request", fields[2].as_bytes()), now).0.code, 403);
    }

    #[test]
    fn install_request_is_one_existing_ticket_not_a_path_or_a_disk_command(){
        let now=Instant::now();let token="a".repeat(64);
        let mut offer=Offer{purpose:Purpose::Install{phase:InstallPhase::Initialize,count:2},ticket:Some((digest(token.as_bytes()),now+TICKET)),requested:false};
        assert_eq!(offer.route(post("/api/install/request",b"/host/disk"),now).0.code,400);
        assert_eq!(offer.route(post("/api/copy/preview",b""),now).0.code,404);
        assert_eq!(offer.route(post("/api/copy-repair/preview",b""),now).0.code,404);
        assert_eq!(offer.route(post("/api/install/request",token.as_bytes()),now+TICKET).0.code,403);
        let (message,close)=offer.route(post("/api/install/request",token.as_bytes()),now);
        assert_eq!(message.code,200);assert!(close&&offer.requested);
        assert_eq!(offer.route(post("/api/install/request",token.as_bytes()),now).0.code,403);
    }
    #[test]
    fn guided_account_is_a_separate_preview_but_only_hands_a_ticket_to_its_parent() {
        assert_eq!(InstallPhase::from_wire(0), Some(InstallPhase::Resume));
        assert_eq!(InstallPhase::from_wire(1), Some(InstallPhase::Initialize));
        assert_eq!(InstallPhase::from_wire(2), Some(InstallPhase::Account));
        assert_eq!(InstallPhase::from_wire(3), None);
        assert_eq!(InstallPhase::Account.wire(), 2);

        let now = Instant::now();
        let mut offer = Offer {
            purpose: Purpose::Install { phase: InstallPhase::Account, count: 2 },
            ticket: None, requested: false,
        };
        let (preview, close) = offer.route(post("/api/install/preview", b""), now);
        assert_eq!(preview.code, 200);
        assert!(!close && !offer.requested);
        let mut fields = preview.body.lines();
        assert_eq!((fields.next(), fields.next()), (Some("setup"), Some("2")));
        let ticket = fields.next().unwrap();
        assert_eq!(fields.next(), None);
        assert_eq!(offer.route(post("/api/install/request", b"account text"), now).0.code, 400);
        assert_eq!(offer.route(post("/api/copy-repair/request", ticket.as_bytes()), now).0.code, 400);
        assert!(!offer.requested);
        let (reply, close) = offer.route(post("/api/install/request", ticket.as_bytes()), now);
        assert_eq!(reply.code, 200);
        assert!(close && offer.requested);
        assert_eq!(offer.route(post("/api/install/request", ticket.as_bytes()), now).0.code, 403);
    }
    #[test]
    fn repair_handoff_still_requires_the_exact_origin_and_both_paired_credentials(){
        let now=Instant::now();let token="a".repeat(64);let bearer="b".repeat(64);let csrf="c".repeat(64);
        let mut panel=Panel::new("127.0.0.1:1900".into(),b"not-used",now);
        panel.session=Some(super::super::Session{bearer:digest(bearer.as_bytes()),csrf:digest(csrf.as_bytes())});
        panel.handoff=Some(Offer{purpose:Purpose::CopyRepair{format:RepairFormat::Exfat,bytes:4096},ticket:Some((digest(token.as_bytes()),now+TICKET)),requested:false});
        let request=|path:&str|{
            let mut r=post(path,token.as_bytes());r.auth=Some(format!("Bearer {bearer}"));r.csrf=Some(csrf.clone());r
        };
        let mut origin=request("/api/copy-repair/request");origin.origin=Some("http://localhost:1900".into());
        assert_eq!(panel.handle(origin,now).code,403);
        let mut cookie=request("/api/copy-repair/request");cookie.csrf=None;
        assert_eq!(panel.handle(cookie,now).code,403);
        assert_eq!(panel.handle(request("/api/install/request"),now).code,400);
        assert!(!panel.closed);
        assert_eq!(panel.handle(request("/api/copy-repair/request"),now).code,200);
        assert!(panel.closed);
    }
    #[test]
    fn repair_page_displays_only_the_parent_selected_format_and_length(){
        let now=Instant::now();
        for (format,label) in [(RepairFormat::Ext4,"ext"),(RepairFormat::Exfat,"exFAT"),(RepairFormat::Fat32,"FAT32")]{
            let mut offer=Offer{purpose:Purpose::CopyRepair{format,bytes:4096},ticket:None,requested:false};
            let (page,close)=offer.route(post("/api/copy-repair/preview",b""),now);
            assert!(!close);assert_eq!(page.code,200);
            let mut fields=page.body.lines();
            assert_eq!(fields.next(),Some(label));assert_eq!(fields.next(),Some("4096"));
            let ticket=fields.next().unwrap();assert!(matches(ticket.as_bytes(),&offer.ticket.unwrap().0));
            assert!(fields.next().is_none());
            assert_eq!(offer.route(post("/api/copy-repair/request",b"exFAT"),now).0.code,400);
            assert_eq!(offer.route(post("/api/copy-repair/request",ticket.as_bytes()),now).0.code,200);
        }
    }
    #[test]
    fn repair_preview_has_a_separate_purpose_without_files_and_consumes_one_ticket(){
        let now=Instant::now();let token="a".repeat(64);
        let mut offer=Offer{purpose:Purpose::CopyRepair{format:RepairFormat::Fat32,bytes:i64::MAX as u64},ticket:Some((digest(token.as_bytes()),now+TICKET)),requested:false};
        assert!(offer.is_repair());
        assert_eq!(offer.route(post("/api/install/request",token.as_bytes()),now).0.code,400);
        assert_eq!(offer.route(post("/api/disks",b""),now).0.code,404);
        assert_eq!(offer.route(post("/api/copy-repair/preview",b"--repair /path"),now).0.code,400);
        assert_eq!(offer.route(post("/api/copy-repair/request",b"/host/image"),now).0.code,400);
        assert_eq!(offer.route(post("/api/copy-repair/request",token.as_bytes()),now+TICKET).0.code,403);
        let (message,close)=offer.route(post("/api/copy-repair/request",token.as_bytes()),now);
        assert_eq!(message.code,200);assert!(close&&offer.requested);
        assert_eq!(offer.route(post("/api/copy-repair/request",token.as_bytes()),now).0.code,403);
    }
}
