//! Physically selected ordinary work rounds. A checksum/map in a reserved
//! tail isn't visible just because its two names exist. SQL must supply the
//! exact selected Record and the greatest physically permitted extent.
//! Candidate readback is a separate type, never a read authority.
use super::{
    Index,Material,Leaf,
    blocks::{Buffers,Ends,Ref,Segment},
    local::Located,
    stored::{WorkDescriptor,WorkKey},
    work::Spec,
};
use crate::{FileId,OperationId,PoolId,fs::{Entity,HeldBytes,HeldDir,LiveStamp,WitnessRead,invalid},hash::Sha256,ordinary::output::CheckedOutput};
use std::{collections::BTreeMap,ffi::CString,io::{self,Write}};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct End {
    pub(crate) data: u64,
    pub(crate) checks: u64,
}
impl End {
    pub(crate) const ZERO:Self=Self{data:0,checks:0};
    fn covers(self, another:Self)->bool {self.data>=another.data && self.checks>=another.checks}
    fn block(self)->Ends {Ends{raw:self.data,checks:self.checks}}
    fn require(self)->io::Result<()>{
        if self.data>i64::MAX as u64 || self.checks>i64::MAX as u64 {return Err(invalid("work endpoints exceed native offsets"));}
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Key {
    pub(crate) pool: PoolId,
    pub(crate) file: FileId,
    pub(crate) round: OperationId,
    pub(crate) save: OperationId,
    pub(crate) revision: u64,
}
impl Key {
    fn codec(self)->WorkKey{
        WorkKey{pool:self.pool,file:self.file,round:self.round,save:self.save,revision:self.revision}
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Map {
    pub(crate) at: u64,
    pub(crate) bytes: u64,
    pub(crate) sha: [u8;32],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Record {
    key:Key,
    source:Ref,
    start:End,
    end:End,
    packed_sha:[u8;32],
    map:Option<(u64,WorkDescriptor)>,
}
impl Record {
    pub(crate) fn first(spec:Spec)->io::Result<Self>{
        let key=Key{pool:spec.pool,file:spec.file,round:spec.op,save:spec.op,revision:1};
        let source=Ref::declared(key.pool,Material::declared(key.file,key.save,0,spec.bytes)?,spec.directory_sha)?;
        Ok(Self{key,source,start:End::ZERO,end:End{data:spec.bytes,checks:source.table_len()?},packed_sha:spec.content_sha,map:None})
    }
    pub(crate) fn edit(key:Key,start:End,end:End,logical:u64,packed_sha:[u8;32],directory_sha:[u8;32],map:Map)->io::Result<Self>{
        start.require()?;end.require()?;
        if !end.covers(start) || end.data-start.data>logical || key.revision==1 && start!=End::ZERO {
            return Err(invalid("wrong endpoints for the actual work revision"));
        }
        let source=Ref::declared(key.pool,Material::declared(key.file,key.save,0,end.data-start.data)?,directory_sha)?;
        let d=WorkDescriptor::declared(key.codec(),logical,map.bytes,map.sha)?;
        if start.checks.checked_add(source.table_len()?)!=Some(map.at) || map.at.checked_add(map.bytes)!=Some(end.checks) {
            return Err(invalid("logical map and material do not fill their exact work window"));
        }
        Ok(Self{key,source,start,end,packed_sha,map:Some((map.at,d))})
    }
    pub(crate) fn key(self)->Key {self.key}
    pub(crate) fn start(self)->End {self.start}
    pub(crate) fn end(self)->End {self.end}
    pub(crate) fn logical(self)->u64 {self.map.map_or(self.source.material.size(),|(_,d)|d.logical_bytes())}
    pub(crate) fn packed_sha(self)->[u8;32] {self.packed_sha}
    pub(crate) fn directory_sha(self)->[u8;32] {self.source.directory_sha()}
    pub(crate) fn map(self)->Option<Map>{self.map.map(|(at,d)|Map{at,bytes:d.index_bytes(),sha:d.index_sha()})}

    fn index(self,checks:&HeldBytes)->io::Result<Index>{
        match self.map{
            Some((at,d))=>d.read_selected(checks,at,self.end.checks),
            None=>Index::full(self.source.material),
        }
    }
}
pub(crate) fn names(round:OperationId)->[CString;4]{Located::change_names(round,0)}

struct Pair {
    data:HeldBytes,
    checks:HeldBytes,
    names:[CString;4],
    dir:Entity,
    key:Key,
}
impl Pair{
    fn open(dir:&HeldDir,key:Key,end:End,prefix:Option<End>)->io::Result<Self>{
        let scope=WitnessRead::at(dir)?;
        Self::open_at(dir,&scope,key,end,prefix)
    }
    fn open_at(dir:&HeldDir,scope:&WitnessRead<'_>,key:Key,end:End,prefix:Option<End>)->io::Result<Self>{
        end.require()?;
        let names=names(key.round);
        let (data,checks)=match prefix{
            Some(limit)=>{
                limit.require()?;
                if !limit.covers(end){return Err(invalid("physical work ceiling does not cover the confirmed cut"));}
                (scope.open_work_prefix(&names[0],&names[1],end.data,limit.data)?,
                 scope.open_work_prefix(&names[2],&names[3],end.checks,limit.checks)?)
            }
            None=>(scope.open(&names[0],&names[1],end.data)?,scope.open(&names[2],&names[3],end.checks)?),
        };
        if data.stamp().same(checks.stamp()){return Err(invalid("work DATA and checks must be separate ordinary files"));}
        let selected=Self{data,checks,names,dir:dir.entity,key};
        selected.require(dir)?;Ok(selected)
    }
    fn require(&self,parent:&HeldDir)->io::Result<()>{
        if parent.entity!=self.dir||!parent.shares_file_mount(self.data.file())?||!parent.shares_file_mount(self.checks.file())?{
            return Err(invalid("wrong actual work parent or mount"));
        }
        WitnessRead::require_parent(parent)?;
        self.data.require_name(parent,&self.names[0])?;self.data.require_name(parent,&self.names[1])?;
        self.checks.require_name(parent,&self.names[2])?;self.checks.require_name(parent,&self.names[3])
    }
    fn segment(&self,member:Record,allowed:End)->io::Result<Segment>{
        let k=member.key;
        if (k.pool,k.file,k.round)!=(self.key.pool,self.key.file,self.key.round) || !allowed.covers(member.end) {
            return Err(invalid("material is not in this authorized original work round and cut"));
        }
        // Map bytes, the following segment and raw EOF are not part of this material.
        Segment::open(member.source,&self.data,&self.checks,member.start.block(),
                      End{data:member.end.data,checks:member.map.map_or(member.end.checks,|(at,_)|at)}.block())
    }
    fn range(&self,dir:&HeldDir,segment:&Segment,at:u64,n:u64,scratch:&mut Buffers,mut out:impl Write)->io::Result<()>{
        self.require(dir)?;segment.range(&self.data,&self.checks,at,n,scratch,&mut out)?;self.require(dir)
    }
}

/// One exact terminal selected in the persisted direct map. This is a
/// request for its original issuance row, not a disk pathname or an alias.
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(crate) struct Need {pub(crate) issuer:OperationId,pub(crate) bytes:u64}

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(crate) struct Origin {pub(crate) record:Record,pub(crate) physical:End}

/// Original archive members touched during this one operation. No archive
/// descriptor is retained per historic round; recheck only the touched
/// two-link names and exact stamps, not their contents.
#[derive(Default)]
pub(crate) struct Used(BTreeMap<[u8;16],Evidence>);
#[derive(Clone,Copy,Eq,PartialEq)]
struct Evidence {key:Key,data:LiveStamp,checks:LiveStamp}
impl Used {
    pub(crate) fn witnessed_rounds(&self)->impl Iterator<Item=(OperationId,LiveStamp,LiveStamp)>+'_ {
        self.0.values().map(|r|(r.key.round,r.data,r.checks))
    }
    pub(crate) fn merge(&mut self,other:Self)->io::Result<()>{
        for (round,e) in other.0{
            if let Some(old)=self.0.get(&round){
                if old.data!=e.data||old.checks!=e.checks{return Err(invalid("used work archive changed across staged ranges"));}
            }else{self.0.insert(round,e);}
        }
        Ok(())
    }
    fn add(&mut self,pair:&Pair)->io::Result<()>{
        let e=Evidence{key:pair.key,data:pair.data.stamp(),checks:pair.checks.stamp()};
        // Keys for equally named pairs may be different material operations,
        // so compare their original source and target stamps, not the save.
        if let Some(old)=self.0.get(&e.key.round.0){
            if old.data!=e.data||old.checks!=e.checks{return Err(invalid("archive changed between selected ranges"));}
        }else{self.0.insert(e.key.round.0,e);}
        Ok(())
    }
    pub(crate) fn require(&self,dir:&HeldDir)->io::Result<()>{
        if self.0.is_empty(){return Ok(());}
        let scope=WitnessRead::at(dir)?;
        for old in self.0.values(){
            let end=End{data:old.data.len,checks:old.checks.len};
            let pair=Pair::open_at(dir,&scope,old.key,end,None)?;
            if pair.data.stamp()!=old.data||pair.checks.stamp()!=old.checks{
                return Err(invalid("a used, exact archive pair changed before delivery"));
            }
        }
        Ok(())
    }
}
struct Reads<'a> {
    scope:Option<WitnessRead<'a>>,
    archive:Option<Pair>,
    used:Used,
}
impl<'a> Reads<'a> {
    fn new()->Self{Self{scope:None,archive:None,used:Used::default()}}
    fn pair<'p>(&'p mut self,dir:&'a HeldDir,held:&'a Selected,part:Origin)->io::Result<(&'p Pair,End)> where 'a:'p{
        let h=held.record.key;
        let r=part.record.key;
        if (r.pool,r.file)!=(h.pool,h.file){return Err(invalid("archive terminal belongs to another original FILE"));}
        if r.round==h.round {
            if r.revision>h.revision || !held.record.end.covers(part.record.end){
                return Err(invalid("work terminal lies past the selected revision"));
            }
            return Ok((&held.pair,held.record.end));
        }
        if self.archive.as_ref().is_none_or(|p|p.key.round!=r.round){
            self.archive=None; // no two archive pairs live across the next open
            if self.scope.is_none(){self.scope=Some(WitnessRead::at(dir)?);}
            let pair=Pair::open_at(dir,self.scope.as_ref().expect("actual FILE census"),r,part.physical,None)?;
            self.used.add(&pair)?;self.archive=Some(pair);
        }
        let pair=self.archive.as_ref().expect("selected archive pair");
        if part.physical.data!=pair.data.stamp().len||part.physical.checks!=pair.checks.stamp().len{
            return Err(invalid("materials disagree about their one original archive extent"));
        }
        Ok((pair,part.physical))
    }
}

/// Opaque immutable map, not an Index that a planner can replace with a
/// draft. Future appends remain outside both of this hold's selected ends.
pub(crate) struct Selected {
    pair:Pair,
    record:Record,
    index:Index,
}
impl Selected {
    pub(crate) fn open(dir:&HeldDir,confirmed:Record,physical:End)->io::Result<Self>{
        Self::open_shape(dir,confirmed,Some(physical))
    }
    pub(crate) fn open_settled(dir:&HeldDir,confirmed:Record)->io::Result<Self>{
        Self::open_shape(dir,confirmed,None)
    }
    fn open_shape(dir:&HeldDir,confirmed:Record,physical:Option<End>)->io::Result<Self>{
        let pair=Pair::open(dir,confirmed.key,confirmed.end,physical)?;
        let index=confirmed.index(&pair.checks)?;
        pair.segment(confirmed,confirmed.end)?; // original material header, including a zero-byte SAVE
        pair.require(dir)?;Ok(Self{pair,record:confirmed,index})
    }
    pub(crate) fn record(&self)->Record{self.record}
    pub(crate) fn require(&self,dir:&HeldDir)->io::Result<()>{self.pair.require(dir)}
    /// Before a new ticket advances physical starts past a prior failed
    /// append, make both exact observed tails durable. The caller holds the
    /// same FILE EX; no read or adoption of their unconfirmed body occurs.
    pub(crate) fn durable_observed_end(&self,dir:&HeldDir)->io::Result<End>{
        let data=&self.pair.data;let checks=&self.pair.checks;
        let check=||->io::Result<()>{
            self.require(dir)?;
            data.stamp().require(data.file())?;checks.stamp().require(checks.file())
        };
        check()?;data.file().sync_all()?;check()?;checks.file().sync_all()?;check()?;
        Ok(End{data:data.stamp().len,checks:checks.stamp().len})
    }

    pub(crate) fn needs(&self,at:u64,n:u64)->io::Result<Vec<Need>> {
        let mut list=BTreeMap::new();
        self.index.visit(at,n,|leaf,_,_|{
            if let Leaf::Data(m)=leaf {
                let id=m.key().0;
                list.insert(id.0,Need{issuer:id,bytes:m.size()});
            }
            Ok(())
        })?;
        Ok(list.into_values().collect())
    }
    fn match_origins(&self,needs:&[Need],origins:&[Origin])->io::Result<()>{
        if needs.len()!=origins.len(){return Err(invalid("incomplete selected work material origins"));}
        for (need,origin) in needs.iter().zip(origins){
            let r=origin.record;let k=r.key();
            if k.pool!=self.record.key.pool||k.file!=self.record.key.file||k.save!=need.issuer||
                r.source.material.size()!=need.bytes || !origin.physical.covers(r.end) ||
                (k.save==self.record.key.save&&r!=self.record){
                return Err(invalid("working map terminal disagrees with the exact issued material"));
            }
        }
        Ok(())
    }
    /// Resolve each overlapping material once, then work on the real pairs
    /// without holding a SQLite transaction through physical reads.
    pub(crate) fn range_with(&self,dir:&HeldDir,at:u64,n:u64,
        resolve:impl FnOnce(&[Need])->io::Result<Vec<Origin>>,check:impl FnMut()->io::Result<()>,out:impl Write)->io::Result<Used>{
        self.require(dir)?;
        let needs=self.needs(at,n)?;
        let origins=resolve(&needs)?;
        self.match_origins(&needs,&origins)?;
        let (page,block)=scratch_for(needs.iter().map(|n|n.bytes).max().unwrap_or(0));
        let mut scratch=Buffers::new(page,block)?;
        let mut out=CheckedOutput{out,check};
        let mut access=Reads::new();let mut segment=None;
        self.index.visit(at,n,|leaf,off,len|match leaf{
            Leaf::Zero=>{
                let mut left=len;let zero=[0u8;65536];
                while left!=0{
                    let count=left.min(zero.len() as u64) as usize;
                    out.write_all(&zero[..count])?;left-=count as u64;
                }
                Ok(())
            },
            Leaf::Data(m)=>{
                let issuer=m.key().0;
                let pos=needs.binary_search_by_key(&issuer.0,|n|n.issuer.0)
                    .map_err(|_|invalid("material was not selected by this work request"))?;
                let origin=origins[pos];
                let (pair,allowed)=access.pair(dir,self,origin)?;
                if segment.as_ref().is_none_or(|(prior,_)|*prior!=m){
                    // Release the prior checksum directory before opening
                    // the next potentially large one.
                    drop(segment.take());
                    segment=Some((m,pair.segment(origin.record,allowed)?));
                }
                pair.range(dir,&segment.as_ref().expect("selected work material").1,off,len,&mut scratch,&mut out)
            },
        })?;
        out.flush()?;
        access.archive=None;access.used.require(dir)?;self.require(dir)?;Ok(access.used)
    }
    /// Used by explicit freezing. Verify every *referenced* source's issued
    /// checksum header/directory, never copy or scan its full payload here.
    pub(crate) fn verify_map(&self,dir:&HeldDir,resolve:impl FnOnce(&[Need])->io::Result<Vec<Origin>>)->io::Result<Used>{
        self.require(dir)?;let needs=self.needs(0,self.record.logical())?;
        let origins=resolve(&needs)?;self.match_origins(&needs,&origins)?;
        let mut access=Reads::new();
        for part in origins{
            let(p,allowed)=access.pair(dir,self,part)?;
            p.segment(part.record,allowed)?;p.require(dir)?;
        }
        access.archive=None;access.used.require(dir)?;self.require(dir)?;Ok(access.used)
    }
}

/// Exact Whole readback of one original pending candidate. The candidate
/// is returned only after independent map and digest validation; unlike a
/// Selected reader this cannot tolerate any late append before SQL.
pub(crate) struct Confirmation{pair:Pair,record:Record}
impl Confirmation{
    pub(crate) fn read_back(dir:&HeldDir,record:Record,data:LiveStamp,checks:LiveStamp)->io::Result<Self>{
        let pair=Pair::open(dir,record.key,record.end,None)?;
        if pair.data.stamp()!=data||pair.checks.stamp()!=checks{return Err(invalid("work verification did not reopen the actual issued pair"));}
        record.index(&pair.checks)?;
        let segment=pair.segment(record,record.end)?;
        let mut out=HashSink(Sha256::new());
        let n=record.source.material.size();let (page,block)=scratch_for(n);
        let mut scratch=Buffers::new(page,block)?;
        pair.range(dir,&segment,0,n,&mut scratch,&mut out)?;
        if out.0.finish()!=record.packed_sha{return Err(invalid("working packed bytes differ from their original request"));}
        pair.require(dir)?;Ok(Self{pair,record})
    }
    pub(crate) fn require(&self,dir:&HeldDir,record:Record)->io::Result<()>{
        if record!=self.record{return Err(invalid("mismatched work candidate"));}
        self.pair.require(dir)
    }
}
fn scratch_for(raw:u64)->(usize,usize){
    let block=crate::DRAFT_CHUNK as u64;
    (32*raw.div_ceil(block).min(super::blocks::PER_PAGE) as usize,raw.min(block) as usize)
}
struct HashSink(Sha256);
impl Write for HashSink{
    fn write(&mut self,b:&[u8])->io::Result<usize>{self.0.update(b)?;Ok(b.len())}
    fn flush(&mut self)->io::Result<()>{Ok(())}
}

/// Authenticated physical transport for one *selected* work round. The
/// caller supplies committed records in issuance order, not current disk
/// lengths. A failed append's bytes are never copied; gaps stay sparse.
/// A target is unpublished until independent named readback completes.
pub(crate) struct TransferRound{
    pair:Pair,last:Record,previous:Option<Record>,
}
impl TransferRound{
    pub(crate) fn open(dir:&HeldDir,scope:&WitnessRead<'_>,last:Record,physical:Option<End>)->io::Result<Self>{
        let pair=Pair::open_at(dir,scope,last.key,last.end,physical)?;
        let hold=Self{pair,last,previous:None};hold.require(dir)?;Ok(hold)
    }
    pub(crate) fn require(&self,dir:&HeldDir)->io::Result<()>{
        self.pair.require(dir)?;
        self.pair.data.stamp().require(self.pair.data.file())?;
        self.pair.checks.stamp().require(self.pair.checks.file())
    }
    pub(crate) fn step(&mut self,dir:&HeldDir,record:Record,target:Option<(&std::fs::File,&std::fs::File)>,
        mut live:impl FnMut()->io::Result<()>)->io::Result<()>{
        use std::os::unix::fs::FileExt;
        let (wanted,prior)=match self.previous{
            None=>(1,End::ZERO),
            Some(r)=>(r.key.revision.checked_add(1).ok_or_else(||invalid("work revision overflow"))?,r.end),
        };
        let k=record.key;let last=self.last.key;
        if (k.pool,k.file,k.round)!=(last.pool,last.file,last.round)||k.revision!=wanted||k.revision>last.revision||
           (k.revision==1&&(k.save!=k.round||record.start!=End::ZERO))||
           (k.revision==last.revision&&record!=self.last)||!record.start.covers(prior)||!self.last.end.covers(record.end){
            return Err(invalid("material does not follow the selected transfer round"));
        }
        if let Some((raw,checks))=target{
            let (a,b)=(LiveStamp::read(raw)?,LiveStamp::read(checks)?);
            if a.same(b)||[self.pair.data.stamp(),self.pair.checks.stamp()].iter().any(|x|x.same(a)||x.same(b))||
                a.len>self.last.end.data||b.len>self.last.end.checks{
                return Err(invalid("transfer target is not the distinct prepared work pair"));
            }
        }
        live()?;self.require(dir)?;
        record.index(&self.pair.checks)?;
        let segment=self.pair.segment(record,self.last.end)?;
        let n=record.source.material.size();
        let (page,block)=scratch_for(n);let mut scratch=Buffers::new(page,block)?;
        struct CopyRaw<'a,F>{out:Option<&'a std::fs::File>,at:u64,sha:Sha256,live:&'a mut F}
        impl<F:FnMut()->io::Result<()>> Write for CopyRaw<'_,F>{
            fn write(&mut self,buf:&[u8])->io::Result<usize>{
                (self.live)()?;self.sha.update(buf)?;
                if let Some(out)=self.out{put_transfer(out,self.at,buf)?;}
                self.at+=buf.len() as u64;Ok(buf.len())
            }
            fn flush(&mut self)->io::Result<()>{Ok(())}
        }
        let mut copy=CopyRaw{out:target.map(|x|x.0),at:record.start.data,sha:Sha256::new(),live:&mut live};
        self.pair.range(dir,&segment,0,n,&mut scratch,&mut copy)?;
        if copy.sha.finish()!=record.packed_sha||copy.at!=record.end.data{
            return Err(invalid("transfer read does not match the committed packed bytes"));
        }
        // The whole exact checksum table was validated above, including its
        // original header, directory and each referenced page. The map was
        // decoded and hashed in the same actual scope. Copy only that window.
        if let Some((_,out))=target{
            let mut at=record.start.checks;let mut buf=[0u8;65536];
            while at<record.end.checks{
                live()?;let n=(record.end.checks-at).min(buf.len() as u64) as usize;
                self.pair.checks.file().read_exact_at(&mut buf[..n],at)?;
                self.pair.checks.stamp().require(self.pair.checks.file())?;
                put_transfer(out,at,&buf[..n])?;at+=n as u64;
            }
        }
        live()?;self.require(dir)?;self.previous=Some(record);Ok(())
    }
    pub(crate) fn require_written(&self,data:&std::fs::File,checks:&std::fs::File)->io::Result<()>{
        self.pair.data.stamp().require(data)?;self.pair.checks.stamp().require(checks)
    }
    pub(crate) fn finish(self,dir:&HeldDir,actual:&mut Used)->io::Result<()>{
        if self.previous!=Some(self.last){return Err(invalid("transfer did not verify all confirmed records of the selected round"));}
        self.require(dir)?;actual.add(&self.pair)
    }
}

/// Write only the absent suffix of this authenticated window. An observed
/// existing prefix is never repaired by overwriting it. The absent gap
/// between confirmed windows remains sparse.
fn put_transfer(out:&std::fs::File,at:u64,bytes:&[u8])->io::Result<()>{
    use std::os::unix::fs::FileExt;
    let stamp=LiveStamp::read(out)?;
    let overlap=stamp.len.saturating_sub(at).min(bytes.len() as u64) as usize;
    let mut copied=0;let mut scratch=[0u8;32768];
    while copied<overlap{
        let n=(overlap-copied).min(scratch.len());
        out.read_exact_at(&mut scratch[..n],at+copied as u64)?;
        if scratch[..n]!=bytes[copied..copied+n]{return Err(invalid("unpublished transfer prefix is not the original material"));}
        copied+=n;
    }
    stamp.require(out)?;
    if overlap<bytes.len(){out.write_all_at(&bytes[overlap..],at+overlap as u64)?;}
    Ok(())
}

/// Effective network edits in logical order. Bytes for all spans are
/// packed contiguously in one supplied, already completed staging FD.
/// `preserved` is the old logical prefix not discarded by truncation;
/// any other unwritten bytes below EOF are holes, not old data.
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(crate) struct Span{pub(crate) at:u64,pub(crate) len:u64}

fn packed_index(base:&Index,material:Material,preserved:u64,eof:u64,spans:&[Span])->io::Result<Index>{
    if spans.len()>super::MAX_ROWS||preserved>base.len().min(eof)||eof>i64::MAX as u64||
       spans.is_empty()&&preserved==base.len()&&eof==base.len(){
        return Err(invalid("invalid or unchanged packed work layout"));
    }
    let mut build=super::Build::new(base.file())?;
    let inherit=|b:&mut super::Build,lo:u64,hi:u64|->io::Result<()>{
        let keep=hi.min(preserved);if lo<keep{b.inherit(base,lo,keep-lo)?;}
        let hole=lo.max(preserved);if hole<hi{b.add(Leaf::Zero,0,hi-hole)?;}
        Ok(())
    };
    let(mut logical,mut packed)=(0,0u64);
    for s in spans{
        if s.at<logical||s.len==0{return Err(invalid("overlapping or empty packed work spans"));}
        let end=s.at.checked_add(s.len).filter(|n|*n<=eof).ok_or_else(||invalid("packed span crosses logical EOF"))?;
        inherit(&mut build,logical,s.at)?;
        build.add(Leaf::Data(material),packed,s.len)?;
        packed=packed.checked_add(s.len).ok_or_else(||invalid("packed data coordinates overflow"))?;
        logical=end;
    }
    if packed!=material.size(){return Err(invalid("packed staging contains unselected or missing bytes"));}
    inherit(&mut build,logical,eof)?;build.finish(eof)
}

/// A request describes the exact normal logical edit plus a completed staging
/// FD; it does not choose the committed FILE, a path or the real work window.
pub(crate) struct Draft<'a> {
    record:Record,
    input:&'a std::fs::File,
    original:LiveStamp,
    index:Vec<u8>,
}
impl<'a> Draft<'a>{
    pub(crate) fn stage(key:Key,start:End,base:Option<&Selected>,at:Option<u64>,
        final_size:u64,input:&'a std::fs::File)->io::Result<Self>{
        let stamp=LiveStamp::read(input)?;
        let preserve=base.map_or(0,|b|b.index.len()).min(final_size);
        let single=at.map(|position|Span{at:position,len:stamp.len});
        Self::encode(key,start,base,preserve,final_size,single.as_slice(),input,stamp)
    }
    pub(crate) fn packed(key:Key,start:End,base:Option<&Selected>,preserve:u64,eof:u64,
        spans:&[Span],input:&'a std::fs::File)->io::Result<Self>{
        let stamp=LiveStamp::read(input)?;
        Self::encode(key,start,base,preserve,eof,spans,input,stamp)
    }
    fn encode(key:Key,start:End,base:Option<&Selected>,preserve:u64,final_size:u64,
        spans:&[Span],input:&'a std::fs::File,stamp:LiveStamp)->io::Result<Self>{
        if base.is_some_and(|b|(b.record.key.pool,b.record.key.file)!=(key.pool,key.file)){
            return Err(invalid("work draft selected a different FILE baseline"));
        }
        let empty;
        let index=match base{
            Some(b)=>&b.index,
            None=>{empty=super::Build::new(key.file)?.finish(0)?;&empty},
        };
        let original=Material::declared(key.file,key.save,0,stamp.len)?;
        let desired=packed_index(index,original,preserve,final_size,spans)?;
        let mut digest=Sha256::new();
        let r=super::blocks::walk(input,key.pool,original,|_,bytes|digest.update(bytes),|_,_|Ok(()))?;
        stamp.require(input)?;
        let (desc,wire)=super::stored::encode_work(key.codec(),&desired)?;
        let at=start.checks.checked_add(r.table_len()?).ok_or_else(||invalid("checks coordinate overflow"))?;
        let end=End{
            data:start.data.checked_add(stamp.len).ok_or_else(||invalid("DATA coordinate overflow"))?,
            checks:at.checked_add(desc.index_bytes()).ok_or_else(||invalid("work index coordinate overflow"))?,
        };
        let map=Map{at,bytes:desc.index_bytes(),sha:desc.index_sha()};
        let record=Record::edit(key,start,end,final_size,digest.finish(),r.directory_sha(),map)?;
        Ok(Self{record,input,original:stamp,index:wire})
    }
    pub(crate) fn record(&self)->Record{self.record}
    pub(crate) fn prepare(&self,token:&crate::system::reader::smb::work_change::ReservedEdit<'_>)->io::Result<Confirmation>{
        let (installed,record)=token.original();
        if record!=self.record{return Err(invalid("prepared input does not match this original SAVE ticket"));}
        let (mut data,mut checks)=(crate::fs::WorkPair::open_edit(token,false)?,crate::fs::WorkPair::open_edit(token,true)?);
        if data.stamp().same(checks.stamp())||data.stamp().same(self.original)||checks.stamp().same(self.original){
            return Err(invalid("staged input and work roles must be independent real files"));
        }
        self.original.require(self.input)?;
        let from=record.start();
        let actual=super::blocks::walk(self.input,record.key.pool,record.source.material,
            |at,bytes|data.put(from.data+at,bytes),|at,bytes|checks.put(from.checks+at,bytes))?;
        if actual!=record.source{return Err(invalid("staged edit no longer matches its original checks"));}
        checks.put(record.map().expect("prepared interval map").at,&self.index)?;
        self.original.require(self.input)?;data.finish()?;checks.finish()?;
        let parent=installed.directory();parent.file.sync_all()?;
        let proof=Confirmation::read_back(parent,record,data.stamp(),checks.stamp())?;
        self.original.require(self.input)?;Ok(proof)
    }
}

#[cfg(test)]
#[path="../../../tests/unit/flat_work_round.rs"]
mod tests;
