//! Real registered native fixtures and the typed business API. This is not a
//! browser, an authenticated remote user, an SMB client or a throughput test.
use std::{fs,io::{self,Cursor},path::Path};
use zxs::{DirectoryEntry,DirectoryParent,Entity,FileInfo,FormalNode,OperationId,PublishedVersion};
#[path="unit/support.rs"] mod support;

const ABC:&str="ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
const HELLO:&str="2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
fn op()->OperationId{OperationId::new().unwrap()}
fn seed(sys:&Path,parent:DirectoryParent,name:&str,bytes:&[u8],sha:&str)->(FileInfo,FormalNode){
    let created=zxs::create_empty_at(sys,op(),parent,name).unwrap();
    zxs::save_first(sys,op(),created.file,zxs::DraftClaim::from_hex(bytes.len() as u64,sha).unwrap(),Cursor::new(bytes)).unwrap();
    let node=zxs::commit_working(sys,op(),created.file,1).unwrap();
    let row=zxs::list_directory(sys,parent).unwrap().entries.into_iter().find_map(|e|match e{
        DirectoryEntry::File(f) if f.name==name=>Some(f),_=>None,
    }).unwrap();assert_eq!(row.file,created.file);(row,node)
}
fn part(v:&PublishedVersion,offset:u64,count:u64)->Vec<u8>{
    let mut out=Vec::new();v.read_range_to(offset,count,&mut out).unwrap();out
}
#[test]
#[ignore="private native roots, actual original handles, separately provisioned recycle filesystem; no protocol service"]
fn browse_one_parent_then_reuse_only_the_explicit_original_published_version(){
    let home=support::PrivateTemp::under(&std::env::temp_dir());
    let other=support::PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let sys=home.child("system");let data=home.child("data");let bin=other.child("recycle");
    let reg=zxs::initialize(op(),&sys,&bin,std::slice::from_ref(&data)).unwrap();
    let volume=reg.volumes.iter().find(|v|v.volume.path==data).unwrap().id;
    let root=DirectoryParent::Root(volume);
    let left=DirectoryParent::Child(volume,zxs::make_directory(&sys,op(),root,"left").unwrap().directory);
    let right=DirectoryParent::Child(volume,zxs::make_directory(&sys,op(),root,"right").unwrap().directory);
    let (row,f0)=seed(&sys,left,"Report.xlsx",b"abc",ABC);
    let (other_case,lower)=seed(&sys,left,"report.xlsx",b"hello",HELLO);
    let (sibling,sibling_node)=seed(&sys,right,"Report.xlsx",b"hello",HELLO);
    let foreign=zxs::PoolId::new().unwrap();
    assert!(PublishedVersion::open_listed(&sys,foreign,left,&row,f0.node).is_err());
    assert!(PublishedVersion::open_listed(&sys,reg.id,right,&row,f0.node).is_err());
    assert!(PublishedVersion::open_listed(&sys,reg.id,left,&sibling,sibling_node.node).is_err());
    assert!(PublishedVersion::open_listed(&sys,reg.id,left,&row,sibling_node.node).is_err());
    assert!(PublishedVersion::open_listed(&sys,reg.id,left,&row,lower.node).is_err());
    let mut altered=row.clone();altered.entry=sibling.entry;
    assert!(PublishedVersion::open_listed(&sys,reg.id,left,&altered,f0.node).is_err());
    altered=row.clone();altered.name="REPORT.xlsx".into();
    assert!(PublishedVersion::open_listed(&sys,reg.id,left,&altered,f0.node).is_err());
    let v=PublishedVersion::open_listed(&sys,reg.id,left,&row,f0.node).unwrap();
    let other_v=PublishedVersion::open_listed(&sys,reg.id,left,&other_case,lower.node).unwrap();
    assert_eq!(v.receipt(),&f0);assert_eq!(part(&v,2,1),b"c");assert_eq!(part(&other_v,1,3),b"ell");drop(other_v);
    // A genuine new decision is allowed between two reads on the same open.
    // The original F0 remains F0; it is not an alias for current contents.
    let abc=zxs::DraftClaim::from_hex(3,ABC).unwrap();
    zxs::save_interval(&sys,op(),row.file,1,zxs::SpanEdit::new(1,abc,zxs::LengthPolicy::KeepOrGrow).unwrap(),Cursor::new(b"abc")).unwrap();
    let i1=zxs::commit_working(&sys,op(),row.file,2).unwrap();
    let fresh=PublishedVersion::open_listed(&sys,reg.id,left,&row,i1.node).unwrap();
    assert_eq!(fresh.receipt(),&i1);assert_eq!(part(&fresh,0,4),b"aabc");drop(fresh);
    assert_eq!(part(&v,0,2),b"ab");assert_eq!(part(&v,3,0),b"");let mut denied=Vec::new();
    assert!(v.read_range_to(3,1,&mut denied).is_err());assert!(denied.is_empty());
    assert!(v.read_range_to(u64::MAX,2,&mut denied).is_err());assert!(denied.is_empty());
    // The actual data root's old name has vanished, not its held FDs. A
    // fresh open cannot select a replacement while the old handle stays fixed.
    let parked=home.0.join("original-data");fs::rename(&data,&parked).unwrap();let _replacement=home.child("data");
    assert!(PublishedVersion::open_listed(&sys,reg.id,left,&row,f0.node).is_err());
    let mut all=Vec::new();v.read_to(&mut all).unwrap();assert_eq!(all,b"abc");
    let original=fs::File::open(sys.join("REGISTRATION")).unwrap();
    assert_eq!(io::Error::from(original.try_lock().unwrap_err()).kind(),io::ErrorKind::WouldBlock);
    drop(v);original.try_lock().unwrap();original.unlock().unwrap();
}
