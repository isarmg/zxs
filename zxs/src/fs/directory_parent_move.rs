//! Physical two-parent transition for one original ordinary directory on
//! one held F. No detached tree, copying, raw path authority or SQL here.
//! An outer link moves first and the body follows; the inverse moves the
//! body first. Both names refer to the same original marker throughout.
use super::{rename::{check_subtree_growth,leaf},*};
use crate::ordinary::portable_birth::{ParentMoveStep as Phase,lock_parent_pair};

type Names=BTreeSet<Vec<u8>>;
struct Sample{old:Names,new:Names,phase:Phase}
impl Sample{
    fn at(&self,is_new:bool)->&Names{if is_new{&self.new}else{&self.old}}
}

struct Edges{old:CString,new:CString,outer:CString,stage:CString}
impl Edges{
    fn leaf(&self,new:bool)->&CStr{if new{&self.new}else{&self.old}}
    fn sample(&self,old:&HeldDir,new:&HeldDir)->io::Result<Sample>{
        let exact=|parent:&HeldDir,names:&Names,name:&CStr|{
            let present=names.contains(name.to_bytes());
            if parent.entry_present(name)?!=present{return Err(unknown());}
            Ok(present)
        };
        let o=census(old)?;let n=census(new)?;
        if exact(old,&o,&self.stage)?||exact(new,&n,&self.stage)?{return Err(unknown());}
        let phase=match (exact(old,&o,&self.old)?,exact(new,&n,&self.new)?,
            exact(old,&o,&self.outer)?,exact(new,&n,&self.outer)?) {
            (true,false,true,false)=>Phase::Old,
            (true,false,false,true)=>Phase::LinkAtNew,
            (false,true,false,true)=>Phase::New,
            (true,true,_,_)|(_,_,true,true)=>return Err(conflict()),
            _=>return Err(unknown()),
        };
        Ok(Sample{old:o,new:n,phase})
    }
    fn room(&self,parent:&HeldDir,names:&Names,new:bool)->io::Result<PathBudget>{
        let body=self.leaf(new);
        let limit=ComponentLimit::held(parent)?;
        let proposed=NameBudget::from_original(path_budget(&parent.path)?,limit);
        proposed.child(&self.outer)?;
        let path=proposed.child(body)?;
        NameBudget::from_original(path,limit).child(DIRECTORY_MARK)?;
        let missing=[body,self.outer.as_c_str()].into_iter().filter(|n|!names.contains(n.to_bytes()));
        let (count,bytes)=missing.fold((0,0),|(c,b),name|(c+1,b+name.to_bytes().len()+1));
        let used=names.iter().map(|n|n.len()+1).sum::<usize>();
        if names.len()+count>DIRECTORY_NAMES||used+bytes>DIRECTORY_BYTES{
            return Err(io::ErrorKind::StorageFull.into());
        }
        Ok(path)
    }
}

fn census(parent:&HeldDir)->io::Result<Names>{
    let found=parent.names()?;let len=found.len();
    let exact=found.into_iter().collect::<Names>();
    if exact.len()!=len{return Err(unknown());}
    Ok(exact)
}

fn require_ancestry(floor:&HeldDir,body:&HeldDir,old:&HeldDir,new:&HeldDir)->io::Result<()>{
    floor.require_named()?;
    let top=Location::held(&floor.file)?;let moved=Location::held(&body.file)?;
    if moved.mount!=top.mount||body.entity.device!=old.entity.device||old.entity.device!=new.entity.device{
        return Err(invalid("directory parents and body do not use one actual rename domain"));
    }
    for start in [old,new]{
        // Walk the kernel's actual parents, not just similarly spelled
        // absolute prefixes. The checked lexical suffix limits the walk and
        // supplies its expected endpoint, which must be the held F itself.
        let count=start.path.strip_prefix(&floor.path).map_err(|_|unknown())?.components().count();
        let mut here=start.file.try_clone()?;
        for distance in 0..=count{
            let at=Location::held(&here)?;
            if at.entity==moved.entity{
                return Err(invalid("directory move cannot select the directory itself or a descendant"));
            }
            if at.mount!=top.mount||(at==top)!=(distance==count){return Err(unknown());}
            if distance!=count{here=child(&here,c"..")?;}
        }
        start.require_named()?;
    }
    floor.require_named()
}

pub(crate) struct DirectoryParentMove<'a>{
    birth:&'a DirectoryBirth,floor:&'a HeldDir,old:HeldDir,new:HeldDir,edges:Edges,
    held:DirectoryHold,phase:Phase,live_owner:bool,
}
impl<'a> DirectoryParentMove<'a>{
    /// The original owner has drained all same-OFD borrows. A definite
    /// pre-reservation error preserves a real restored SH or no lease.
    pub(crate) fn hold_live(birth:&'a DirectoryBirth,floor:&'a HeldDir,old:&HeldDir,new:&HeldDir,
        old_name:&str,new_name:&str,owner:&mut Option<DirectoryHold>)->io::Result<Self>{
        Self::capture(birth,floor,old,new,old_name,new_name,owner,true)
    }

    /// Reopen a particular already reserved operation's exact phase. This
    /// never turns an observed cold OLD into permission to start a move.
    pub(crate) fn hold_reserved(birth:&'a DirectoryBirth,floor:&'a HeldDir,old:&HeldDir,new:&HeldDir,
        old_name:&str,new_name:&str)->io::Result<Self>{
        Self::capture(birth,floor,old,new,old_name,new_name,&mut None,false)
    }

    fn capture(birth:&'a DirectoryBirth,floor:&'a HeldDir,old:&HeldDir,new:&HeldDir,
        old_name:&str,new_name:&str,owner:&mut Option<DirectoryHold>,live_owner:bool)->io::Result<Self>{
        birth.valid()?;
        let edges=Edges{old:leaf(old_name)?,new:leaf(new_name)?,outer:birth.witness(),stage:birth.stage()};
        let (old,new)=lock_parent_pair(old,new)?;
        let seen=edges.sample(&old,&new)?;
        let phase=seen.phase;
        if live_owner&&phase!=Phase::Old{return Err(conflict());}
        let held=if live_owner{owner.take().ok_or_else(unknown)?}else if phase==Phase::LinkAtNew{
            // Only a cold split has its outer witness outside the current
            // body parent. Acquire that exact pair once, then keep its OFD.
            let dir=old.directory_in_sample(&edges.old,&seen.old)?.ok_or_else(unknown)?;
            let marker=WitnessRead::at(&dir)?.two_parent_header(&old,&new,&seen.new,
                DIRECTORY_MARK,&edges.outer,birth.header().len() as u64)?;
            DirectoryHold{dir,marker,witness:edges.outer.clone()}
        }else if phase==Phase::Old{birth.open_at(&old,&seen.old,&edges.old)?}
        else{birth.open_at(&new,&seen.new,&edges.new)?};
        let mut known=Self{birth,floor,old,new,edges,held,phase,live_owner};
        let captured=(||{
            known.held.marker.file().try_lock().map_err(io::Error::from)?;
            let fresh=known.prove(false)?;
            if live_owner{known.fit(&fresh,true)?;}
            Ok(())
        })();
        if let Err(e)=captured{
            if live_owner{*owner=known.release_unchanged().ok();}
            return Err(e);
        }
        Ok(known)
    }

    pub(crate) fn phase(&self)->Phase{self.phase}
    pub(crate) fn parents(&self)->(&HeldDir,&HeldDir){(&self.old,&self.new)}
    pub(crate) fn reservation_sample(&mut self)->io::Result<(Names,Names)>{
        if !self.live_owner||self.phase!=Phase::Old{return Err(unknown());}
        let sample=self.prove(false)?;
        self.edges.room(&self.new,&sample.new,true)?;
        Ok((sample.old,sample.new))
    }
    pub(crate) fn release_unchanged(mut self)->io::Result<DirectoryHold>{
        if !self.live_owner||self.phase!=Phase::Old||self.held.witness!=self.edges.outer{return Err(unknown());}
        self.held.marker.file().try_lock_shared().map_err(io::Error::from)?;
        let names=census(&self.old)?;
        if self.held.dir.path!=self.old.checked_child_path(&self.edges.old)?||
            !census(&self.held.dir)?.contains(DIRECTORY_MARK.to_bytes()){
            return Err(unknown());
        }
        self.held.require_selected(&self.old,&names)?;
        self.held.marker.recheck_directory_marker(&self.held.dir,&self.old,&self.edges.outer,&self.birth.header(),false)?;
        Ok(self.held)
    }

    fn parent(&self,is_new:bool)->&HeldDir{if is_new{&self.new}else{&self.old}}
    fn fit(&self,sample:&Sample,to_new:bool)->io::Result<()>{
        let proposed=self.edges.room(self.parent(to_new),sample.at(to_new),to_new)?;
        let original=path_budget(&self.held.dir.path)?;
        if proposed.bytes>original.bytes||proposed.depth>original.depth{
            check_subtree_growth(&self.held.dir,proposed)?;
        }
        Ok(())
    }
    fn prove(&mut self,moved_link:bool)->io::Result<Sample>{
        let sample=self.edges.sample(&self.old,&self.new)?;
        if sample.phase!=self.phase||self.held.witness!=self.edges.outer{return Err(unknown());}
        let body_new=self.phase==Phase::New;let outer_new=self.phase!=Phase::Old;
        let containing=if body_new{&self.new}else{&self.old};
        let outer=if outer_new{&self.new}else{&self.old};
        let body=self.held.directory();
        let named=containing.directory_in_sample(self.edges.leaf(body_new),sample.at(body_new))?.ok_or_else(unknown)?;
        if named.path!=body.path||named.entity!=body.entity||!named.same_mount(body)?||
            !census(body)?.contains(DIRECTORY_MARK.to_bytes()){
            return Err(unknown());
        }
        WitnessRead::require_parent(body)?;require_ancestry(self.floor,body,&self.old,&self.new)?;
        self.held.marker.recheck_directory_marker(&self.held.dir,outer,&self.edges.outer,&self.birth.header(),moved_link)?;
        self.held.dir.require_named()?;containing.require_named()?;outer.require_named()?;
        Ok(sample)
    }

    fn sync(&mut self,moved_link:bool)->io::Result<()>{
        self.prove(moved_link)?;
        self.held.marker.file().sync_all()?;self.held.dir.file.sync_all()?;
        self.old.file.sync_all()?;self.new.file.sync_all()?;
        self.prove(false)?;Ok(())
    }

    /// `admit` rechecks the same authorized operation just before each
    /// no-replacement syscall. `finish` is only called after both real
    /// parents, the directory and its marker are durable and exactly proven.
    pub(crate) fn settle<T>(mut self,want:DirectoryRenameSide,mut admit:impl FnMut(Phase)->io::Result<()>,
        mut point:impl FnMut(Phase)->io::Result<()>,finish:impl FnOnce(&DirectoryHold)->io::Result<T>)
        ->io::Result<(DirectoryHold,T)>{
        use Phase::{Old,LinkAtNew,New};
        let goal=match want{DirectoryRenameSide::Original=>Old,DirectoryRenameSide::Destination=>New};
        if (!self.live_owner&&self.phase==Old&&goal==New)||(self.live_owner&&goal==Old){
            return Err(invalid("not an authorized physical direction for this directory move"));
        }
        self.sync(false)?;
        if !self.live_owner&&self.phase!=goal{
            let before=self.prove(false)?;
            self.fit(&before,goal==New)?;
        }
        while self.phase!=goal{
            let before=self.prove(false)?;
            let to_new=goal==New;
            self.edges.room(self.parent(to_new),before.at(to_new),to_new)?;
            let (link,next)=match (self.phase,goal){
                (Old,New)=>(true,LinkAtNew),(LinkAtNew,New)=>(false,New),
                (New,Old)=>(false,LinkAtNew),(LinkAtNew,Old)=>(true,Old),
                _=>return Err(unknown()),
            };
            let from=self.parent(!to_new);let to=self.parent(to_new);
            let (a,b)=if link{(self.edges.outer.as_c_str(),self.edges.outer.as_c_str())}
                else{(self.edges.leaf(!to_new),self.edges.leaf(to_new))};
            let target=to.checked_child_path(b)?;
            admit(self.phase)?;
            // SAFETY: actual locked same-F parents and checked single
            // components, never replace and never fall back to copying.
            os_result(unsafe{zxs_rename_fresh(from.file.as_raw_fd(),a.as_ptr(),to.file.as_raw_fd(),b.as_ptr())})?;
            if !link{self.held.dir.path=target;}
            self.phase=next;point(next)?;
            self.sync(link)?;
        }
        let result=finish(&self.held)?;
        self.held.marker.file().try_lock_shared().map_err(io::Error::from)?;
        self.prove(false)?;Ok((self.held,result))
    }
}

#[cfg(test)]
#[path = "../../tests/unit/attached/fs/directory_parent_move/tests.rs"]
mod tests;
