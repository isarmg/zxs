//! Physical archive selection for one SQLite transfer. Holds no statement
//! or transaction through a source disk, destination disk, or peer wait.
use super::{Catalog,Connection,InitialReader,file_location::{Place,Placed},work_record::{self,ReadRecord}};
use crate::{OperationId,fs::{self,WitnessRead,HeldDir,invalid},
    ordinary::{portable_birth::MoveInstall,working_round::{End,TransferRound,Used}}};
use std::io;

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
struct Round{first:ReadRecord,last:ReadRecord,source:End}
#[derive(Debug,Eq,PartialEq)]
pub(super) struct Manifest{rounds:Vec<Round>}
impl Manifest{
    pub(super) fn select(db:&Connection,b:&Placed,last:Option<ReadRecord>)->io::Result<Self>{
        let file=&b.birth;let mut rounds=Vec::new();
        let mut add=|last:ReadRecord,source:End|->io::Result<()>{
            if rounds.len()>=(fs::DIRECTORY_NAMES-1)/4{return Err(invalid("archived transfer exceeds actual FILE names"));}
            let end=last.record.end();
            let first=work_record::confirmed(db,file,last.record.key().round)?;
            if first.record.key().revision!=1||first.record.key().round!=last.record.key().round||first.issue>last.issue||
                source.data<end.data||source.checks<end.checks{
                return Err(invalid("invalid original round for file transfer"));
            }
            rounds.push(Round{first,last,source});Ok(())
        };
        let nodes=db.prepare(c"SELECT n.save_op,n.round_op,n.revision FROM main.manual_node n
             JOIN main.operation o ON o.op_id=n.commit_op AND o.kind=11 AND o.state=1
             WHERE n.file_id=?1 ORDER BY n.ord")?;
        nodes.bind_bytes(1,&file.file.0)?;let mut tip=None;
        while work_record::start_one(&nodes)?{
            let r=work_record::confirmed(db,file,OperationId(nodes.array(0)?))?;
            if r.record.key().round!=OperationId(nodes.array(1)?)||r.record.key().revision!=work_record::natural(&nodes,2)?||
                last.is_none_or(|l|r.issue>l.issue)||tip.is_some_and(|i:ReadRecord|i.issue>=r.issue){
                return Err(invalid("manual archives do not precede the selected transfer view"));
            }
            add(r,r.record.end())?;tip=Some(r);
        }
        if let Some(last)=last{
            if Some(last)!=tip{
                if tip.is_some_and(|t|t.record.key().round==last.record.key().round){
                    return Err(invalid("current work cannot extend a frozen transfer round"));
                }
                add(last,work_record::ceiling(db,file,last)?)?;
            }
        }
        Ok(Self{rounds})
    }
    // Capacity is only an early lower-bound rejection. Count real selected
    // bytes in short pages, never a long SUM through this FILE's history.
    pub(super) fn space(&self,source:&InitialReader,placed:&Placed,
        check_sql:impl Fn(&Connection,&Catalog)->io::Result<()>,live:impl Fn()->io::Result<()>)->io::Result<u64>{
        let mut bytes=0u64;
        for round in &self.rounds{
            Self::visit(source,placed,*round,&check_sql,&live,|r|{
                let start=r.record.start();let end=r.record.end();
                bytes=bytes.checked_add(end.data-start.data).and_then(|n|n.checked_add(end.checks-start.checks))
                    .ok_or_else(||invalid("confirmed archive size overflow"))?;
                Ok(())
            })?;
        }
        Ok(bytes)
    }
    fn visit(source:&InitialReader,placed:&Placed,round:Round,
        check_sql:&impl Fn(&Connection,&Catalog)->io::Result<()>,live:&impl Fn()->io::Result<()>,mut visit:impl FnMut(ReadRecord)->io::Result<()>)
        ->io::Result<()>{
        let mut after=round.first.issue-1;let mut prev=None;
        while after<round.last.issue{
            live()?;
            let page=source.at(|db,c|{
                check_sql(db,&c)?;
                if Place::current(db,&c,placed.here.file)?!=placed.here{return Err(invalid("transfer source changed during material selection"));}
                Self::page(db,placed,round,after)
            })?;
            if page.is_empty(){return Err(invalid("missing confirmed material of a selected transfer round"));}
            for (m,predecessor) in page{
                if prev.is_some()&&prev!=predecessor{return Err(invalid("confirmed material does not follow the last exact SAVE"));}
                visit(m)?;after=m.issue;prev=Some(m.record.key().save);
            }
        }
        if prev!=Some(round.last.record.key().save){return Err(invalid("material did not reach the original transfer view"));}
        Ok(())
    }
    pub(super) fn transfer(&self,source:&InitialReader,placed:&Placed,from:&HeldDir,stage:&MoveInstall,
        check_sql:impl Fn(&Connection,&Catalog)->io::Result<()>,live:impl Fn()->io::Result<()>)
        ->io::Result<(Used,Used,std::collections::BTreeSet<Vec<u8>>)> {
        live()?;
        let shape=self.rounds.iter().map(|r|(r.last.record.key().round,r.last.record.end())).collect::<Vec<_>>();
        let names=stage.prepare_pairs(&shape)?;
        let source_scope=WitnessRead::at(from)?;let target_scope=WitnessRead::at(stage.body())?;
        let(mut source_used,mut target_used)=(Used::default(),Used::default());
        for round in &self.rounds{
            live()?;
            let end=round.last.record.end();
            let(raw,checks)=stage.open_pair(&target_scope,round.last.record.key().round,end)?;
            let mut transport=TransferRound::open(from,&source_scope,round.last.record,Some(round.source))?;
            let stream=|reader:&mut TransferRound,dir:&HeldDir,sinks:Option<(&std::fs::File,&std::fs::File)>|->io::Result<()>{
                Self::visit(source,placed,*round,&check_sql,&live,|r|reader.step(dir,r.record,sinks,&live))
            };
            stream(&mut transport,from,Some((&raw,&checks)))?;
            transport.finish(from,&mut source_used)?;
            stage.sync_pair(&raw,&checks,end)?;
            let mut readback=TransferRound::open(stage.body(),&target_scope,round.last.record,None)?;
            readback.require_written(&raw,&checks)?;
            stream(&mut readback,stage.body(),None)?;
            readback.require_written(&raw,&checks)?;
            readback.finish(stage.body(),&mut target_used)?;
        }
        live()?;source_used.require(from)?;target_used.require(stage.body())?;
        Ok((source_used,target_used,names))
    }
    fn page(db:&Connection,placed:&Placed,round:Round,after:i64)->io::Result<Vec<(ReadRecord,Option<OperationId>)>>{
        // The pre-v10 internal first-work issuer uses issue 0. It can only
        // contribute its original confirmed first record, never a pending row.
        if after<0&&round.first.issue==0{
            let live=work_record::confirmed(db,&placed.birth,round.first.record.key().save)?;
            if live!=round.first{return Err(invalid("original first SAVE changed"));}
            return Ok(vec![(live,None)]);
        }
        let q=db.prepare(c"SELECT w.edit_op FROM main.work_edit w
         JOIN main.operation o ON o.op_id=w.edit_op AND o.kind=10 AND o.state=1
         WHERE w.file_id=?1 AND w.round_op=?2 AND w.issue>?3 AND w.issue<=?4 ORDER BY w.issue LIMIT 32")?;
        q.bind_bytes(1,&placed.birth.file.0)?;q.bind_bytes(2,&round.last.record.key().round.0)?;
        q.bind_integer(3,after)?;q.bind_integer(4,round.last.issue)?;
        let mut rows=Vec::new();
        while work_record::start_one(&q)?{
            let op=OperationId(q.array(0)?);
            let confirmed=work_record::edit(db,&placed.birth,op)?.filter(|r|r.confirmed)
                .ok_or_else(||invalid("missing exact confirmed original transfer material"))?;
            let edit=confirmed.edit;
            let cur=ReadRecord{record:edit.record,issue:edit.issue};
            if (rows.is_empty()&&after==round.first.issue-1&&cur!=round.first)||
                cur.record.key().round!=round.last.record.key().round||cur.issue<=after||cur.issue>round.last.issue{
                return Err(invalid("transfer material outside original round bounds"));
            }
            rows.push((cur,edit.predecessor));
        }
        Ok(rows)
    }
}
