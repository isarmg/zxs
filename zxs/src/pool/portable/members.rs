//! Scope a real, ordinary FILE member to a selected portable account and
//! share. Listing is per logical root, never per publisher or creator.
use super::{Selected,ShareUse,decision,marker,row_at,denied};
use crate::{FileId,FileInfo,Role,VolumeId,fs::{HeldDir,invalid},pool::portable_claim::ROOT_NAME};
use std::{collections::BTreeSet,io};

impl Selected<'_,'_>{
    pub(super) fn checked<T>(&self,f:impl FnOnce(&decision::Decisions,VolumeId)->io::Result<T>)->io::Result<T>{
        self.session.at_state(self.original.id(),self.original.action(),|s,now|{
            if now!=self.original{return Err(denied());}f(s,now.volume())
        })
    }
    fn on_root<T>(&self,volume:VolumeId,f:impl FnOnce(&HeldDir,&BTreeSet<Vec<u8>>)->io::Result<T>)->io::Result<T>{
        let source=&self.session.source.source;
        let row=source.genesis.rows.iter().find(|r|r.volume==volume&&r.role==Role::File).ok_or_else(denied)?;
        // Neither share clients nor returned FILE metadata supply paths.
        let root=HeldDir::open(&row.path)?;root.share()?;row_at(&root,row)?;
        if root.entity==source.system.entity{return Err(invalid("ordinary parent aliases selected SYSTEM"));}
        let mark=marker(&root,source.genesis.mark(row))?;
        let sample=||->io::Result<BTreeSet<Vec<u8>>>{
            let v=root.names()?;let n=v.len();let set:BTreeSet<_>=v.into_iter().collect();
            if set.len()!=n||!set.contains(ROOT_NAME.to_bytes()){return Err(invalid("ambiguous or missing native root marker"));}
            Ok(set)
        };
        let before=sample()?;let answer=f(&root,&before)?;
        if before!=sample()?{return Err(invalid("portable ordinary name set changed"));}
        mark.require(&root)?;root.require_named()?;Ok(answer)
    }
    pub(in crate::pool) fn list_files(&self)->io::Result<Vec<FileInfo>>{
        if self.original.action()!=ShareUse::List{return Err(denied());}
        let(vol,members)=self.checked(|s,v|Ok((v,s.files.at(v).cloned().collect::<Vec<_>>())))?;
        self.on_root(vol,|root,names|{
            let mut known=BTreeSet::from([ROOT_NAME.to_bytes().to_vec()]);
            let mut out=Vec::with_capacity(members.len());
            for e in &members{
                let terminal=e.container()?;let witness=e.witness()?;
                if !known.insert(terminal.into_bytes())||!known.insert(witness.into_bytes()){return Err(invalid("two FILEs claim one native role"));}
                e.installed(root,names)?;
                for i in 0..e.slot{
                    let n=e.lower(i)?;
                    if n.as_c_str()==ROOT_NAME{continue;} // specifically the root declaration, not a stage
                    if names.contains(n.to_bytes()){
                        let d=root.directory_in_sample(&n,names)?.ok_or_else(||invalid("skipped stage no longer exists"))?;
                        d.only(None)?;d.require_named()?;
                        if !known.insert(n.into_bytes()){return Err(invalid("private slot aliases another role"));}
                    }
                }
                out.push(info(e));
            }
            if &known!=names{return Err(invalid("unknown or uninstalled ordinary root member"));}
            // No external callback under SYSTEM. An unrelated authorization
            // can extend the prefix, but a changed member or grant cannot pass.
            self.checked(|s,v|if v==vol&&s.files.at(v).eq(members.iter()){Ok(())}else{Err(denied())})?;
            Ok(out)
        })
    }
    /// Concrete metadata only; returning this fact is not permission to
    /// write afterwards. SAVE will have to select the same current grant,
    /// original entry/branch and revision again at its own commitment cut.
    pub(in crate::pool) fn file(&self,name:&str,id:FileId)->io::Result<FileInfo>{
        if self.original.action()==ShareUse::Create{return Err(denied());}
        let e=self.checked(|s,v|s.files.get(v,name).filter(|e|e.file==id).cloned().ok_or_else(denied))?;
        self.on_root(e.volume,|root,names|{
            let held=e.installed(root,names)?;
            self.checked(|s,v|{
                if s.files.get(v,name).is_none_or(|a|a.as_ref()!=e.as_ref()){return Err(denied());}
                held.require(root)?;Ok(info(&e))
            })
        })
    }
}
fn info(e:&super::super::files::ArchivedBirth)->FileInfo{
    FileInfo{entry:e.entry,file:e.file,history:e.history,volume:e.volume,parent:crate::DirectoryParent::Root(e.volume),
        name:e.name.clone(),created_by:e.operation,decision_sequence:Some(e.sequence)}
}
