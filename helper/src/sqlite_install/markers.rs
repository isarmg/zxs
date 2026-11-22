//! Enrollment only, in the original fixed slot. Application data, historical
//! markers, arbitrary caller names and filesystem-specific formats are not inputs.
use super::{bad,originals::Group,DATABASE,MARKER};
use crate::pinned::{Entity,same_mount_leaf};
use rustix::fs::{AtFlags,Mode,OFlags,StatxFlags,makedev,openat,statx};
use std::{ffi::OsStr,fs::File,io,os::unix::{ffi::OsStrExt,fs::FileExt}};

#[derive(Eq,PartialEq)]
struct Stamp{dev:u64,ino:u64,mount:u64,uid:u32,mode:u16,links:u32,len:u64,mt:(i64,u32),ct:(i64,u32)}
fn stamp(fd:&File,name:&OsStr)->io::Result<Stamp>{
    let f=AtFlags::SYMLINK_NOFOLLOW|AtFlags::NO_AUTOMOUNT|
        if name.is_empty(){AtFlags::EMPTY_PATH}else{AtFlags::empty()};
    let ask=StatxFlags::TYPE|StatxFlags::MODE|StatxFlags::NLINK|StatxFlags::UID|StatxFlags::INO|StatxFlags::SIZE
        |StatxFlags::MNT_ID|StatxFlags::MTIME|StatxFlags::CTIME;
    let s=statx(fd,name,f,ask)?;
    if s.stx_mask&ask.bits()!=ask.bits()||s.stx_mnt_id==0||s.stx_ino==0{return Err(bad("cannot identify the original directory or marker"));}
    Ok(Stamp{dev:makedev(s.stx_dev_major,s.stx_dev_minor),ino:s.stx_ino,mount:s.stx_mnt_id,uid:s.stx_uid,
        mode:s.stx_mode,links:s.stx_nlink,len:s.stx_size,mt:(s.stx_mtime.tv_sec,s.stx_mtime.tv_nsec),ct:(s.stx_ctime.tv_sec,s.stx_ctime.tv_nsec)})
}
fn held(f:&File)->io::Result<Stamp>{stamp(f,OsStr::new(""))}
fn slot()->&'static OsStr{OsStr::from_bytes(MARKER)}
fn present(g:&Group,i:usize)->io::Result<bool>{
    if i==0{let s=g.controls(0,&[DATABASE.as_bytes(),MARKER])?;
        if !s[0]{return Err(bad("original database missing on SYSTEM"));}Ok(s[1])}
    else{Ok(g.controls(i,&[MARKER])?[0])}
}
fn feedback_shape(file:&Stamp,dir:&Stamp,uid:u32,len:u64)->io::Result<()>{
    if file.dev!=dir.dev||file.mount!=dir.mount||file.mode!=0o100600||file.links!=1||file.uid!=uid||file.len!=len{
        return Err(bad("existing or new enrollment slot is not the exact private regular marker"));
    }Ok(())
}
fn exact_bytes(f:&File)->io::Result<[u8;101]>{
    let mut b=[0;101];f.read_exact_at(&mut b,0)?;
    loop{match f.read_at(&mut [0],101){Ok(0)=>break,Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue,
        Err(e)=>return Err(e),_=>return Err(bad("extra bytes on original marker")),}}
    Ok(b)
}
struct Mark{fd:File,s:Stamp}
impl Mark{
    fn inspect(g:&Group,i:usize,wanted:&[u8;101])->io::Result<Self>{
        Self::read_original(&g.dirs()[i],g.uid(),wanted)
    }
    fn read_original(dir:&File,uid:u32,wanted:&[u8;101])->io::Result<Self>{
        let m=Self::pin_original(dir,uid)?;m.verify(dir,wanted)?;Ok(m)
    }
    fn pin_original(dir:&File,uid:u32)->io::Result<Self>{Self::pin_system(dir,uid,false)}
    fn pin_system(dir:&File,uid:u32,local:bool)->io::Result<Self>{
        let before=named_mark(dir,local)?;feedback_shape(&before,&held(dir)?,uid,101)?;
        let pin=if local{same_mount_leaf(dir,slot())?}else{
            File::from(openat(dir,slot(),OFlags::PATH|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty())?)
        };
        let s=held(&pin)?;if s!=before{return Err(bad("original enrollment slot changed before it was held"));}
        feedback_shape(&s,&held(dir)?,uid,101)?;
        // Only this original pre-admitted private regular control file is opened.
        // No raw DB descriptor is open here, and no mutable path follows the pin.
        let read=if local{crate::pinned::discovery_claim_reader}else{crate::pinned::sqlite_claim_reader};
        let fd=read(&pin,Entity{dev:s.dev,ino:s.ino},s.mount)?;
        if held(&fd)?!=s||named_mark(dir,local)?!=s{return Err(bad("original enrollment slot changed during opening"));}
        Ok(Self{fd,s})
    }
    fn verify(&self,dir:&File,wanted:&[u8;101])->io::Result<()>{self.verify_system(dir,wanted,false)}
    fn verify_system(&self,dir:&File,wanted:&[u8;101],local:bool)->io::Result<()>{
        if held(&self.fd)?!=self.s||named_mark(dir,local)?!=self.s{return Err(bad("original enrollment marker or name changed"));}
        if &exact_bytes(&self.fd)?!=wanted{
            return Err(bad("existing enrollment marker disagrees with this original claim; left intact"));
        }
        if held(&self.fd)?!=self.s||named_mark(dir,local)?!=self.s{return Err(bad("enrollment marker changed during read"));}
        Ok(())
    }
    fn create(g:&Group,i:usize,wanted:&[u8;101])->io::Result<Self>{
        let dir=&g.dirs()[i];
        let fd=File::from(openat(dir,slot(),OFlags::CREATE|OFlags::EXCL|OFlags::RDWR|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::RUSR|Mode::WUSR)?);
        let empty=held(&fd)?;feedback_shape(&empty,&held(dir)?,g.uid(),0)?;
        if !present(g,i)?||stamp(dir,slot())?!=empty{return Err(bad("new fixed name does not point to the exclusive empty original"));}
        fd.write_all_at(wanted,0)?;
        let s=held(&fd)?;feedback_shape(&s,&held(dir)?,g.uid(),101)?;
        let me=Self{fd,s};me.verify(dir,wanted)?;Ok(me)
    }
    fn sync(self,g:&Group,i:usize,wanted:&[u8;101])->io::Result<Self>{
        let dir=&g.dirs()[i];let parent_before=held(dir)?;self.verify(dir,wanted)?;
        self.fd.sync_all()?;dir.sync_all()?;
        let again=Self::inspect(g,i,wanted)?;
        if again.s!=self.s||held(dir)?!=parent_before{return Err(bad("marker or original parent changed across sync/readback"));}
        self.verify(dir,wanted)?;Ok(again)
    }
}

/// Non-authoritative, never passed to a SQLite or maintenance opener. A
/// readable claim and matching database metadata do not prove committed INIT.
pub(super) struct DiscoveredSystem {
    mark:Mark,
    body:[u8;101],
    database:Stamp,
}
impl DiscoveredSystem{
    pub(super) fn probe(dir:&File,uid:u32)->io::Result<Option<Self>>{
        let absent=|e:&io::Error|e.kind()==io::ErrorKind::NotFound;
        let pin=match same_mount_leaf(dir,slot()){
            Ok(f)=>f,
            Err(e) if absent(&e)=>{
                return match same_mount_leaf(dir,slot()){
                    Err(e) if absent(&e)=>Ok(None),
                    _=>Err(bad("a control marker appeared during discovery")),
                };
            }
            Err(e)=>return Err(e),
        };
        let before=held(&pin)?;let parent=held(dir)?;
        feedback_shape(&before,&parent,uid,101)?;
        let fd=crate::pinned::discovery_claim_reader(&pin,Entity{dev:before.dev,ino:before.ino},before.mount)?;
        if held(&fd)?!=before||held(&same_mount_leaf(dir,slot())?)?!=before{return Err(bad("control marker changed during opening"));}
        let mark=Mark{fd,s:before};
        let body=exact_bytes(&mark.fd)?;
        verify_discovery_mark(&mark,&body,dir)?;
        let [pool,_,_]=coordinates(&body);
        if matches!(body[20],1|2)&&volume_body(&body,&pool){return Ok(None);}
        if !system_body(&body,&pool){return Err(bad("the private marker is not recognizable SYSTEM evidence"));}
        // Only O_PATH metadata for the database. The time/size check can
        // yield unknown on a busy pool; no content/lock/journal is used.
        let database=held(&same_mount_leaf(dir,OsStr::new(DATABASE))?)?;
        let ds=held(dir)?;
        if database.len==0||database.ino==mark.s.ino{
            return Err(bad("no distinct positive-sized SYSTEM database metadata"));
        }
        feedback_shape(&database,&ds,uid,database.len)?;
        let found=Self{mark,body,database};found.require(dir)?;Ok(Some(found))
    }
    pub(super) fn coordinates(&self)->[[u8;16];3]{coordinates(&self.body)}
    pub(super) fn require(&self,dir:&File)->io::Result<()>{
        verify_discovery_mark(&self.mark,&self.body,dir)?;
        if held(&same_mount_leaf(dir,OsStr::new(DATABASE))?)?!=self.database{return Err(bad("SYSTEM database metadata changed"));}
        Ok(())
    }
}
fn named_mark(dir:&File,local:bool)->io::Result<Stamp>{
    if local{held(&same_mount_leaf(dir,slot())?)}else{stamp(dir,slot())}
}
fn verify_discovery_mark(mark:&Mark,body:&[u8;101],dir:&File)->io::Result<()>{
    let same=||->io::Result<bool>{Ok(held(&mark.fd)?==mark.s&&held(&same_mount_leaf(dir,slot())?)?==mark.s)};
    if !same()?||exact_bytes(&mark.fd)?!=*body||!same()?{return Err(bad("recognized discovery marker changed"));}
    Ok(())
}
fn coordinates(body:&[u8;101])->[[u8;16];3]{
    std::array::from_fn(|i|body[21+16*i..37+16*i].try_into().expect("fixed marker"))
}
/// Fixed physical leaf proof of one independently selected approved F.
pub(super) struct OrdinaryMark(Mark);
impl OrdinaryMark{
    pub(super) fn select(dir:&File,uid:u32,wanted:&[u8;101])->io::Result<Self>{
        if wanted[20]!=2{return Err(bad("not an ordinary volume marker"));}
        Ok(Self(Mark::read_original(dir,uid,wanted)?))
    }
    pub(super) fn verify(&self,dir:&File,wanted:&[u8;101])->io::Result<()>{self.0.verify(dir,wanted)}
}

/// Fixed physical leaf proof for only the one committed INIT recycle role.
pub(super) struct RecycleMark(Mark);
impl RecycleMark{
    pub(super) fn select(dir:&File,uid:u32,wanted:&[u8;101])->io::Result<Self>{
        if wanted[20]!=1{return Err(bad("not an original recycle volume marker"));}
        Ok(Self(Mark::read_original(dir,uid,wanted)?))
    }
    pub(super) fn verify(&self,dir:&File,wanted:&[u8;101])->io::Result<()>{self.0.verify(dir,wanted)}
}

/// Retains the original private marker across a short readonly SYSTEM observation.
pub(super) struct SystemMark(Mark);
impl SystemMark {
    pub(super) fn before_sql(root:super::originals::HeldRoot<'_>)->io::Result<Self>{
        Ok(Self(Mark::pin_system(root.dir(),root.uid(),root.local_only())?))
    }
    pub(super) fn verify(&self,root:super::originals::HeldRoot<'_>,wanted:&[u8;101])->io::Result<()> {
        self.0.verify_system(root.dir(),wanted,root.local_only())
    }
    pub(super) fn observe(&self,root:super::originals::HeldRoot<'_>,wanted:&[u8;101])->io::Result<SystemObservation>{
        self.verify(root,wanted)?;
        let found=SystemObservation::received(same_mount_leaf(root.dir(),slot())?,root.dir(),root.uid())?;
        if found.id!=self.0.s{return Err(bad("original SYSTEM observation marker changed"));}
        self.verify(root,wanted)?;
        Ok(found)
    }
}

/// A non-readable pin to the exact physical marker already verified by the
/// low reader. Neither the marker's token/body nor a readable FD is carried.
pub(super) struct SystemObservation{fd:File,id:Stamp,uid:u32}
impl SystemObservation{
    pub(super) fn received(fd:File,dir:&File,uid:u32)->io::Result<Self>{
        use rustix::{fs::fcntl_getfl,io::{FdFlags,fcntl_getfd}};
        if !fcntl_getfl(&fd)?.contains(OFlags::PATH)||!fcntl_getfd(&fd)?.contains(FdFlags::CLOEXEC){
            return Err(bad("the original SYSTEM observation marker pin must be non-readable"));
        }
        let id=held(&fd)?;
        let this=Self{fd,id,uid};
        this.require(dir)?;
        Ok(this)
    }
    pub(super) fn pin(&self)->&File{&self.fd}
    pub(super) fn require(&self,dir:&File)->io::Result<[u8;64]>{
        let s=&self.id;
        feedback_shape(s,&held(dir)?,self.uid,101)?;
        if held(&self.fd)?!=*s||held(&same_mount_leaf(dir,slot())?)?!=*s{
            return Err(bad("original named SYSTEM observation marker was changed or removed"));
        }
        let mut stamp=[0;64];
        for (x,n) in stamp.as_chunks_mut::<8>().0.iter_mut().zip([s.dev,s.ino,s.mount,s.len,s.ct.0 as u64,u64::from(s.ct.1),
            s.mt.0 as u64,u64::from(s.mt.1)]){*x=n.to_be_bytes();}
        Ok(stamp)
    }
}

#[cfg(test)]
#[test]
fn reattach_marker_pin_rejects_any_read_fd_and_same_bytes_at_a_new_inode(){
    use std::os::unix::fs::PermissionsExt;
    let t=tempfile::tempdir().unwrap();
    let path=t.path().join(slot());
    std::fs::write(&path,[45;101]).unwrap();
    std::fs::set_permissions(&path,std::fs::Permissions::from_mode(0o600)).unwrap();
    let dir=File::open(t.path()).unwrap();
    let uid=rustix::process::geteuid().as_raw();
    assert!(SystemObservation::received(File::open(&path).unwrap(),&dir,uid).is_err());
    let proof=SystemObservation::received(same_mount_leaf(&dir,slot()).unwrap(),&dir,uid).unwrap();
    proof.require(&dir).unwrap();
    let prior=t.path().join("earlier");
    std::fs::rename(&path,&prior).unwrap();
    std::fs::copy(&prior,&path).unwrap();
    assert!(proof.require(&dir).is_err());
}
fn system_body(body:&[u8;101],pool:&[u8;16])->bool{
    body[20]==0&&volume_body(body,pool)
}
fn volume_body(body:&[u8;101],pool:&[u8;16])->bool{
    body[..20]==*b"ZXS-SQLITE-1/VOLUME\0"&&*pool!=[0;16]&&body[21..37]==pool[..]
        &&body[37..53]!=[0;16]&&body[37..53]!=pool[..]&&body[53..69]!=[0;16]&&body[69..]!=[0;32]
}
/// Only after every originally selected file+parent has been synchronized and
/// independently reopened. Keep the actual marker FDs alive through final SQL.
pub(super) struct Synced(Vec<Mark>);
impl Synced{
    pub(super) fn install(g:&Group,db:&super::reservation::Existing<'_>,expected:&[[u8;101]])->io::Result<Self>{
        if expected.len()!=g.dirs().len(){return Err(bad("incomplete approved marker set"));}
        g.recheck()?;let mut existing=Vec::new();let mut gap=false;
        // Inspect every original before even the first recognized write.
        for (i,m) in expected.iter().enumerate(){
            db.sidecars_absent()?;
            if !present(g,i)?{gap=true;existing.push(None);}
            else if gap{return Err(bad("enrollment markers do not form a prefix of this original operation"));}
            else{existing.push(Some(Mark::inspect(g,i,m)?));}
        }
        let mut kept=Vec::new();
        for(i,(prior,m))in existing.into_iter().zip(expected).enumerate(){
            db.sidecars_absent()?;
            if present(g,i)?!=prior.is_some(){return Err(bad("fixed slot changed after group admission; do not adopt a race"));}
            let f=match prior{Some(f)=>f,None=>Mark::create(g,i,m)?};kept.push(f.sync(g,i,m)?);
        }
        let done=Self(kept);db.sidecars_absent()?;done.verify_clean(g,expected)?;Ok(done)
    }
    pub(super) fn verify_clean(&self,g:&Group,expected:&[[u8;101]])->io::Result<()>{
        g.scan_names(0,&[DATABASE.as_bytes(),MARKER])?;
        self.verify_remaining(g,expected)
    }
    // Only between this connection's completed UPDATE RETURNING and COMMIT.
    // Just before UPDATE S had exactly DB + its own marker. SQLite alone owns
    // its rollback journal in this protected, exclusively held directory.
    // Reject any *other* name; never open, attest by name, clean, or adopt a
    // journal. After native close, even that spelling is no longer allowed.
    pub(super) fn verify_committing(&self,g:&Group,tx:&rusqlite::Transaction<'_>,expected:&[[u8;101]])->io::Result<()>{
        if tx.is_autocommit(){return Err(bad("not completing the expected active SQLite transaction"));}
        let names=g.controls(0,&[DATABASE.as_bytes(),MARKER,b"SYSTEM.sqlite3-journal"])?;
        if !names[0]||!names[1]{return Err(bad("original SYSTEM database or marker missing before COMMIT"));}
        self.verify_remaining(g,expected)
    }
    fn verify_remaining(&self,g:&Group,expected:&[[u8;101]])->io::Result<()>{
        g.recheck()?;
        if self.0.len()!=expected.len()||expected.len()!=g.dirs().len(){return Err(bad("incomplete synchronized originals"));}
        // No raw opens or closes for marker verification in the SQL transaction:
        // statx the names, read the already-held original descriptors.
        for(i,((f,bytes),dir))in self.0.iter().zip(expected).zip(g.dirs()).enumerate(){
            if i!=0{g.scan_names(i,&[MARKER])?;}
            f.verify(dir,bytes)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod live_web_tests{
    use super::*;
    #[test]
    fn a_same_directory_with_another_reported_pool_does_not_match_the_original_mark(){
        let raw=crate::sqlite_install::catalog::claimed_marker(0,&[1;16],&[2;16],&[3;16],&[4;32]);
        assert!(system_body(&raw,&[1;16]));
        assert!(!system_body(&raw,&[5;16]));
        for at in [0,20,37,53,69]{
            let mut wrong=raw;
            match at{0|20=>wrong[at]^=1,37|53=>wrong[at..at+16].fill(0),_=>wrong[at..].fill(0)}
            assert!(!system_body(&wrong,&[1;16]));
        }
    }
}
