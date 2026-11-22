//! High side of the actual S-only probe. The only constructor of a
//! VerifiedPreflight completes the original low process and its fd receipt.
use super::{Fault,Parent,Result,Role,foreground,view::{Kind,Source,View}};
use crate::{smb::launch::{low::{PreflightQuestion,PublicReply,QUESTION_MAX,QuestionSource,REPLY_MAX},
    site::{Binding,SiteLease},transport::{read_sealed,seal_public}},
    system::reader::smb::runtime::PreflightSelection};
use std::{io,mem::size_of,time::{Duration,Instant}};

fn no()->io::Error{io::Error::new(io::ErrorKind::PermissionDenied,"the original read-only SMB startup preflight did not complete")}

// This borrows the actual site/lease, not just its bytes, owner, path or
// public preflight fields. A different cold view cannot adopt the receipt.
#[must_use]
pub(in crate::smb::launch) struct VerifiedPreflight<'s,'p>{
    site:&'s SiteLease<'p>,
    binding:Binding,
    question:PreflightQuestion,
    original:PublicReply,
}
impl<'s,'p> VerifiedPreflight<'s,'p>{
    pub(super) fn execute(parent:&Parent,site:&'s SiteLease<'p>)->io::Result<Self>{
        foreground::require().map_err(|_|no())?;
        let (path,system,where_)=site.system()?;
        let subject=site.subject();
        let binding=site.binding();
        let question=PreflightQuestion::encode(QuestionSource{
            nonce:nonce().map_err(|_|no())?,release:site.image_sha(),installed:site.install_sha(),
            init_actor:subject.init_actor,uid:subject.uid,gid:subject.gid,entity:where_.entity,
            system:path,install:site.plan_bytes(),
        }).map_err(|_|no())?;
        let sealed=seal_public(question.bytes(),QUESTION_MAX).map_err(|_|no())?;
        let view=View::prepare(parent,vec![Source{path,original:system,kind:Kind::System}],Role::Preflight).map_err(|_|no())?;
        site.require()?;foreground::require().map_err(|_|no())?;
        let born=parent.prepare().and_then(|p|p.birth(site.image().map_err(|_|Fault::Handoff)?,
            &view,(subject.uid,subject.gid),Role::Preflight)).map_err(|_|no())?;
        let until=Instant::now()+Duration::from_secs(10);
        let (frame,body)=born.preflight(&question.start(),&sealed,until).map_err(|_|no())?;
        let bytes=read_sealed(&body,REPLY_MAX).map_err(|_|no())?;
        let original=PublicReply::decode_verified(&question,&frame,&bytes).map_err(|_|no())?;
        let this=Self{site,binding,question,original};
        // The original worker is already reaped. Its moved view was
        // consumed and may be unmounted by the exiting namespace; it must
        // never be reclassified as a new prepared view. Recheck the
        // original host and retained named S/site, not departed mounts.
        parent.require().map_err(|_|no())?;
        this.qualified_for(site)?;
        Ok(this)
    }
    pub(in crate::smb::launch) fn qualified_for(&self,site:&SiteLease<'p>)->io::Result<&PreflightSelection>{
        let origin=site.origin();let actual=self.original.facts();
        if !std::ptr::eq(self.site,site)||self.binding!=site.binding()||
            actual.origin!=[origin.pool,origin.init,origin.instance,origin.bind,origin.setup]||
            actual.install_sha!=site.install_sha()||self.question.release()!=site.image_sha(){
            return Err(no());
        }
        foreground::require().map_err(|_|no())?;self.site.require()?;
        Ok(actual)
    }
    pub(in crate::smb::launch) fn plan(&self)->&PreflightQuestion{&self.question}
}
pub(super) fn nonce()->Result<[u8;32]>{
    let mut random=[0u8;32];
    // SAFETY: write only into one bounded initialized local span. No
    // substituted entropy, path, process attribute or environment input.
    let count=unsafe{libc::getrandom(random.as_mut_ptr().cast(),size_of::<[u8;32]>(),libc::GRND_NONBLOCK)};
    if count!=random.len() as isize||random==[0;32]{return Err(Fault::Resources);}
    Ok(random)
}
