//! Exact private native objects for one as-yet-unreserved physical operation.
//! No RawFd, File, root path or arbitrary-name callback is lent outside this
//! module. They vanish on process exit and are never a serving copy, a
//! replacement for a known stage, a decision or an authority to unlink.
use super::{Access,CustodyEnd,Side};
use crate::{Entity,fs::{self,Birth,invalid,stream},ordinary::{IncomingSelection,KnownStage}};
use std::{fs::File,io,path::Path};

pub(crate) struct Unlinked<'v>{selection:IncomingSelection<'v>,files:Vec<(usize,File)>,known:Option<KnownStage>,target:(Entity,Birth)}
pub(crate) struct PreparedIncoming<'v>{selection:IncomingSelection<'v>,files:Vec<(usize,File,fs::OriginalStamp)>,known:Option<KnownStage>,target:(Entity,Birth)}
impl CustodyEnd<'_,'_>{
    fn check_new_target(&self,from:&CustodyEnd<'_,'_>,o:&IncomingSelection<'_>)->io::Result<()>{
        if !self.belongs(from.grant,Side::Target)||!from.belongs(self.grant,Side::Source)||!std::ptr::eq(self.pool,from.pool)||
            !matches!(self.pool.access,Access::Incoming|Access::Write){return Err(invalid("a fresh transfer requires the same two incoming endpoint leases"));}
        self.require_current()?;from.require_current()?;o.require(self.pool,self.grant,from,self)?;
        if self.root.same_mount(&from.root)?||self.root.entity.device==from.root.entity.device||
            Birth::read(&self.root.file)?.filesystem()==Birth::read(&from.root.file)?.filesystem(){
            return Err(invalid("lifecycle transfers require two distinct original filesystems"));}
        let iso=o.source_isolation();from.root.checked_child_path(&iso)?;
        if from.root.entry_present(&iso)?{return Err(invalid("original source quarantine already occupied"));}
        let dest=self.grant.name(Side::Target);self.root.checked_child_path(dest)?;
        for s in o.stage_names(){self.root.checked_child_path(&s)?;}
        o.check_occupancy(self.pool,self.grant,self.role,dest,&self.root.names()?)?;
        from.require_current()?;self.require_current()
    }
    // Inspect only this operation's issued, bounded names on its one physical
    // endpoint. Empty unbound names are spent slots, not adoptable objects.
    fn window(&self,o:&IncomingSelection<'_>)->io::Result<(Option<fs::HeldDir>,Option<u8>)>{
        let(issued,bound)=o.stage_state(self.pool,self.grant)?;let mut selected=None;let mut free=None;
        for(idx,n) in o.stage_names().enumerate(){let i=u8::try_from(idx).map_err(|_|invalid("invalid private slot"))?;
            let current=self.root.child_directory(&n)?;
            if let Some((b,expected))=&bound{
                if i==*b{let d=current.ok_or_else(||invalid("bound original private target vanished"))?;
                    if d.entity!=expected.entity||Birth::read(&d.file)?!=expected.birth{return Err(invalid("substituted bound private target"));}
                    selected=Some(d);
                }else if let Some(d)=current{
                    if i>*b{return Err(invalid("extra entry after the original bound slot"));}d.only(None)?;d.require_named()?;
                }
            }else if let Some(d)=current{
                if !issued||free.is_some(){return Err(invalid("unknown private slot or a hole in the original unbound prefix"));}
                d.only(None)?;d.require_named()?;
            }else if free.is_none(){free=Some(i);}
        }
        self.require_current()?;Ok((selected,free))
    }
    pub(crate) fn claim_anonymous<'v>(&self,from:&CustodyEnd<'_,'_>,o:IncomingSelection<'v>)->io::Result<Unlinked<'v>>{
        self.check_new_target(from,&o)?;
        // F0, F0+I1 and F0+I1+I2 each have exactly their own source
        // inventory. No O_TMPFILE is opened until every required role matches.
        let n=o.allocation_count(self.grant)?;
        let target=self.identity()?;let mut files=Vec::with_capacity(n);
        let(dir,free)=self.window(&o)?;if dir.is_none()&&free.is_none(){return Err(invalid("original private ticket exhausted"));}
        let known=o.capture_known(self.pool,self.grant,dir)?;
        for i in 0..n{if known.as_ref().is_some_and(|k|k.has(i)){continue;}let(file,_)=self.root.anonymous_record()?;let st=stream::anonymous_stamp(&file)?;
            if !self.root.shares_file_mount(&file)?||st.stat.size()!=0||st.stat.entity().device!=target.0.device||st.birth.filesystem()!=target.1.filesystem(){
                return Err(invalid("anonymous role is not new on the exact selected target filesystem"));}
            files.push((i,file));
        }
        self.check_new_target(from,&o)?;let(dir,_)=self.window(&o)?;o.require_known(self.pool,self.grant,known.as_ref(),dir.as_ref())?;
        Ok(Unlinked{selection:o,files,known,target})
    }
}
impl<'v> Unlinked<'v>{
    pub(crate) fn copy(self)->io::Result<PreparedIncoming<'v>>{
        let Self{selection:o,mut files,known,target}=self;
        if let Some(k)=&known{k.verify()?;}
        // Reserve the whole actual inventory (including non-HEAD RAW) before
        // writing even one payload byte. No SYSTEM or root lease here.
        for (i,f) in &files{
            let(n,_)=o.claim(*i)?;if n!=0{stream::allocate_stream(f,n)?;}
            let stamp=stream::anonymous_stamp(f)?;
            if stamp.stat.size()!=n||stamp.stat.entity().device!=target.0.device||stamp.birth.filesystem()!=target.1.filesystem(){
                return Err(invalid("unable to reserve the complete original physical role on its actual destination"));}
        }
        let mut result=Vec::with_capacity(files.len());
        for(i,f)in files.drain(..){
            let(n,d)=o.claim(i)?;o.stream(i,|at,b|stream::write_at(&f,b,at))?;
            f.sync_all()?;let stamp=stream::anonymous_stamp(&f)?;
            if stamp.stat.size()!=n||stamp.stat.entity().device!=target.0.device||stamp.birth.filesystem()!=target.1.filesystem()||
                stream::hash_unnamed(&f,&stamp)?!=d{return Err(invalid("independent target readback differs from the complete original role"));}
            result.push((i,f,stamp));
        }Ok(PreparedIncoming{selection:o,files:result,known,target})
    }
}
/// Can only be constructed with the exact PreparedIncoming and both writer
/// leases still in the closure. It cannot open R or name/erase a FILE.
pub(crate) struct ReservationTurn<'w,'v>{pool:&'w crate::pool::lease::LocalPool,selection:&'w IncomingSelection<'v>,
    check:&'w dyn Fn()->io::Result<()>}
impl<'v> ReservationTurn<'_,'v>{
    pub(crate) fn require(&self)->io::Result<(&crate::pool::lease::LocalPool,&IncomingSelection<'v>)>{
        (self.check)()?;Ok((self.pool,self.selection))
    }
}
/// This value never contains the recycle root, and cannot be created by
/// supplying a slot/inode. Its check owns the live mkdir result, not an
/// observed preexisting directory at the same spelling.
pub(crate) struct FreshStageTurn<'w,'v>{selection:&'w IncomingSelection<'v>,pool:&'w crate::pool::lease::LocalPool,
    slot:u8,dir:&'w fs::HeldDir,check:&'w dyn Fn()->io::Result<()>}
impl<'v> FreshStageTurn<'_,'v>{
    pub(crate) fn require(&self)->io::Result<(&crate::pool::lease::LocalPool,&IncomingSelection<'v>,u8,Entity,Birth)>{
        (self.check)()?;Ok((self.pool,self.selection,self.slot,self.dir.entity,Birth::read(&self.dir.file)?))
    }
}
impl<'v> PreparedIncoming<'v>{
    pub(crate) fn with_writer<T>(&self,system:&Path,inspect:impl FnOnce(ReservationTurn<'_,'v>)->io::Result<T>)->io::Result<T>{
        self.selection.with_ends(system,Access::Write,|p,old,new|{
            let check=||self.check_for_reservation(p,old,new);check()?;
            inspect(ReservationTurn{pool:p,selection:&self.selection,check:&check})
        })
    }

    pub(crate) fn ensure_private_stage(&self,system:&Path,after_mkdir:impl FnOnce()->io::Result<()>)->io::Result<u8>{
        self.selection.with_ends(system,Access::Write,|p,old,new|{
            self.check_for_reservation(p,old,new)?;
            let(issued,st)=self.selection.stage_state(p,new.grant)?;
            if !issued{return Err(invalid("no durable original authorizes private target naming"));}
            let(dir,free)=new.window(&self.selection)?;
            if let Some((i,_))=st{if dir.is_none(){return Err(invalid("cannot recreate a bound private directory"));}
                self.selection.confirm_issued(p,new.grant)?;return Ok(i);}
            let slot=free.ok_or_else(||invalid("all original target slots occupied; never adopt earlier directories"))?;
            let name=self.selection.stage_names().nth(usize::from(slot)).ok_or_else(||invalid("unissued private slot"))?;
            let dir=new.root.new_child_directory(&name)?;
            dir.only(None)?;dir.file.sync_all()?;new.root.file.sync_all()?;after_mkdir()?;
            let check=||{
                self.check_for_reservation(p,old,new)?;
                if self.selection.stage_state(p,new.grant)?!=(true,None){return Err(invalid("directory binding is no longer fresh"));}
                let reopened=new.root.child_directory(&name)?.ok_or_else(||invalid("newly created private stage vanished"))?;
                if reopened.entity!=dir.entity||Birth::read(&reopened.file)?!=Birth::read(&dir.file)?||!reopened.same_mount(&dir)?{
                    return Err(invalid("private name is not this call's newly created native directory"));}
                reopened.only(None)?;dir.only(None)?;dir.require_named()?;new.require_current()
            };
            self.selection.bind_created(&FreshStageTurn{selection:&self.selection,pool:p,slot,dir:&dir,check:&check})?;
            Ok(slot)
        })
    }
    /// All durable names are beneath this operation's already-bound native
    /// private directory, in role order. Even complete named bytes are not a
    /// decision: the returned input must be verified outside both root locks.
    pub(crate) fn name_and_reopen(&self,system:&Path,mut after_link:impl FnMut()->io::Result<()>)->io::Result<crate::ordinary::LifecycleScan>{
        self.selection.with_ends(system,Access::Write,|p,old,new|{
            self.check_for_reservation(p,old,new)?;
            self.selection.confirm_issued(p,new.grant)?;
            let(dir,_)=new.window(&self.selection)?;let dir=dir.ok_or_else(||invalid("no real bound target for naming original FILE roles"))?;
            let mut complete=Vec::with_capacity(self.selection.count());
            for i in 0..self.selection.count(){
                // No substitution of an already-named role. Any fresh file is
                // exactly the anonymous FD previously fully written/read back.
                let exists=self.known.as_ref().map(|k|k.recognized(i,&dir,&self.selection)).transpose()?.flatten();
                let stamp=if let Some(known)=exists{known}else{
                    let(_,f,st)=self.files.iter().find(|(j,_,_)|*j==i).ok_or_else(||invalid("no original anonymous target for this role"))?;
                    if !new.root.shares_file_mount(f)?||!dir.shares_file_mount(f)?||stream::anonymous_stamp(f)?!=*st{
                        return Err(invalid("anonymous role no longer at the original checked target"));}
                    let(name,token)=self.selection.new_role(p,new.grant,i,st)?;
                    fs::set_preparation_token(f,&token)?;dir.link_stream(&name,f,st,st.stat.size())?;after_link()?;
                    let actual=dir.pin_stream(&name,st.stat.size())?.ok_or_else(||invalid("newly named material disappeared"))?;
                    if actual.stamp.birth!=st.birth||actual.stamp.stat.entity()!=st.stat.entity(){return Err(invalid("replaced just-linked role"));}
                    fs::require_material_token(&actual.file,&token)?;actual.named(&dir,&name)?;actual.stamp
                };
                complete.push(stamp);
            }
            dir.file.sync_all()?;new.root.file.sync_all()?;new.check_new_target(old,&self.selection)?;
            let goal=self.selection.goal(p,new.grant,&dir,&complete)?;new.read_target(&goal)
        })
    }
    /// After linking, the original anonymous stamps are intentionally no
    /// longer proof. Re-check the full *named* verified destination, both
    /// lease-protected parents and the frozen exact source at this writer.
    pub(crate) fn recheck_for_seal(&self,p:&crate::pool::lease::LocalPool,target:&crate::ordinary::LifecycleChecked)->io::Result<()>{
        if !matches!(p.access,Access::Write)||target.installed(){return Err(invalid("an original lifecycle decision requires its private-stage writer"));}
        let g=self.selection.grant(p)?;let old=p.custody_end(&g,Side::Source)?;let new=p.custody_end(&g,Side::Target)?;
        new.check_new_target(&old,&self.selection)?;target.require(p,&g,&new)?;
        old.root.file.sync_all()?;new.root.file.sync_all()?;old.require_current()?;new.require_current()?;target.require(p,&g,&new)
    }
    /// Reopen both original roles and the same still-current FILE and reserve
    /// candidate, not the unrelated latest actor/name. No namespace or durable
    /// operation is created, even after all these checks succeed.
    pub(crate) fn recheck(&self,system:&Path)->io::Result<()>{
        self.selection.with_ends(system,Access::Incoming,|_,old,new|self.check_material(old,new))
    }
    /// The later issuer must call this under its very own SYSTEM/volume
    /// writer, before creating a durable reservation. An earlier recheck
    /// under Incoming is intentionally not such authority.
    pub(crate) fn check_for_reservation(&self,p:&crate::pool::lease::LocalPool,
        old:&CustodyEnd<'_,'_>,new:&CustodyEnd<'_,'_>)->io::Result<()>{
        if !matches!(p.access,Access::Write)||!std::ptr::eq(new.pool,p)||!std::ptr::eq(old.pool,p){
            return Err(invalid("anonymous originals require this exact reservation writer"));}
        self.check_material(old,new)
    }
    fn check_material(&self,old:&CustodyEnd<'_,'_>,new:&CustodyEnd<'_,'_>)->io::Result<()>{
        new.check_new_target(old,&self.selection)?;
        let(dir,_)=new.window(&self.selection)?;
        self.selection.require_known(new.pool,new.grant,self.known.as_ref(),dir.as_ref())?;
        if self.target!=new.identity()?{return Err(invalid("different transfer target"));}
        for i in 0..self.selection.count(){if self.files.iter().filter(|(j,_,_)|*j==i).count()+usize::from(self.known.as_ref().is_some_and(|k|k.has(i)))!=1{
            return Err(invalid("missing or repeated selected material role"));}}
        for(i,f,st)in &self.files{
            if !new.root.shares_file_mount(f)?||st.stat.size()!=self.selection.claim(*i)?.0||stream::anonymous_stamp(f)?!=*st{
                return Err(invalid("prepared original is not linkable on this exact current target mount"));}
        }new.check_new_target(old,&self.selection)
    }
    #[cfg(test)]
    pub(crate) fn inspection(&self)->io::Result<Vec<(Entity,Birth,u64,[u8;32])>>{
        self.files.iter().map(|(_,f,st)|Ok((st.stat.entity(),st.birth.clone(),st.stat.size(),stream::hash_unnamed(f,st)?))).collect()
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    #[ignore="two caller-provided private aliases of exactly one directory through distinct existing Linux mount instances"]
    fn two_mounts_of_the_same_directory_do_not_make_its_original_unnamed_file_linkable_on_both(){
        let get=|key:&str|fs::HeldDir::open(Path::new(&std::env::var_os(key).expect("two private test mount aliases required"))).unwrap();
        let original=get("ZXS_TEST_LINK_MOUNT_A");let other=get("ZXS_TEST_LINK_MOUNT_B");
        assert_eq!(original.entity,other.entity);assert_eq!(Birth::read(&original.file).unwrap(),Birth::read(&other.file).unwrap());
        assert!(!original.same_mount(&other).unwrap());
        let(f,_)=original.anonymous_record().unwrap();let before=stream::anonymous_stamp(&f).unwrap();
        assert!(original.shares_file_mount(&f).unwrap());assert!(!other.shares_file_mount(&f).unwrap());
        assert_eq!(stream::anonymous_stamp(&f).unwrap(),before); // still anonymous; no named output or mount operation
    }
}
