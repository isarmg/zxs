//! Fold root membership once, by ordinary parent, not once per share. A
//! CREATE is authorized at its original cut; today's observer uses its own
//! current grant rather than inheriting the creator's permissions.
use super::{accounts::Accounts,decision::request,shares::{ShareUse,Shares},Genesis,fixed,id,take};
use crate::{BranchId,EntryId,FileId,HistoryId,OperationId,Role,VolumeId,fs::invalid,
    identity::{AccountId,ShareId},ordinary::portable_birth::Birth};
use std::{collections::{BTreeMap,BTreeSet},ffi::CStr,io,sync::Arc};
use crate::{fs::{DIRECTORY_BYTES,DIRECTORY_NAMES},pool::portable_claim::ROOT_NAME};

pub(super) const CREATE:u8=13;
const DOMAIN:&[u8]=b"ZXS-PORTABLE-1/CREATE-ROOT-FILE\0";
#[derive(Clone,Copy)]
struct Usage{names:usize,bytes:usize}
impl Usage{
    fn plus(self,n:&CStr)->io::Result<Self>{
        let names=self.names.checked_add(1).filter(|v|*v<=DIRECTORY_NAMES)
            .ok_or_else(||invalid("portable root exceeds the actual directory entry limit"))?;
        let bytes=self.bytes.checked_add(1+n.to_bytes().len()).filter(|v|*v<=DIRECTORY_BYTES)
            .ok_or_else(||invalid("portable root exceeds the actual directory byte limit"))?;
        Ok(Self{names,bytes})
    }
}
struct Root{members:BTreeMap<String,Arc<Birth>>,required:Usage}
impl Default for Root{
    fn default()->Self{Self{members:BTreeMap::new(),required:Usage{names:1,bytes:1+ROOT_NAME.to_bytes().len()}}}
}
#[derive(Default)]
pub(super) struct Files{
    roots:BTreeMap<VolumeId,Root>,
    entries:BTreeSet<EntryId>,files:BTreeSet<FileId>,histories:BTreeSet<HistoryId>,branches:BTreeSet<BranchId>,
}
impl Files{
    pub(super) fn at(&self,volume:VolumeId)->impl Iterator<Item=&Arc<Birth>>{
        self.roots.get(&volume).into_iter().flat_map(|r|r.members.values())
    }
    pub(super) fn get(&self,volume:VolumeId,name:&str)->Option<&Arc<Birth>>{self.roots.get(&volume)?.members.get(name)}
    pub(super) fn apply(&mut self,g:&Genesis,accounts:&Accounts,shares:&Shares,op:OperationId,original:[u8;32],
        bytes:&[u8],d:[u8;32],seq:u64,used:&mut BTreeSet<OperationId>)->io::Result<()>{
        let mut b=bytes;
        let actor=AccountId(id(&mut b)?);let cred=fixed(&mut b)?;
        let share=ShareId(id(&mut b)?);let publication=fixed(&mut b)?;let grant=fixed(&mut b)?;
        if fixed::<1>(&mut b)?!=[0]{return Err(invalid("unverified portable FILE parent kind"));}
        let vol=VolumeId(id(&mut b)?);let n=usize::from(fixed::<1>(&mut b)?[0]);
        let name=std::str::from_utf8(take(&mut b,n)?).map_err(|_|invalid("invalid original FILE name"))?.to_owned();
        let inputs=bytes.len()-b.len();
        if request(DOMAIN,g.pool,op,&bytes[..inputs])?!=original{return Err(invalid("FILE creation differs from its original request"));}
        let account=accounts.get(actor).filter(|a|a.decision==cred).ok_or_else(||invalid("unknown original FILE creator credential"))?;
        let scope=shares.selected(g.pool,account,share,ShareUse::Create)?;
        if !scope.matches(vol,publication,grant){return Err(invalid("FILE creation does not match its then-current direct grant"));}
        let e=Birth{pool:g.pool,volume:vol,operation:op,entry:EntryId(id(&mut b)?),file:FileId(id(&mut b)?),
            history:HistoryId(id(&mut b)?),branch:BranchId(id(&mut b)?),ticket:OperationId(id(&mut b)?),
            slot:fixed::<1>(&mut b)?[0],name,decision:d,sequence:seq};
        if !b.is_empty(){return Err(invalid("trailing FILE birth fields"));}
        let ordinary=g.rows.iter().find(|r|r.volume==vol&&r.role==Role::File).ok_or_else(||invalid("not an original ordinary root"))?;
        let recycle=&g.rows[1];e.validate_geometry((&ordinary.path,ordinary.limit),(&recycle.path,recycle.limit),ROOT_NAME)?;
        if !used.insert(e.ticket)||self.get(vol,&e.name).is_some()||!self.entries.insert(e.entry)||!self.files.insert(e.file)||
            !self.histories.insert(e.history)||!self.branches.insert(e.branch){return Err(invalid("occupied portable FILE identity, ticket or parent name"));}
        let root=self.roots.entry(vol).or_default();
        let previous=root.required;let with_witness=previous.plus(&e.witness()?)?;
        let terminal=e.container()?;let stage=e.lower(e.slot)?;let required=with_witness.plus(&terminal)?;
        // Existing lower-slot leftovers are not forever mandatory. But
        // this original stage, each of its skipped slots, and the before/
        // after terminal states had to fit even at their lower bound.
        let mut peak=if terminal.to_bytes().len()>=stage.to_bytes().len(){required}else{with_witness.plus(&stage)?};
        for i in 0..e.slot{
            let n=e.lower(i)?;
            if n.as_c_str()!=ROOT_NAME{peak=peak.plus(&n)?;}
        }
        root.required=required;root.members.insert(e.name.clone(),Arc::new(e));Ok(())
    }
}
