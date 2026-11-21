//! Exact request audience. In proxy-only mode a kernel-observed local
//! socket and proof of the dedicated hop key precede every browser action.
//! Neither the hop nor forwarding headers can grant a Web session.
use crate::fs::invalid;
use std::{io,net::{IpAddr,Ipv4Addr,Ipv6Addr,SocketAddr}};

#[derive(Clone,PartialEq,Eq)]
pub(super) struct Audience{pub origin:String,pub secure:bool}
impl Audience{
    pub fn cookie(&self)->&'static str{if self.secure{"__Host-zxs"}else{"zxs-http"}}
    pub fn set_cookie(&self,value:&str,expired:bool)->String{
        format!("{}={value}; Path=/; {}HttpOnly; SameSite=Strict{}",self.cookie(),
            if self.secure{"Secure; "}else{""},if expired{"; Max-Age=0"}else{""})
    }
}

pub(crate) const MAX_HTTPS_ORIGIN:usize=8+259;
pub(super) const HOP_HEADER:&str="ZXS-Proxy-Key";
pub(crate) enum WebAccess{Direct,External(ExternalHttps)}

/// Run-scoped public policy only. The independent high-entropy bearer value
/// lives in the TLS proxy; ZXS's startup packet carries only this verifier.
pub(crate) struct ExternalHttps{origin:String,verifier:[u8;32]}
fn hex(b:u8)->Option<u8>{
    match b{b'0'..=b'9'=>Some(b-b'0'),b'a'..=b'f'=>Some(b-b'a'+10),_=>None}
}
fn verifier(s:&str)->Option<[u8;32]>{
    if s.len()!=64{return None;}
    let mut out=[0;32];
    for (b,p) in out.iter_mut().zip(s.as_bytes().chunks_exact(2)){
        *b=hex(p[0])?<<4|hex(p[1])?;
    }
    Some(out)
}
fn same_verifier(a:[u8;32],b:[u8;32])->bool{
    // Reuse the already compiled RustCrypto/ctutils fixed-output comparison.
    // This empty marker specifies a size, not a new MAC or hash algorithm.
    enum Size32{}
    impl digest::OutputSizeUser for Size32{type OutputSize=digest::consts::U32;}
    digest::CtOutput::<Size32>::new(a.into())==digest::CtOutput::<Size32>::new(b.into())
}
fn browser_host(host:&str)->bool{
    if let Ok(ip)=host.parse::<Ipv4Addr>(){
        return ip.to_string()==host&&!ip.is_unspecified()&&!ip.is_multicast()&&!ip.is_broadcast();
    }
    if host.is_empty()||host.len()>253{return false;}
    let valid_label=|l:&str|!l.is_empty()&&l.len()<=63&&!l.starts_with('-')&&!l.ends_with('-')&&
        !l.starts_with("xn--")&&l.bytes().all(|c|c.is_ascii_lowercase()||c.is_ascii_digit()||c==b'-');
    if !host.split('.').all(valid_label){return false;}
    // Browsers treat a final numeric/0x label as IPv4, not DNS. A
    // noncanonical or abbreviated IPv4 spelling must not pass as a name.
    let last=host.rsplit('.').next().expect("checked nonempty ASCII DNS name");
    !last.bytes().all(|b|b.is_ascii_digit())&&
        !last.strip_prefix("0x").is_some_and(|n|n.bytes().all(|b|b.is_ascii_hexdigit()))
}
fn bad()->io::Error{invalid("invalid approved Web HTTPS origin or proxy key digest")}
impl WebAccess{
    pub(crate) fn external(origin:&str,verifier:[u8;32])->io::Result<Self>{
        let a=origin.strip_prefix("https://").filter(|s|!s.is_empty()&&origin.len()<=MAX_HTTPS_ORIGIN).ok_or_else(bad)?;
        let port_ok=|s:&str|s.parse::<u16>().is_ok_and(|v|v!=0&&v!=443&&v.to_string()==s);
        let ok=if let Some(rest)=a.strip_prefix('['){
            // Mapped IPv6 uses different Rust and browser serializations.
            rest.split_once(']').is_some_and(|(ip,suffix)|ip.parse::<Ipv6Addr>().is_ok_and(|v|
                v.to_ipv4_mapped().is_none()&&v.to_string()==ip&&!v.is_unspecified()&&!v.is_multicast())&&
                (suffix.is_empty()||suffix.strip_prefix(':').is_some_and(port_ok)))
        }else{
            let(host,port)=a.split_once(':').map_or((a,None),|(h,p)|(h,Some(p)));
            browser_host(host)&&port.is_none_or(port_ok)
        };
        if verifier==[0;32]||!ok{return Err(bad());}
        Ok(Self::External(ExternalHttps{origin:origin.to_owned(),verifier}))
    }
    pub(crate) fn external_confirmation(origin:&str,digest_text:&str)->io::Result<Self>{
        Self::external(origin,verifier(digest_text.strip_prefix("sha256:").ok_or_else(bad)?).ok_or_else(bad)?)
    }
    pub(crate) fn external_fields(&self)->Option<(&str,[u8;32])>{
        match self{Self::Direct=>None,Self::External(p)=>Some((&p.origin,p.verifier))}
    }
    /// For the stream parser's first pre-read check. The actual production
    /// caller supplies getsockname/getpeername of the accepted original FD.
    pub(super) fn permits_socket(&self,local:SocketAddr,peer:IpAddr)->bool{
        let loopback=IpAddr::V4(Ipv4Addr::LOCALHOST);
        match self{
            Self::Direct=>!local.ip().is_unspecified()&&!local.ip().is_multicast()&&local.port()!=0,
            Self::External(_)=>local.ip()==loopback&&peer==loopback&&local.port()!=0,
        }
    }
    /// Only after permits_socket and the single-valued header check. Never
    /// move the raw, header-buffer-borrowed hop key into a business Request.
    pub(super) fn audience(&self,local:SocketAddr,host:&str,hop_key:Option<&str>)->Option<Audience>{
        match self{
            Self::External(p)=>{
                let key=hop_key?.as_bytes();
                if host!=p.origin.strip_prefix("https://")?||key.len()!=64||!key.iter().all(|&b|hex(b).is_some())||
                    !same_verifier(crate::hash::digest(key).ok()?,p.verifier){return None;}
                Some(Audience{origin:p.origin.clone(),secure:true})
            }
            Self::Direct=>{
                if hop_key.is_some(){return None;}
                let actual=if local.port()!=80{local.to_string()}else{match local.ip(){
                    IpAddr::V4(ip)=>ip.to_string(),IpAddr::V6(ip)=>format!("[{ip}]")}};
                if host!=actual{return None;}
                Some(Audience{origin:format!("http://{actual}"),secure:false})
            }
        }
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn approved_origin_and_hop_verifier_are_bounded_and_canonical(){
        for yes in ["https://admin.example","https://admin.example:8444","https://192.0.2.1",
            "https://127.0.0.1:8444","https://[2001:db8::1]","https://name.0xgg"]{
            assert!(WebAccess::external(yes,[1;32]).is_ok());
        }
        for no in ["http://admin.example","https://Admin.example","https://admin.example:443","https://admin.example:0444",
            "https://a..example","https://*.example","https://user@admin.example","https://admin.example/",
            "https://admin.example?","https://admin.example#","https://[2001:0db8::1]","https:// [2001:db8::1]",
            "https://admin.example\n","https://127.1","https://0177.0.0.1","https://0x7f000001","https://2130706433",
            "https://192.0.002.1","https://192.0.2.1.","https://1.2.3.09","https://admin.09","https://admin.0x",
            "https://admin.0xf","https://[::ffff:192.0.2.1]","https://[::ffff:c000:201]","https://xn--example.test",
            "https://0.0.0.0","https://224.0.0.1","https://255.255.255.255"]{
            assert!(WebAccess::external(no,[1;32]).is_err());
        }
        let origin="https://admin.example";
        assert!(WebAccess::external_confirmation(origin,&format!("sha256:{}","54".repeat(32))).is_ok());
        for no in ["54".repeat(32),format!("sha256:{}","00".repeat(32)),format!("sha256:{}","AB".repeat(32)),
            format!("sha256:{}","54".repeat(31)),format!("sha256:{} ","54".repeat(32))]{
            assert!(WebAccess::external_confirmation(origin,&no).is_err());
        }
        for i in 0..32{let key=[19;32];let mut wrong=key;wrong[i]^=1;assert!(!same_verifier(key,wrong));}
    }
}
