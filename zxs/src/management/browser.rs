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
#[path = "../../tests/unit/attached/management/browser/tests.rs"]
mod tests;
