//! Read the original ordinary-root share and its separately decided data
//! rights. Management of an account or publication of a share grants no
//! FILE rights. These values never open a filesystem or perform a write.
use super::{credential::Account,Genesis,Row,fixed,id,accounts::{Accounts,Management},decision::request};
use crate::{OperationId,Role,VolumeId,PoolId,identity::AccountId,fs::invalid};
use std::{collections::BTreeMap,io};

pub(in crate::pool) use crate::identity::ShareId;
pub(super) const PUBLISH:u8=10;
pub(super) const GRANT:u8=11;
const PUBLISH_DOMAIN:&[u8]=b"ZXS-PORTABLE-1/PUBLISH-ROOT-SHARE\0";
const GRANT_DOMAIN:&[u8]=b"ZXS-PORTABLE-1/ROOT-ACCESS-GRANT\0";
const WRITE_BITS:u8=8|16; // create, save
const KNOWN:u8=1|2|4|WRITE_BITS;

#[derive(Clone,Copy,PartialEq,Eq)]
pub(in crate::pool) enum ShareUse{List,Current,Published,Create,Save}
impl ShareUse {
    fn mask(self)->u8{match self{Self::List=>1,Self::Current=>1|2,Self::Published=>1|4,Self::Create=>1|8,Self::Save=>1|16}}
}
#[derive(Clone,Copy)]
struct SharedRoot {volume:VolumeId,decision:[u8;32]}
#[derive(Clone,Copy)]
struct Rights {share:[u8;32],bits:u8,decision:[u8;32]}
/// A non-secret fact for a particular authorized open. Private fields stop
/// other crate modules manufacturing a check from an ID or a previous bool.
#[derive(Clone,Copy,Eq,PartialEq)]
pub(super) struct Selection{
    pool:PoolId,account:AccountId,credential:[u8;32],share:ShareId,published:[u8;32],
    volume:VolumeId,grant:[u8;32],action:ShareUse,
}
impl Selection{
    pub(super) fn id(&self)->ShareId{self.share}
    pub(super) fn action(&self)->ShareUse{self.action}
    pub(super) fn volume(&self)->VolumeId{self.volume}
}
#[derive(Default)]
pub(super) struct Shares{
    published:BTreeMap<ShareId,SharedRoot>,
    rights:BTreeMap<(ShareId,AccountId),Rights>,
}
fn root<'a>(g:&'a Genesis,b:&mut &[u8])->io::Result<&'a Row>{
    if fixed::<1>(b)?!=[0]{return Err(invalid("not a supported portable root share endpoint"));}
    let v=VolumeId(id(b)?);
    g.rows.iter().find(|r|r.role==Role::File&&r.volume==v).ok_or_else(||invalid("share endpoint is not an original ordinary volume"))
}
impl Shares{
    pub(super) fn apply(&mut self,g:&Genesis,accounts:&Accounts,kind:u8,op:OperationId,original:[u8;32],
        body:&[u8],decision:[u8;32])->io::Result<()>{
        let domain=match kind{PUBLISH=>PUBLISH_DOMAIN,GRANT=>GRANT_DOMAIN,_=>return Err(invalid("unknown share decision"))};
        if request(domain,g.pool,op,body)?!=original{return Err(invalid("share decision differs from original request"));}
        let mut b=body;
        let scope=accounts.management(&mut b)?;
        let share=ShareId(id(&mut b)?);
        let volume=root(g,&mut b)?.volume;
        if kind==PUBLISH{
            if !b.is_empty()||self.published.contains_key(&share){return Err(invalid("conflicting publication or trailing share fields"));}
            self.published.insert(share,SharedRoot{volume,decision});return Ok(());
        }
        let published=fixed::<32>(&mut b)?;
        let current=self.published.get(&share).filter(|s|s.volume==volume&&s.decision==published)
            .ok_or_else(||invalid("grant does not name that exact published ordinary root"))?;
        let to=AccountId(id(&mut b)?);
        if accounts.get(to).is_none(){return Err(invalid("no such account in the admitted original prefix"));}
        let previous=match fixed::<1>(&mut b)?{
            [0]=>None,[1]=>Some(fixed::<32>(&mut b)?),_=>return Err(invalid("unknown grant predecessor tag")),
        };
        let bits=fixed::<1>(&mut b)?[0];
        let last=self.rights.get(&(share,to));
        let changed=bits^last.map_or(0,|r|r.bits);
        if !b.is_empty()||bits & !KNOWN!=0||(bits!=0&&bits&1==0)||last.map(|r|r.decision)!=previous||changed==0||
            (changed&WRITE_BITS!=0&&scope!=Management::ReadAndWriteGrants){
            return Err(invalid("invalid or unauthorized ACL change"));}
        // Persist a zero-rights tombstone with its real decision when
        // replaying: regrant must reference the revocation, not old rights.
        self.rights.insert((share,to),Rights{share:current.decision,bits,decision});Ok(())
    }
    pub(super) fn selected(&self,pool:PoolId,subject:&Account,share:ShareId,action:ShareUse)->io::Result<Selection>{
        let denied=||io::Error::new(io::ErrorKind::PermissionDenied,"not authorized");
        let p=self.published.get(&share).ok_or_else(denied)?;
        let r=self.rights.get(&(share,subject.account)).filter(|r|r.share==p.decision&&r.bits&action.mask()==action.mask()).ok_or_else(denied)?;
        Ok(Selection{pool,account:subject.account,credential:subject.decision,share,published:p.decision,
            volume:p.volume,grant:r.decision,action})
    }
}
