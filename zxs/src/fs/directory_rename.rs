//! The one physical transition for a directory renamed within its existing
//! parent. Immutable birth bytes and the outside hard-link stay unchanged.
//! The caller supplies the exact SQL name/phase and the original live hold;
//! this module does not issue any business authority or find a pending op.
use super::*;

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(crate) enum DirectoryRenameSide{Original,Destination}

pub(crate) struct DirectoryRename<'a>{
    birth:&'a DirectoryBirth,parent:HeldDir,held:DirectoryHold,
    old:CString,new:CString,side:DirectoryRenameSide,live_owner:bool,
}

pub(super) fn leaf(value:&str)->io::Result<CString>{
    let raw=crate::display::component(value.as_bytes(),255)?;
    if crate::ordinary::portable_birth::Birth::reserved_spelling(raw.as_bytes()){return Err(unknown());}
    CString::new(value).map_err(|_|unknown())
}

fn sample(parent:&HeldDir)->io::Result<BTreeSet<Vec<u8>>>{Ok(parent.names()?.into_iter().collect())}

impl<'a> DirectoryRename<'a>{
    /// A cold caller can prove either existing shape, but may never start
    /// its first forward rename. Tests may also provide an owned live lease.
    pub(crate) fn hold(birth:&'a DirectoryBirth,parent:&HeldDir,old_name:&str,new_name:&str,
        mut owner:Option<DirectoryHold>)->io::Result<Self>{
        let hot=owner.is_some();Self::capture(birth,parent,old_name,new_name,&mut owner,hot)
    }

    /// The original owner must have drained all users of this OFD. On a
    /// definite pre-reservation error it keeps a positively restored SH or
    /// no lease at all; a failed flock conversion is not retained evidence.
    pub(crate) fn hold_live(birth:&'a DirectoryBirth,parent:&HeldDir,old_name:&str,new_name:&str,
        owner:&mut Option<DirectoryHold>)->io::Result<Self>{
        Self::capture(birth,parent,old_name,new_name,owner,true)
    }

    fn capture(birth:&'a DirectoryBirth,parent:&HeldDir,old_name:&str,new_name:&str,
        owner:&mut Option<DirectoryHold>,live_owner:bool)->io::Result<Self>{
        birth.valid()?;
        let old=leaf(old_name)?;let new=leaf(new_name)?;
        if old==new{return Err(unknown());}
        let owned=HeldDir::trusted_storage(&parent.path)?;owned.exclusive()?;
        if owned.entity!=parent.entity||!owned.same_mount(parent)?{return Err(unknown());}
        let formed=sample(&owned)?;
        let side=Self::in_parent(birth,&owned,&formed,&old,&new)?;
        if live_owner&&side!=DirectoryRenameSide::Original{return Err(conflict());}
        let name=if side==DirectoryRenameSide::Original{&old}else{&new};
        let held=if live_owner{owner.take().ok_or_else(unknown)?}else{birth.open_at(&owned,&formed,name)?};
        let result=Self{birth,parent:owned,held,old,new,side,live_owner};
        let checked=(||{
            // Upgrade that exact OFD only after the owner drained any same
            // OFD users. Actual independent descendants retain separate SH.
            result.held.marker.file().try_lock().map_err(io::Error::from)?;
            result.require(side)?;
            if live_owner{
                let target=result.target_room(&formed)?;
                if result.new.to_bytes().len()>result.old.to_bytes().len(){
                    check_subtree_growth(&result.held.dir,target)?;
                }
            }
            Ok(())
        })();
        if let Err(e)=checked{
            if live_owner{*owner=result.release_unchanged().ok();}
            return Err(e);
        }
        Ok(result)
    }

    /// Called again with the actual locked parent; enumeration is not used
    /// to call an unregistered raw object a directory proof.
    fn in_parent(birth:&DirectoryBirth,parent:&HeldDir,names:&BTreeSet<Vec<u8>>,old:&CStr,new:&CStr)
        ->io::Result<DirectoryRenameSide>{
        let pin=birth.witness();let stage=birth.stage();
        for name in [old,new,pin.as_c_str(),stage.as_c_str()]{
            if parent.entry_present(name)?!=names.contains(name.to_bytes()){return Err(unknown());}
        }
        if !names.contains(pin.to_bytes())||names.contains(stage.to_bytes()){return Err(unknown());}
        match (names.contains(old.to_bytes()),names.contains(new.to_bytes())){
            (true,false)=>Ok(DirectoryRenameSide::Original),
            (false,true)=>Ok(DirectoryRenameSide::Destination),
            (true,true)=>Err(conflict()),
            _=>Err(unknown()),
        }
    }

    fn require(&self,side:DirectoryRenameSide)->io::Result<()> {
        let names=sample(&self.parent)?;
        if Self::in_parent(self.birth,&self.parent,&names,&self.old,&self.new)?!=side{return Err(unknown());}
        let expected=if side==DirectoryRenameSide::Original{&self.old}else{&self.new};
        if self.held.witness!=self.birth.witness()||
            self.held.dir.path.file_name().is_none_or(|actual|actual.as_bytes()!=expected.to_bytes()){
            return Err(unknown());
        }
        self.held.require_selected(&self.parent,&names)?;
        let canonical=self.birth.header();let mut observed=vec![0;canonical.len()];
        self.held.marker.file().read_exact_at(&mut observed,0)?;
        if observed!=canonical{return Err(unknown());}
        self.held.marker.unchanged()?;Ok(())
    }

    fn target_room(&self,names:&BTreeSet<Vec<u8>>)->io::Result<PathBudget>{
        let limit=ComponentLimit::held(&self.parent)?;
        let budget=NameBudget::from_original(path_budget(&self.parent.path)?,limit);
        let path=budget.child(&self.new)?;
        NameBudget::from_original(path,limit).child(DIRECTORY_MARK)?;
        let a=self.old.to_bytes().len();let b=self.new.to_bytes().len();
        if b>a{
            let bytes:usize=names.iter().map(|v|v.len()+1).sum();
            if bytes+(b-a)>DIRECTORY_BYTES{return Err(io::ErrorKind::StorageFull.into());}
        }
        Ok(path)
    }

    pub(crate) fn side(&self)->DirectoryRenameSide{self.side}
    pub(crate) fn parent(&self)->&HeldDir{&self.parent}
    /// Still the original, protected and exact OLD name. The caller may
    /// release the EX before any durable request is known to exist.
    pub(crate) fn reservation_sample(&self)->io::Result<BTreeSet<Vec<u8>>>{
        if !self.live_owner||self.side!=DirectoryRenameSide::Original{return Err(unknown());}
        self.require(self.side)?;
        let names=sample(&self.parent)?;
        if Self::in_parent(self.birth,&self.parent,&names,&self.old,&self.new)?!=self.side{return Err(unknown());}
        self.target_room(&names)?;Ok(names)
    }
    pub(crate) fn release_unchanged(self)->io::Result<DirectoryHold>{
        if !self.live_owner||self.side!=DirectoryRenameSide::Original||
           self.held.dir.path.file_name().is_none_or(|x|x.as_bytes()!=self.old.to_bytes())||
           self.held.witness!=self.birth.witness(){return Err(unknown());}
        self.held.marker.file().try_lock_shared().map_err(io::Error::from)?;
        self.held.require_selected(&self.parent,&sample(&self.parent)?)?;
        let canonical=self.birth.header();let mut bytes=vec![0;canonical.len()];
        self.held.marker.file().read_exact_at(&mut bytes,0)?;
        if bytes!=canonical{return Err(unknown());}
        self.held.marker.unchanged()?;Ok(self.held)
    }

    /// `admit` must verify the already reserved original while the exact
    /// parent/marker are still held. `finish` selects or aborts that same
    /// ticket only after real same-parent durability and both-name proof.
    pub(crate) fn settle<T>(mut self,want:DirectoryRenameSide,admit:impl FnOnce()->io::Result<()>,
        after_syscall:impl FnOnce()->io::Result<()>,finish:impl FnOnce(&DirectoryHold)->io::Result<T>)
        ->io::Result<(DirectoryHold,T)>{
        let seen=self.side;
        self.require(seen)?;
        if want!=seen{
            if want==DirectoryRenameSide::Destination&&!self.live_owner{return Err(unknown());}
            let (from,to)=if want==DirectoryRenameSide::Destination{(&self.old,&self.new)}else{(&self.new,&self.old)};
            if want==DirectoryRenameSide::Destination{self.target_room(&sample(&self.parent)?)?;}
            let target=self.parent.checked_child_path(to)?;
            admit()?;
            // SAFETY: held exact single-component names and real locked
            // directory. The native primitive is RENAME_NOREPLACE only.
            os_result(unsafe{zxs_rename_fresh(self.parent.file.as_raw_fd(),from.as_ptr(),self.parent.file.as_raw_fd(),to.as_ptr())})?;
            self.held.dir.path=target;
            after_syscall()?;
        }
        self.held.sync_before_publish(&self.parent)?;
        self.require(want)?;
        let output=finish(&self.held)?;
        // Return a read hold on the original directory, not a second open.
        // The caller treats a failure after its final decision as unknown.
        self.held.marker.file().try_lock_shared().map_err(io::Error::from)?;
        self.require(want)?;
        Ok((self.held,output))
    }
}

// Only a longer prefix needs this geometry-only walk. The real directory's
// exclusive marker prevents legitimate descendant work; names come from
// retained relative FDs and no regular file is opened for its contents.
// Large trees fail before a request is issued instead of pausing forever.
const MAX_GEOMETRY_NAMES:usize=32_768;
pub(super) fn check_subtree_growth(dir:&HeldDir,proposed:PathBudget)->io::Result<()>{
    let maximum=CString::new(vec![b'x';255]).map_err(|_|unknown())?;
    let mut remaining=MAX_GEOMETRY_NAMES;
    subtree_fits(dir,proposed,&maximum,&mut remaining)
}
fn subtree_fits(dir:&HeldDir,proposed:PathBudget,maximum:&CStr,remaining:&mut usize)->io::Result<()>{
    // Preserve future names too, not only today's material. At a currently
    // real directory the widest reserved shape is a private transfer
    // pocket, its FILE container, then one resident material/name (three
    // bounded native components). A not-yet-installed child cannot itself
    // already have a published child. Reserve this checked upper bound
    // also for physical bodies and stages rather than guessing their kind.
    proposed.child(maximum)?.child(maximum)?.child(maximum)?;
    let entries=sample(dir)?;
    *remaining=remaining.checked_sub(entries.len()).ok_or_else(||
        io::Error::new(io::ErrorKind::WouldBlock,"ordinary rename exceeds the bounded path geometry scan"))?;
    for raw in &entries{
        let edge=CString::new(raw.as_slice()).map_err(|_|unknown())?;
        let next=proposed.child(&edge)?;
        let pin=dir.pin_record(&edge)?.ok_or_else(unknown)?;
        let meta=pin.metadata()?;
        if meta.is_dir(){
            let child=dir.directory_in_sample(&edge,&entries)?.ok_or_else(unknown)?;
            if child.entity!=(Entity{device:meta.dev(),inode:meta.ino()})||mounts::id(&pin)?!=mounts::id(&child.file)?{
                return Err(unknown());
            }
            drop(pin);
            subtree_fits(&child,next,maximum,remaining)?;
            child.require_named()?;
        }
    }
    if sample(dir)?!=entries{return Err(unknown());}
    dir.require_named()
}

#[cfg(test)]
#[path = "../../tests/unit/attached/fs/directory_rename/tests.rs"]
mod tests;
