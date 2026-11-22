// No setup commands, existing media, machine credentials or anonymous root authority.
// Each physical fixture must be explicitly provisioned for the unprivileged test process.
use super::*;
use crate::sqlite_install::originals::Originals;
use rustix::fs::{flock,FlockOperation};
use std::{fs::{self,File,OpenOptions},os::unix::fs::{MetadataExt,OpenOptionsExt,PermissionsExt},path::PathBuf};
use tempfile::{Builder,TempDir};

struct Area{_f:TempDir,_r:TempDir,roots:Vec<PathBuf>,uid:u32}
impl Area{
    fn new()->Self{
        fn child(key:&str)->TempDir{
            let parent=std::env::var_os(key).expect("provision dedicated local test parents on two independent filesystems");
            let out=Builder::new().prefix("helper-private-sql-init-").tempdir_in(parent).unwrap();
            fs::set_permissions(out.path(),fs::Permissions::from_mode(0o700)).unwrap();out
        }
        let f=child("HELPER_TEST_PORTABLE_DATA_PARENT");let r=child("HELPER_TEST_PORTABLE_RECYCLE_PARENT");
        let roots=vec![f.path().join("system"),r.path().join("recycle"),f.path().join("files")];
        for p in &roots {fs::create_dir(p).unwrap();fs::set_permissions(p,fs::Permissions::from_mode(0o700)).unwrap();}
        let uid=rustix::process::getuid().as_raw();assert!(uid>0,"run as the explicitly provisioned unprivileged worker");
        Self{_f:f,_r:r,roots,uid}
    }
    fn request(&self)->Request{Request::new([1;16],[2;16],[3;16],self.uid,self.roots.clone()).unwrap()}
    fn pin(&self,r:&Request)->Originals{Originals::pin(r,File::open("/").unwrap()).unwrap()}
    fn db(&self)->PathBuf{self.roots[0].join(super::super::DATABASE)}
}
#[test]
fn pending_sql_is_real_and_never_activates_or_writes_a_volume_marker(){
    let a=Area::new();let r=a.request();
    let g=a.pin(&r).lock(a.uid).unwrap();let other=File::open(&a.roots[0]).unwrap();
    assert_eq!(flock(&other,FlockOperation::NonBlockingLockShared).unwrap_err(),rustix::io::Errno::WOULDBLOCK);
    let result=first(&r,&g,&Issue::new(3).unwrap()).unwrap();assert_eq!(result.op,r.op());assert_ne!(result.pool,[0;16]);
    let meta=fs::symlink_metadata(a.db()).unwrap();assert!(meta.is_file()&&meta.size()>0);assert_eq!(meta.mode()&0o7777,0o600);
    assert!(a.roots.iter().all(|d|!d.join(".zxs-new-ffffffffffffffffffffffffffffffff-0").try_exists().unwrap()));
    assert!(a.roots[1..].iter().all(|d|fs::read_dir(d).unwrap().next().is_none()));
    assert_eq!(flock(&other,FlockOperation::NonBlockingLockShared).unwrap_err(),rustix::io::Errno::WOULDBLOCK);
    drop(g);flock(&other,FlockOperation::NonBlockingLockShared).unwrap();
    let db=rusqlite::Connection::open_with_flags_and_vfs(a.db(),rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY|rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,c"unix").unwrap();
    let state:i64=db.query_row("SELECT state FROM operation WHERE kind=1",[],|r|r.get(0)).unwrap();
    let count:i64=db.query_row("SELECT count(*) FROM volume_claim",[],|r|r.get(0)).unwrap();
    let pair:i64=db.query_row("SELECT count(*) FROM account_pair",[],|r|r.get(0)).unwrap();
    assert_eq!((state,count,pair),(0,3,0));db.close().unwrap();
}
#[test]
fn any_busy_or_nonempty_original_leaves_siblings_without_a_new_db(){
    let a=Area::new();let req=a.request();let blocker=File::open(&a.roots[2]).unwrap();
    flock(&blocker,FlockOperation::NonBlockingLockShared).unwrap();assert!(a.pin(&req).lock(a.uid).is_err());
    assert!(!a.db().try_exists().unwrap());drop(blocker);
    fs::write(a.roots[1].join("foreign"),b"untouched").unwrap();let group=a.pin(&req).lock(a.uid).unwrap();
    assert!(first(&req,&group,&Issue::new(3).unwrap()).is_err());assert!(!a.db().try_exists().unwrap());
    assert_eq!(&fs::read(a.roots[1].join("foreign")).unwrap(),b"untouched");
}
#[test]
fn occupied_database_is_preserved_even_if_it_is_a_zero_length_private_regular_file(){
    let a=Area::new();let r=a.request();let g=a.pin(&r).lock(a.uid).unwrap();
    let original=OpenOptions::new().create_new(true).write(true).mode(0o600).open(a.db()).unwrap();
    let m=original.metadata().unwrap();drop(original);
    assert!(first(&r,&g,&Issue::new(3).unwrap()).is_err());
    let after=fs::symlink_metadata(a.db()).unwrap();assert_eq!((m.dev(),m.ino(),m.len()),(after.dev(),after.ino(),after.len()));
}

#[test]
fn vfs_must_not_open_a_replaced_parent_or_a_late_unattributed_journal(){
    let a=Area::new();let request=a.request();let group=a.pin(&request).lock(a.uid).unwrap();
    let reserved=Reserved::create(&group,&request).unwrap();
    let journal=a.roots[0].join("SYSTEM.sqlite3-journal");fs::write(&journal,b"foreign").unwrap();
    assert!(engine::open_reserved(&reserved).is_err());assert_eq!(fs::read(&journal).unwrap().as_slice(),b"foreign");
    fs::remove_file(&journal).unwrap();reserved.pristine().unwrap();
    let moved=a._f.path().join("moved-system");fs::rename(&a.roots[0],&moved).unwrap();
    fs::create_dir(&a.roots[0]).unwrap();fs::set_permissions(&a.roots[0],fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(a.db(),b"replacement is never opened").unwrap();
    assert!(engine::open_reserved(&reserved).is_err());
    assert_eq!(fs::read(a.db()).unwrap().as_slice(),b"replacement is never opened");
    assert_eq!(fs::metadata(moved.join(super::super::DATABASE)).unwrap().len(),0);
}

#[test]
fn empty_old_attribute_on_selected_leaf_or_ancestor_never_creates_a_new_db(){
    use rustix::fs::{fgetxattr,fsetxattr,XattrFlags};
    for ancestor in [false,true]{
        let a=Area::new();let r=a.request();
        let path=if ancestor {a._f.path()}else{a.roots[2].as_path()};
        let before=File::open(path).unwrap();
        fsetxattr(&before,c"user.zxs.volume",&[],XattrFlags::CREATE)
            .expect("this private fixture explicitly needs an observable user attribute namespace");
        let err=a.pin(&r).lock(a.uid).err().expect("having no file entries must not hide a volume attribute");
        assert_eq!(err.to_string(),"root or ancestor has an existing volume attribute; no takeover");
        assert!(!a.db().try_exists().unwrap());
        let mut buf=[0u8;1];assert_eq!(fgetxattr(&before,c"user.zxs.volume",&mut buf[..]).unwrap(),0);
    }
}

fn deterministic_issue()->Issue{Issue{pool:[4;16],rows:(0..3).map(|i|([0x11+i;16],[0x21+i;32])).collect()}}
fn expected_marker(i:usize)->Vec<u8>{
    // Independent literal of the public SQL protocol, not the production encoder.
    let mut b=b"ZXS-SQLITE-1/VOLUME\0".to_vec();b.push([0,1,2][i]);b.extend([4;16]);
    b.extend([0x11+i as u8;16]);b.extend([1;16]);b.extend([0x21+i as u8;32]);b
}
fn mark_path(a:&Area,i:usize)->PathBuf{a.roots[i].join(std::ffi::OsStr::from_bytes(super::super::MARKER))}
fn private_contents(p:&std::path::Path,b:&[u8]){
    let mut f=OpenOptions::new().write(true).create_new(true).mode(0o600).open(p).unwrap();
    std::io::Write::write_all(&mut f,b).unwrap();f.sync_all().unwrap();
    File::open(p.parent().unwrap()).unwrap().sync_all().unwrap();
}
#[test]
fn cold_same_op_finishes_partially_labeled_roots_and_retry_does_not_recreate_missing_marker(){
    use super::super::complete;
    let a=Area::new();let r=a.request();let g=a.pin(&r).lock(a.uid).unwrap();
    let pending=first(&r,&g,&deterministic_issue()).unwrap();
    assert_eq!(pending.pool,[4;16]);drop(pending);drop(g);
    private_contents(&mark_path(&a,0),&expected_marker(0));let s=fs::metadata(mark_path(&a,0)).unwrap();
    let mut wrong=r.paths().to_vec();wrong[2]=a._f.path().join("other");
    fs::create_dir(&wrong[2]).unwrap();fs::set_permissions(&wrong[2],fs::Permissions::from_mode(0o700)).unwrap();
    let q=Request::new([1;16],[2;16],[3;16],a.uid,wrong).unwrap();
    let g=a.pin(&q).lock(a.uid).unwrap();assert!(complete::original(&super::super::catalog::Canonical::new().unwrap(),&q,&g,None).is_err());drop(g);
    assert!(!mark_path(&a,1).try_exists().unwrap());
    let g=a.pin(&r).lock(a.uid).unwrap();let other=File::open(&a.roots[0]).unwrap();
    let done=complete::original(&super::super::catalog::Canonical::new().unwrap(),&r,&g,None).unwrap();assert!(!done.previous);assert_eq!(done.pool,[4;16]);
    assert_eq!(flock(&other,FlockOperation::NonBlockingLockShared).unwrap_err(),rustix::io::Errno::WOULDBLOCK);
    for i in 0..3{assert_eq!(fs::read(mark_path(&a,i)).unwrap(),expected_marker(i));}
    assert_eq!(fs::metadata(mark_path(&a,0)).unwrap().ino(),s.ino());drop(g);
    let g=a.pin(&r).lock(a.uid).unwrap();fs::remove_file(mark_path(&a,1)).unwrap();
    let history=complete::original(&super::super::catalog::Canonical::new().unwrap(),&r,&g,None).unwrap();assert!(history.previous);assert_eq!(history.op,r.op());
    assert!(!mark_path(&a,1).try_exists().unwrap());drop(g);flock(&other,FlockOperation::NonBlockingLockShared).unwrap();
}
#[test]
fn unknown_last_marker_or_truncated_first_marker_neither_gets_overwritten_nor_publishes(){
    use super::super::complete;
    for (i,data) in [(2,expected_marker(1)),(0,b"partial".to_vec())]{
        let a=Area::new();let r=a.request();let g=a.pin(&r).lock(a.uid).unwrap();
        let p=first(&r,&g,&deterministic_issue()).unwrap();private_contents(&mark_path(&a,i),&data);
        let m=fs::metadata(mark_path(&a,i)).unwrap();assert!(complete::original(&super::super::catalog::Canonical::new().unwrap(),&r,&g,Some(&p)).is_err());
        assert_eq!(fs::read(mark_path(&a,i)).unwrap(),data);assert_eq!(fs::metadata(mark_path(&a,i)).unwrap().ino(),m.ino());
        for j in (0..3).filter(|&j|j!=i){assert!(!mark_path(&a,j).try_exists().unwrap());}
        let db=rusqlite::Connection::open_with_flags_and_vfs(a.db(),rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY|rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,c"unix").unwrap();
        assert_eq!(db.query_row("SELECT state FROM operation WHERE kind=1",[],|row|row.get::<_,i64>(0)).unwrap(),0);
    }
}
#[test]
fn original_authorization_never_creates_an_absent_db_or_recovers_an_unknown_journal(){
    use super::super::complete;
    let a=Area::new();let r=a.request();let g=a.pin(&r).lock(a.uid).unwrap();
    assert!(complete::original(&super::super::catalog::Canonical::new().unwrap(),&r,&g,None).is_err());assert!(!a.db().try_exists().unwrap());
    let p=first(&r,&g,&deterministic_issue()).unwrap();
    let j=a.roots[0].join("SYSTEM.sqlite3-journal");private_contents(&j,b"leave foreign rollback evidence");
    assert!(complete::original(&super::super::catalog::Canonical::new().unwrap(),&r,&g,Some(&p)).is_err());assert_eq!(fs::read(&j).unwrap(),b"leave foreign rollback evidence");
    assert!(!mark_path(&a,0).try_exists().unwrap());
}
#[test]
fn real_sqlite_schema_for_the_same_version_must_not_have_undeclared_objects(){
    use super::super::complete;
    let a=Area::new();let r=a.request();let g=a.pin(&r).lock(a.uid).unwrap();
    let p=first(&r,&g,&deterministic_issue()).unwrap();
    let db=rusqlite::Connection::open_with_flags_and_vfs(a.db(),rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE|rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,c"unix").unwrap();
    db.execute_batch("CREATE TABLE alien(x BLOB);").unwrap();db.close().unwrap();
    assert!(complete::original(&super::super::catalog::Canonical::new().unwrap(),&r,&g,Some(&p)).is_err());assert!(!mark_path(&a,0).try_exists().unwrap());
}
#[test]
fn unrelated_file_appearing_after_marker_sync_blocks_sql_completion_and_stays_intact(){
    use super::super::{catalog::Canonical,complete,markers::Synced,reservation::Existing};
    let a=Area::new();let r=a.request();let g=a.pin(&r).lock(a.uid).unwrap();
    let p=first(&r,&g,&deterministic_issue()).unwrap();let db=Existing::observe(&g,&r).unwrap();
    let tpl=Canonical::new().unwrap();let claim=complete::snapshot(&db,&tpl,&g,&r).unwrap();
    p.require(&db,&claim,&r).unwrap();let marks=Synced::install(&g,&db,&claim.markers).unwrap();
    let other=a.roots[1].join("belongs-to-operator");private_contents(&other,b"keep me");
    assert!(complete::publish(&db,&tpl,&g,&r,&claim,&marks).is_err());
    assert!(!complete::snapshot(&db,&tpl,&g,&r).unwrap().committed);
    assert_eq!(fs::read(&other).unwrap(),b"keep me");
    for i in 0..3{assert_eq!(fs::read(mark_path(&a,i)).unwrap(),expected_marker(i));}
}

// Keep the fixture media in the same owned parents, but make the original
// path of each external root unavailable. No mounting, raw disk or plugin.
fn parked(a:&Area,park:bool){
    for i in [1,2]{
        let root=&a.roots[i];let away=root.with_extension("parked");
        if park{fs::rename(root,&away).unwrap();assert!(!root.try_exists().unwrap());}
        else{fs::rename(&away,root).unwrap();}
    }
}

#[test]
fn exact_readonly_receipt_distinguishes_pending_and_historical_when_the_two_external_roots_are_absent(){
    use super::super::{catalog::Canonical,complete,originals::LockedSystem,receipt,reservation::Existing};
    let a=Area::new();let r=a.request();let g=a.pin(&r).lock(a.uid).unwrap();
    let initial=first(&r,&g,&deterministic_issue()).unwrap();drop(initial);drop(g);

    parked(&a,true);
    let before=fs::read(a.db()).unwrap();
    let locked=LockedSystem::from_fixture(&a.roots[0]).unwrap();
    let db=Existing::initial_receipt(&locked,&r).unwrap();
    assert!(!db.allows_install_write());assert!(engine::existing(&db,engine::Access::Write).is_err());
    assert_eq!(receipt::read(&locked,&r).unwrap(),receipt::Receipt{pool:[4;16],committed_sha:None});
    assert_eq!(fs::read(a.db()).unwrap(),before);
    assert!(!mark_path(&a,0).try_exists().unwrap());
    for i in [1,2]{assert!(!a.roots[i].try_exists().unwrap());}
    drop(db);drop(locked);parked(&a,false);

    let g=a.pin(&r).lock(a.uid).unwrap();
    assert!(!complete::original(&Canonical::new().unwrap(),&r,&g,None).unwrap().previous);
    drop(g);parked(&a,true);
    let before=fs::read(a.db()).unwrap();
    let lock=LockedSystem::from_fixture(&a.roots[0]).unwrap();
    assert_eq!(receipt::read(&lock,&r).unwrap(),receipt::Receipt{pool:[4;16],committed_sha:Some(r.sha())});
    assert_eq!(fs::read(a.db()).unwrap(),before);
    assert_eq!(fs::read(mark_path(&a,0)).unwrap(),expected_marker(0));
    drop(lock);

    // SQL being committed is not success if this original S marker is gone.
    fs::rename(mark_path(&a,0),a._f.path().join("retained-system-mark")).unwrap();
    let lock=LockedSystem::from_fixture(&a.roots[0]).unwrap();
    assert!(receipt::read(&lock,&r).is_err());
    assert_eq!(fs::read(a._f.path().join("retained-system-mark")).unwrap(),expected_marker(0));
    assert!(!mark_path(&a,0).try_exists().unwrap());
}

#[test]
fn readonly_receipt_refuses_a_different_order_busy_system_wrong_marker_and_unknown_sidecar(){
    use super::super::{originals::LockedSystem,receipt};
    let a=Area::new();let r=a.request();
    {
        let locked=LockedSystem::from_fixture(&a.roots[0]).unwrap();
        assert!(receipt::read(&locked,&r).is_err());assert!(!a.db().try_exists().unwrap());
    }
    let group=a.pin(&r).lock(a.uid).unwrap();let p=first(&r,&group,&deterministic_issue()).unwrap();
    drop(p);drop(group);
    let online=File::open(&a.roots[0]).unwrap();flock(&online,FlockOperation::NonBlockingLockShared).unwrap();
    assert!(LockedSystem::from_fixture(&a.roots[0]).is_err());drop(online);

    parked(&a,true);
    let lock=LockedSystem::from_fixture(&a.roots[0]).unwrap();
    let bad_approval=Request::new(r.op(),r.actor(),[8;16],r.uid(),r.paths().to_vec()).unwrap();
    assert!(receipt::read(&lock,&bad_approval).is_err());
    let mut paths=r.paths().to_vec();paths[2]=a._f.path().join("unrelated");
    let bad_bytes=Request::new(r.op(),r.actor(),r.approval(),r.uid(),paths).unwrap();
    assert!(receipt::read(&lock,&bad_bytes).is_err());
    assert!(!a._f.path().join("unrelated").try_exists().unwrap());

    let j=a.roots[0].join("SYSTEM.sqlite3-journal");private_contents(&j,b"foreign sidecar");
    assert!(receipt::read(&lock,&r).is_err());assert_eq!(fs::read(&j).unwrap(),b"foreign sidecar");
    fs::remove_file(&j).unwrap();
    let substitute=a.roots[0].join(".ZXS-new-ffffffffffffffffffffffffffffffff-0");
    private_contents(&substitute,&expected_marker(0));
    assert!(receipt::read(&lock,&r).is_err());assert_eq!(fs::read(&substitute).unwrap(),expected_marker(0));
    fs::remove_file(&substitute).unwrap();
    let path=mark_path(&a,0);private_contents(&path,&expected_marker(1));
    assert!(receipt::read(&lock,&r).is_err());assert_eq!(fs::read(&path).unwrap(),expected_marker(1));
    fs::remove_file(&path).unwrap();
    private_contents(&path,&expected_marker(0));
    assert_eq!(receipt::read(&lock,&r).unwrap(),receipt::Receipt{pool:[4;16],committed_sha:None});
}
