//! Target half of a same-parent file replacement. The original writer is
//! held across the preparation, the joined reservation, and confirmation.
use super::*;
use crate::ordinary::working_round::Confirmation;

pub(in crate::system::reader::smb) struct ReplacementSave<'writer, 'input> {
    at: WriteAt<'writer>,
    draft: Draft<'input>,
    before: State,
    edit: Edit,
}

pub(in crate::system::reader::smb) struct SyncedReplacement {
    after: State,
    proof: Confirmation,
}

impl<'writer> WriteAt<'writer> {
    pub(super) fn prepare_replacement<'input>(self, op: OperationId, input: &'input File, len: u64)
        -> io::Result<ReplacementSave<'writer, 'input>>
    {
        if input.metadata()?.len()!=len { return Err(invalid("replacement source length changed")); }
        let src=self.source;
        let before=src.at(|db,c|{
            if self.original(db,op)?.is_some(){return Err(invalid("replacement SAVE original is already present"));}
            let state=self.state(db,&c)?;
            if work_record::active_pending(db,&self.birth)?.is_some(){
                return Err(busy("settle the original target SAVE first"));
            }
            Ok(state)
        })?;
        let superseded=before.pending();
        self.physical(&before)?;
        let actual=self.installed.directory();
        let selected=before.selected.map(|(r,physical)|Selected::open(actual,r.record,physical)).transpose()?;
        let (key,start)=if let Some((last,_))=before.selected.filter(|(r,_)|
            before.manual.is_none_or(|(_,_,round,_)|r.record.key().round!=round)){
            let key=last.record.key();
            let revision=key.revision.checked_add(1).filter(|n|*n<i64::MAX as u64)
                .ok_or_else(||invalid("work revision exhausted"))?;
            let start=if superseded.is_some_and(|p|p.edit.record.key().round==key.round){
                selected.as_ref().ok_or_else(||invalid("missing selected original pair"))?.durable_observed_end(actual)?
            }else{last.record.end()};
            (Key{save:op,revision,..key},start)
        }else{
            (Key{pool:self.birth.pool,file:self.birth.file,round:op,save:op,revision:1},End::ZERO)
        };
        let full=(len!=0).then_some(Span{at:0,len});
        let draft=Draft::packed(key,start,selected.as_ref(),0,len,full.as_slice(),input)?;
        let scope=WitnessRead::at(actual)?;
        before.members.require(&scope)?;
        let old_round=superseded.filter(|p|p.edit.record.key().round!=key.round).map(|p|p.edit.record.key().round);
        let retained=old_round.map_or(0,|round|scope.present_work_roles(&crate::ordinary::working_round::names(round)));
        if key.revision==1 { before.members.fits_next_pair(key.round,old_round,retained)?; }
        let record=draft.record();
        let need=(record.end().data-start.data).checked_add(record.end().checks-start.checks)
            .ok_or_else(||invalid("replacement work space overflow"))?;
        if self.root.space()?.available_bytes<need {
            return Err(io::Error::new(io::ErrorKind::StorageFull,"insufficient space for replacement work"));
        }
        let issue=before.latest.map_or(0,|i|i.edit.issue).checked_add(1)
            .filter(|v|*v<i64::MAX).ok_or_else(||invalid("work issuance exhausted"))?;
        let a=self.authority;
        let edit=Edit{record,actor:a.account,credential:a.op,generation:a.generation,share:a.setting.share,
            setting:a.setting.op,position:self.place.op,volume:self.place.volume,nonce:self.nonce,issue,
            parent:before.parent(),predecessor:before.base(),previous_pending:superseded.map(|p|p.edit.record.key().save),retained};
        self.physical(&before)?;
        Ok(ReplacementSave{at:self,draft,before,edit})
    }
}

impl ReplacementSave<'_, '_> {
    pub(in crate::system::reader::smb) fn op(&self)->OperationId { self.edit.record.key().save }

    /// The caller must reserve the companion recycle and the pair row in this same transaction.
    pub(in crate::system::reader::smb) fn reserve(&self,db:&Connection,c:&Catalog)->io::Result<()>{
        if self.at.state(db,c)?!=self.before{return Err(busy("replacement target changed before reservation"));}
        self.edit.insert(db,&self.at.birth)
    }

    fn issued(&self,db:&Connection,c:&Catalog)->io::Result<State>{
        let after=self.at.state(db,c)?;
        if after.pending()!=Some(Issued{edit:self.edit,confirmed:false}){
            return Err(busy("not the exact pending replacement SAVE"));
        }
        Ok(after)
    }

    pub(in crate::system::reader::smb) fn sync(&self)->io::Result<SyncedReplacement>{
        let after=self.at.source.at(|db,c|self.issued(db,&c))?;
        self.at.physical(&after)?;
        let proof=self.draft.prepare(&ReservedEdit{installed:&self.at.installed,record:self.edit.record})?;
        let ready=SyncedReplacement{after,proof};
        self.require_synced(&ready)?;
        Ok(ready)
    }

    /// All filesystem activity stays outside SQLite final selection.
    pub(in crate::system::reader::smb) fn require_synced(&self,ready:&SyncedReplacement)->io::Result<()>{
        self.at.physical(&ready.after)?;
        ready.proof.require(self.at.installed.directory(),self.edit.record)
    }

    pub(in crate::system::reader::smb) fn require_selectable(&self,db:&Connection,c:&Catalog,
        ready:&SyncedReplacement,
    )->io::Result<()>{
        if self.issued(db,c)?!=ready.after{return Err(busy("replacement target changed before confirmation"));}
        Ok(())
    }

    pub(in crate::system::reader::smb) fn confirmed_at(&self,db:&Connection)->io::Result<bool>{
        Ok(self.at.original(db,self.op())?==Some(Issued{edit:self.edit,confirmed:true}))
    }
}
