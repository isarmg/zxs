//! Original member-scoped handoff worker. Native CLOSE or same-tree R
//! SET_INFO supplies the request; here a current SQL boundary and physical
//! source, verified target and exact original receipt decide its outcome.
use super::{InitialReader,Site,SmbSession,Tree,TreeOrigin,denied,live,os};
use crate::system::reader::{smb::file_transfer::Handoff,file_location::Place,account::share_mode::{Mode,Setting}};
use crate::{OperationId,Role,display,hash::Sha256};
use std::{io,os::fd::{AsRawFd,FromRawFd,OwnedFd},sync::atomic::{AtomicUsize,Ordering}};

static ACTIVE:AtomicUsize=AtomicUsize::new(0);
pub(super) struct Permit;
impl Permit{
    pub(super) fn take()->Option<Self>{
        ACTIVE.fetch_update(Ordering::AcqRel,Ordering::Acquire,|n|(n<8).then_some(n+1)).ok()?;
        Some(Self)
    }
}
impl Drop for Permit{fn drop(&mut self){ACTIVE.fetch_sub(1,Ordering::AcqRel);}}

unsafe extern "C"{
    fn zxs_member_xfer_pending(member:i32)->i32;
    fn zxs_xfer_take(member:i32,worker:i32,kind:*mut u8,folder:*mut u8,name:*mut u8,mark:*mut u8)->i32;
    fn zxs_xfer_reply(fd:i32,status:u8,file:*const u8,op:*const u8)->i32;
    fn zxs_member_held(member:i32)->i32;
}
pub(super) enum Request{Delete([u8;16]),Mark,Restore{folder:String,name:String}}
pub(super) fn pending(member:i32)->i32{unsafe{zxs_member_xfer_pending(member)}}
pub(super) fn take(member:i32,origin:&TreeOrigin)->io::Result<(Request,OwnedFd)>{
    let(mut kind,mut folder,mut name,mut mark)=(0,[0u8;256],[0u8;223],[0u8;16]);
    let fd=unsafe{zxs_xfer_take(member,origin.worker.as_raw_fd(),&mut kind,folder.as_mut_ptr(),name.as_mut_ptr(),mark.as_mut_ptr())};
    os(fd)?;
    let answer=unsafe{OwnedFd::from_raw_fd(fd)};
    let f=usize::from(folder[0]);let n=usize::from(name[0]);
    let ask=match kind{
        1 if f==0&&n==0&&mark!=[0;16]=>Request::Delete(mark),
        3 if f==0&&n==0=>Request::Mark,
        2 if f>0&&n>0&&f<=255&&n<=222=>Request::Restore{
            folder:display::component(&folder[1..1+f],255)?.to_owned(),
            name:display::component(&name[1..1+n],222)?.to_owned(),
        },
        _=>return Err(denied()),
    };
    Ok((ask,answer))
}
pub(super) fn refuse(answer:OwnedFd,why:u8){let _=unsafe{zxs_xfer_reply(answer.as_raw_fd(),why,std::ptr::null(),std::ptr::null())};}

fn mark_for(auth:&SmbSession<'_>,source:&Place,wanted:Setting,nonce:[u8;8])->io::Result<OperationId>{
    if nonce==[0;8]{return Err(denied());}
    let mut h=Sha256::new();h.update(b"ZXS-NATIVE/DISPOSITION-1\0")?;
    // The stamp is private to the already pidfd-authorized member; it is
    // not a new login or on-wire capability. Its nonce distinguishes a
    // re-mark of the same link from an older still-closing native FSP.
    for v in [auth.source.pool.0,auth.original.instance,auth.original.account.0,auth.original.op.0,source.file.0,source.op.0,
        source.volume.0,wanted.share.0,wanted.op.0]{h.update(&v)?;}
    h.update(&auth.original.generation.to_be_bytes())?;h.update(&source.generation.to_be_bytes())?;
    h.update(&nonce)?;
    let mut stamp=[0u8;16];stamp[..8].copy_from_slice(&nonce);stamp[8..].copy_from_slice(&h.finish()[..8]);
    Ok(OperationId(stamp))
}
pub(super) fn run(site:Site<'_>,origin:&TreeOrigin,branch:&OwnedFd,answer:OwnedFd,request:Request,source:Place,
    ordinary:Option<Setting>,_permit:Permit){
    let still=||->io::Result<()>{
        if (site.stopping)()||!live(origin,branch,0)||unsafe{zxs_member_held(answer.as_raw_fd())}!=0{return Err(denied());}
        Ok(())
    };
    let result=(||{
        still()?;site.validate()?;
        let src=InitialReader::alongside(site.dir,site.mark,site.pool,site.shape)?;
        let auth=SmbSession{source:&src,original:origin.slot.original.clone()};
        let (tree,ask)=match request{
            Request::Mark if source.role==Role::File=>{
                let expected=ordinary.ok_or_else(denied)?;
                let tree=auth.connect_id(expected.share)?;
                if tree.setting!=expected||expected.mode!=Mode::ReadWrite{return Err(denied());}
                let member=tree.open_current(&source.name)?;
                if *member.native_placement()!=source{return Err(denied());}
                member.native_key()?;still()?;
                // Proves the current original source to native disposition;
                // it does not issue an Op or touch the retained filesystem.
                let nonce=OperationId::new()?.0[..8].try_into().expect("disposition nonce width");
                return Ok((source.file,mark_for(&auth,&source,expected,nonce)?));
            },
            Request::Delete(mark) if source.role==Role::File=>{
                let expected=ordinary.ok_or_else(denied)?;
                let tree=auth.connect_id(expected.share)?;
                let nonce=mark[..8].try_into().expect("disposition nonce width");
                if tree.setting!=expected||mark_for(&auth,&source,expected,nonce)?.0!=mark{return Err(denied());}
                (tree,Handoff::Delete(source.clone()))
            },
            Request::Restore{folder,name} if source.role==Role::Recycle&&ordinary.is_none()=>{
                let setting=src.at(|db,c|{
                    auth.require_current(db,&c)?;
                    site.plan.current(db,&c,&folder)?.filter(|g|g.mode==Mode::ReadWrite).ok_or_else(denied)
                })?;
                let tree=Tree{session:&auth,setting};
                (tree,Handoff::RestoreBound{source:source.clone(),target:name})
            },
            _=>return Err(denied()),
        };
        let op=OperationId::new()?;
        let moved=match ask{
            Handoff::Delete(source)=>tree.transfer_last_close(source,op,&still)?,
            other=>tree.transfer(&other,op,&still)?,
        };
        if moved.from!=source{return Err(io::ErrorKind::ConnectionAborted.into());}
        Ok((moved.to.file,moved.op))
    })();
    if still().is_err()||site.validate().is_err(){return;}
    match result{
        Ok((file,op))=>{let _=unsafe{zxs_xfer_reply(answer.as_raw_fd(),0,file.0.as_ptr(),op.0.as_ptr())};}
        Err(e)=>refuse(answer,match e.kind(){
            io::ErrorKind::WouldBlock=>1,io::ErrorKind::StorageFull=>2,io::ErrorKind::AlreadyExists=>3,
            io::ErrorKind::NotFound=>4,io::ErrorKind::NotConnected=>6,io::ErrorKind::ConnectionAborted=>7,_=>5,
        }),
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    use crate::{system::reader::tests::transfer_pool,test_support::PrivateTemp};
    use std::path::Path;

    #[test]
    #[ignore="uses two true storage roots and independent SQLite connections, not an SMB service"]
    fn disposition_proofs_cannot_be_replayed_as_a_fresh_mark_or_after_regrant(){
        let private=PrivateTemp::under(Path::new("/tmp"));let other=PrivateTemp::under(Path::new("/dev/shm"));
        let (path,pool,_,share)=transfer_pool(&private,&other.0.join("actual-R"));
        let source=InitialReader::open(&path,pool).unwrap();let db=InitialReader::open(&path,pool).unwrap();
        let auth=SmbSession::fixture_authenticated(&source,"files.smb").unwrap();
        let tree=auth.connect(share).unwrap();
        let file=tree.create_file(OperationId([0xcb;16]),"to-retain.txt").unwrap().file;
        let place=source.at(|c,m|Place::current(c,&m,file)).unwrap();
        let old_read=tree.open_current("to-retain.txt").unwrap();
        let mark=mark_for(&auth,&place,tree.setting,[1;8]).unwrap();
        // A second FSP's newer mark cannot be cleared as though it were
        // the abandoned older FSP's marker (even for the same actor).
        assert_ne!(mark,mark_for(&auth,&place,tree.setting,[2;8]).unwrap());
        let mut other_place=place.clone();other_place.generation+=1;
        assert_ne!(mark,mark_for(&auth,&other_place,tree.setting,[1;8]).unwrap());
        let admin=db.authenticate_web("first.admin","correct horse battery staple").unwrap();
        admin.set_share_mode(OperationId([0xcc;16]),share,tree.setting.op,Mode::Disabled).unwrap();
        admin.set_share_mode(OperationId([0xcd;16]),share,OperationId([0xcc;16]),Mode::ReadWrite).unwrap();
        let reset=auth.connect(share).unwrap();
        assert_ne!(mark,mark_for(&auth,&place,reset.setting,[1;8]).unwrap());
        assert!(old_read.native_key().is_err());
        db.at(|db,_|{
            let q=db.prepare(c"SELECT count(*) FROM main.operation WHERE kind=12")?;
            assert!(crate::system::reader::work_record::start_one(&q)?);assert_eq!(q.integer(0)?,0);q.done()
        }).unwrap();
    }
}
