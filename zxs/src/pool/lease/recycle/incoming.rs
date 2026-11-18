//! Exact private native objects for one as-yet-unreserved physical operation.
//! No RawFd, File, root path or arbitrary-name callback is lent outside this
//! module. They vanish on process exit and are never a serving copy, a
//! replacement for a known stage, a decision or an authority to unlink.
use super::{Access,CustodyEnd,Side};
use crate::{Entity,fs::{self,Birth,invalid,stream},ordinary::IncomingSelection};
use std::{fs::File,io,path::Path};

pub(crate) struct Unlinked<'v>{selection:IncomingSelection<'v>,files:Vec<File>,target:(Entity,Birth)}
pub(crate) struct PreparedIncoming<'v>{selection:IncomingSelection<'v>,files:Vec<(File,fs::OriginalStamp)>,target:(Entity,Birth)}
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
    pub(crate) fn claim_anonymous<'v>(&self,from:&CustodyEnd<'_,'_>,o:IncomingSelection<'v>)->io::Result<Unlinked<'v>>{
        self.check_new_target(from,&o)?;
        let n=o.count();if !(3..=4).contains(&n){return Err(invalid("cannot allocate a partial or unknown FILE role set"));}
        let target=self.identity()?;let mut files=Vec::with_capacity(n);
        for _ in 0..n{let(file,_)=self.root.anonymous_record()?;let st=stream::anonymous_stamp(&file)?;
            if !self.root.shares_file_mount(&file)?||st.stat.size()!=0||st.stat.entity().device!=target.0.device||st.birth.filesystem()!=target.1.filesystem(){
                return Err(invalid("anonymous role is not new on the exact selected target filesystem"));}
            files.push(file);
        }
        self.check_new_target(from,&o)?;Ok(Unlinked{selection:o,files,target})
    }
}
impl<'v> Unlinked<'v>{
    pub(crate) fn copy(self)->io::Result<PreparedIncoming<'v>>{
        let Self{selection:o,mut files,target}=self;
        // Reserve the whole actual inventory (including non-HEAD RAW) before
        // writing even one payload byte. No SYSTEM or root lease here.
        for (i,f) in files.iter().enumerate(){
            let(n,_)=o.claim(i)?;if n!=0{stream::allocate_stream(f,n)?;}
            let stamp=stream::anonymous_stamp(f)?;
            if stamp.stat.size()!=n||stamp.stat.entity().device!=target.0.device||stamp.birth.filesystem()!=target.1.filesystem(){
                return Err(invalid("unable to reserve the complete original physical role on its actual destination"));}
        }
        let mut result=Vec::with_capacity(files.len());
        for(i,f)in files.drain(..).enumerate(){
            let(n,d)=o.claim(i)?;o.stream(i,|at,b|stream::write_at(&f,b,at))?;
            f.sync_all()?;let stamp=stream::anonymous_stamp(&f)?;
            if stamp.stat.size()!=n||stamp.stat.entity().device!=target.0.device||stamp.birth.filesystem()!=target.1.filesystem()||
                stream::hash_unnamed(&f,&stamp)?!=d{return Err(invalid("independent target readback differs from the complete original role"));}
            result.push((f,stamp));
        }Ok(PreparedIncoming{selection:o,files:result,target})
    }
}
impl PreparedIncoming<'_>{
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
        if self.target!=new.identity()?||self.files.len()!=self.selection.count(){return Err(invalid("different anonymous transfer target or missing role"));}
        for(i,(f,st))in self.files.iter().enumerate(){
            if !new.root.shares_file_mount(f)?||st.stat.size()!=self.selection.claim(i)?.0||stream::anonymous_stamp(f)?!=*st{
                return Err(invalid("prepared original is not linkable on this exact current target mount"));}
        }new.check_new_target(old,&self.selection)
    }
    #[cfg(test)]
    pub(crate) fn inspection(&self)->io::Result<Vec<(Entity,Birth,u64,[u8;32])>>{
        self.files.iter().map(|(f,st)|Ok((st.stat.entity(),st.birth.clone(),st.stat.size(),stream::hash_unnamed(f,st)?))).collect()
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
