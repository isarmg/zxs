//! One portable SYSTEM decision stream, counted strictly after GENESIS. A
//! digest or a structurally valid first-account frame is not a login/grant.
use super::*;
use super::accounts;

const MAGIC:&[u8]=b"ZXS-PORTABLE-1/DECISION\0";
const FIXED:usize=MAGIC.len()+4+16+8+32+1+16+32+32;
const MAX_FRAME:usize=4096;
pub(super) const MAX_JOURNAL:usize=crate::ordinary::journal::MAX_JOURNAL;
const MAX_DECISIONS:usize=crate::ordinary::journal::MAX_DECISIONS;
pub(super) fn request(domain:&[u8],pool:PoolId,op:OperationId,public:&[u8])->io::Result<[u8;32]>{
    let mut h=hash::Sha256::new();
    for p in [domain,&pool.0,&op.0,public]{h.update(p)?;}Ok(h.finish())
}

pub(super) struct Envelope<'a>{
    seq:u64, previous:[u8;32], kind:u8, op:OperationId, request:[u8;32], body:&'a[u8], digest:[u8;32],
}
impl<'a> Envelope<'a>{
    fn next(pool:PoolId,input:&mut &'a[u8])->io::Result<Self>{
        let prefix=(*input).strip_prefix(MAGIC).ok_or_else(||invalid("unknown portable decision family"))?;
        let mut len_field=prefix;
        let n=u32::from_be_bytes(fixed(&mut len_field)?) as usize;
        if !(FIXED..=MAX_FRAME).contains(&n){return Err(invalid("unbounded or short portable decision"));}
        let frame=take(input,n)?;
        let (head,end)=frame.split_at(n-32);
        let digest=hash::digest(head)?;
        if end!=digest.as_slice(){return Err(invalid("portable decision digest differs from actual frame"));}
        let mut x=&head[MAGIC.len()+4..];
        if PoolId(id(&mut x)?)!=pool{return Err(invalid("portable decision belongs to another pool"));}
        Ok(Self{seq:u64::from_be_bytes(fixed(&mut x)?),previous:fixed(&mut x)?,kind:fixed::<1>(&mut x)?[0],
            op:OperationId(id(&mut x)?),request:fixed(&mut x)?,body:x,digest})
    }
}

#[derive(Clone,Copy,Eq,PartialEq)]
pub(super) struct Head{sequence:u64,bytes:u64,digest:[u8;32]}
impl Head{pub(super) fn sequence(self)->u64{self.sequence}}

pub(super) struct Decisions{
    pub(super) accounts:accounts::Accounts,pub(super) shares:super::shares::Shares,pub(super) files:super::files::Files,pub(super) head:Head,
}
impl Decisions{
    /// Admit only complete, understood, contiguous decisions at the actual
    /// matching floor. Until original INTENT recovery is implemented, any
    /// remaining bytes are not safely usable as an older checkpoint.
    pub(super) fn read(g:&Genesis,tail:&[u8],floor:&[u8])->io::Result<Self>{
        Self::extending(g,tail,floor,None)
    }
    pub(super) fn extending(g:&Genesis,tail:&[u8],floor:&[u8],prior:Option<Head>)->io::Result<Self>{
        if tail.len()>MAX_JOURNAL||floor.len()!=FLOOR_SIZE{return Err(invalid("portable cut exceeds the frame or floor budget"));}
        let mut f=floor.strip_prefix(FLOOR).ok_or_else(||invalid("unknown portable floor"))?;
        if fixed::<32>(&mut f)?!=g.digest{return Err(invalid("floor selects another original register"));}
        let seq=u64::from_be_bytes(fixed(&mut f)?);
        let frame_bytes=u64::from_be_bytes(fixed(&mut f)?);
        let to=fixed::<32>(&mut f)?;
        if !f.is_empty()||seq>MAX_DECISIONS as u64||frame_bytes!=tail.len() as u64{return Err(invalid("incomplete or unconfirmed portable register cut"));}
        let mut rest=tail;let mut prev=g.digest;
        let mut head=Head{sequence:0,bytes:0,digest:g.digest};
        let mut found=prior.is_none_or(|p|p==head);
        let mut state=Self{accounts:Default::default(),shares:Default::default(),files:Default::default(),head};
        let mut used=std::collections::BTreeSet::new();
        used.insert(g.op);
        for expected in 1..=seq{
            let e=Envelope::next(g.pool,&mut rest)?;
            if e.seq!=expected||e.previous!=prev||!used.insert(e.op){return Err(invalid("wrong original predecessor, operation or sequence"));}
            match e.kind{
                accounts::FIRST if expected==1=>state.accounts.apply(g,e.kind,e.op,e.request,e.body,e.digest)?,
                accounts::ADD=>state.accounts.apply(g,e.kind,e.op,e.request,e.body,e.digest)?,
                super::shares::PUBLISH|super::shares::GRANT=>{
                    state.shares.apply(g,&state.accounts,e.kind,e.op,e.request,e.body,e.digest)?;
                }
                super::files::CREATE=>state.files.apply(g,&state.accounts,&state.shares,e.op,e.request,e.body,e.digest,e.seq,&mut used)?,
                _=>return Err(invalid("unsupported portable decision or repeated first administrator")),
            }
            prev=e.digest;head=Head{sequence:expected,bytes:(tail.len()-rest.len()) as u64,digest:prev};found|=prior==Some(head);
        }
        if !found||!rest.is_empty()||to!=prev{return Err(invalid("portable floor does not select this full prefix"));}
        state.head=head;Ok(state)
    }
}
