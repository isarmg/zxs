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
