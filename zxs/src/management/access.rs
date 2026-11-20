//! An authority is selected from the accepted socket, never from a forwarding
//! header. A token is usable on exactly one transport and browser origin.
use crate::fs::invalid;
use std::{io,net::{IpAddr,Ipv6Addr,SocketAddr}};

#[derive(Clone,PartialEq,Eq)]
pub(super) struct Audience{pub origin:String,pub secure:bool}
impl Audience{
    pub fn cookie(&self)->&'static str{if self.secure{"__Host-zxs"}else{"zxs-http"}}
    pub fn set_cookie(&self,value:&str,expired:bool)->String{
        format!("{}={value}; Path=/; {}HttpOnly; SameSite=Strict{}",self.cookie(),
            if self.secure{"Secure; "}else{""},if expired{"; Max-Age=0"}else{""})
    }
}
/// Optional external TLS terminator, solely in trusted startup configuration.
/// The peer address/route must be reserved to this terminator by the operator.
pub struct ExternalHttps{peer:IpAddr,origin:String,authority:String}
impl ExternalHttps{
    pub fn new(peer:IpAddr,origin:&str)->io::Result<Self>{
        let bad=||invalid("invalid external HTTPS origin or trusted proxy IP");
        let a=origin.strip_prefix("https://").filter(|s|!s.is_empty()&&s.len()<=259).ok_or_else(bad)?;
        let port_ok=|s:&str|s.parse::<u16>().is_ok_and(|v|v!=0&&v!=443&&v.to_string()==s);
        let ok=if let Some(rest)=a.strip_prefix('['){
            rest.split_once(']').is_some_and(|(ip,suffix)|ip.parse::<Ipv6Addr>().is_ok_and(|v|v.to_string()==ip&&!v.is_unspecified()&&!v.is_multicast())&&
                (suffix.is_empty()||suffix.strip_prefix(':').is_some_and(port_ok)))
        }else{
            let(host,port)=a.split_once(':').map_or((a,None),|(h,p)|(h,Some(p)));
            !host.is_empty()&&host.len()<=253&&host.split('.').all(|l|!l.is_empty()&&l.len()<=63&&!l.starts_with('-')&&!l.ends_with('-')&&l.bytes().all(|c|c.is_ascii_lowercase()||c.is_ascii_digit()||c==b'-'))&&port.is_none_or(port_ok)
        };
        if !peer.is_ipv4()||peer.is_unspecified()||peer.is_multicast()||!ok{return Err(bad());}
        Ok(Self{peer,origin:origin.to_owned(),authority:a.to_owned()})
    }
}
pub(super) fn select(local:SocketAddr,peer:IpAddr,host:&str,proxy:Option<&ExternalHttps>)->Option<Audience>{
    if let Some(p)=proxy.filter(|p|p.peer==peer&&p.authority==host){
        return Some(Audience{origin:p.origin.clone(),secure:true});
    }
    if local.ip().is_unspecified()||local.ip().is_multicast()||local.port()==0{return None;}
    let actual=if local.port()!=80{local.to_string()}else{match local.ip(){
        IpAddr::V4(ip)=>ip.to_string(),IpAddr::V6(ip)=>format!("[{ip}]")}};
    if host!=actual{return None;}
    Some(Audience{origin:format!("http://{actual}"),secure:false})
}
