//! One portable SYSTEM decision stream, counted strictly after GENESIS. A
//! digest or a structurally valid first-account frame is not a login/grant.
use super::*;
use crate::ordinary::first_account::First;

const MAGIC:&[u8]=b"ZXS-PORTABLE-1/DECISION\0";
const FIXED:usize=MAGIC.len()+4+16+8+32+1+16+32+32;
const MAX_FRAME:usize=4096;
pub(super) const MAX_JOURNAL:usize=crate::ordinary::journal::MAX_JOURNAL;
const MAX_DECISIONS:usize=crate::ordinary::journal::MAX_DECISIONS;
// The original AccountId/operator_ref/login/salt/tag fields are identical
// to MAINT's fixed First. The original Op is already in the new envelope.
const FIRST_TAG:u8=9;

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

pub(super) struct Decisions{
    pub(super) first:Option<First>,
}
impl Decisions{
    /// Admit only complete, understood, contiguous decisions at the actual
    /// matching floor. Until original INTENT recovery is implemented, any
    /// remaining bytes are not safely usable as an older checkpoint.
    pub(super) fn read(g:&Genesis,tail:&[u8],floor:&[u8])->io::Result<Self>{
        if tail.len()>MAX_JOURNAL||floor.len()!=FLOOR_SIZE{return Err(invalid("portable cut exceeds the frame or floor budget"));}
        let mut f=floor.strip_prefix(FLOOR).ok_or_else(||invalid("unknown portable floor"))?;
        if fixed::<32>(&mut f)?!=g.digest{return Err(invalid("floor selects another original register"));}
        let seq=u64::from_be_bytes(fixed(&mut f)?);
        let frame_bytes=u64::from_be_bytes(fixed(&mut f)?);
        let to=fixed::<32>(&mut f)?;
        if !f.is_empty()||seq>MAX_DECISIONS as u64||frame_bytes!=tail.len() as u64{return Err(invalid("incomplete or unconfirmed portable register cut"));}
        let mut rest=tail;let mut prev=g.digest;
        let mut state=Self{first:None};
        for expected in 1..=seq{
            let e=Envelope::next(g.pool,&mut rest)?;
            if e.seq!=expected||e.previous!=prev||e.op==g.op{return Err(invalid("wrong original predecessor, operation or sequence"));}
            match e.kind{
                FIRST_TAG if expected==1=>{
                    state.first=Some(First::original_terms(g.pool,g.op,e.op,e.request,e.digest,e.body)?);
                }
                _=>return Err(invalid("unsupported portable decision or repeated first administrator")),
            }
            prev=e.digest;
        }
        if !rest.is_empty()||to!=prev{return Err(invalid("portable floor does not select this full prefix"));}
        Ok(state)
    }
}
