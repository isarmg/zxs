use super::*;
const FIXTURE:&str=include_str!("../vectors/maintenance-genesis-v1.txt");
fn raw(key:&str)->Vec<u8>{
    let hex=FIXTURE.lines().find_map(|s|s.strip_prefix(key).and_then(|s|s.strip_prefix('='))).unwrap();
    assert_eq!(hex.len()%2,0);(0..hex.len()).step_by(2).map(|i|u8::from_str_radix(&hex[i..i+2],16).unwrap()).collect()
}
struct Sample{image:Vec<u8>,lower:Vec<u8>,labels:Vec<Vec<u8>>}
impl Sample{
    fn load()->Self{Self{image:raw("genesis"),lower:raw("lower_bound"),
        labels:["label_system","label_recycle","label_file_0","label_file_1"].map(raw).to_vec()}}
    fn read(&self,uid:u32)->io::Result<PoolInfo>{
        read_empty(&self.image,uid,&self.lower,&self.labels.iter().map(Vec::as_slice).collect::<Vec<_>>())
    }
    // These are self-consistent changed byte samples; rejection must not be
    // an incidental mismatch with the old checksum, SYSTEM label or floor.
    fn recalculate(&mut self){
        let end=self.image.len()-32;let h=crate::hash::digest(&self.image[..end]).unwrap();
        self.image[end..].copy_from_slice(&h);self.lower[16..].copy_from_slice(&h);
        let off=self.labels[0].len()-32;self.labels[0][off..].copy_from_slice(&h);
    }
}
#[test]
fn the_fixed_four_root_image_and_its_native_byte_paths_are_not_a_legacy_business_genesis(){
    let t=Sample::load();assert_eq!(t.image.len(),423);assert_eq!(&t.image[391..],raw("sha256_prefix").as_slice());
    let view=t.read(1001).unwrap();assert_eq!(view.id.0,[0x11;16]);
    assert_eq!(view.initialized_by.0,[0x22;16]);assert_eq!(view.volumes.len(),4);
    assert_eq!(view.volumes[3].volume.role,Role::File);assert_eq!(view.volumes[3].volume.path.as_os_str().as_bytes(),b"/m/\xffvol");
    assert!(Record::prefix(&t.image).is_err());assert!(t.read(0).is_err());
    let mut wrong=t;wrong.image.splice(..GENESIS.len(),MAGIC.iter().copied());wrong.recalculate();
    assert!(wrong.read(1001).is_err());
}
#[test]
fn a_rehashed_alias_or_nested_root_cannot_be_an_empty_registration(){
    for swap in 0..5{
        let mut t=Sample::load();
        // Represented rows begin at 105,176,247,319 (fixed literal vector);
        // an Entity begins 17 bytes in, its 28B Birth content 34 bytes in.
        match swap{
            0=>t.image.copy_within(58..74,105+17), // SYSTEM claims register's Entity
            1=>t.image.copy_within(75..103,105+34),// SYSTEM claims register's Birth
            2=>t.image.copy_within(247+34..247+62,319+34),// two F alias by handle
            3=>{ // /m/sys is an actual component ancestor of /m/sys/x.
                t.image.splice(319+63..391,[0,8,b'/',b'm',b'/',b's',b'y',b's',b'/',b'x']);
            },
            _=>t.image[391-7..391].copy_from_slice(b"/m/data"), // duplicate original path
        }
        t.recalculate();assert!(t.read(1001).is_err());
    }
    let mut ok=Sample::load();ok.image[391-7..391].copy_from_slice(b"/m/sysx");ok.recalculate();
    assert!(ok.read(1001).is_ok()); // lexical prefix only: distinct sibling
}
#[test]
fn one_checksum_does_not_explain_another_role_frontier_or_trailing_frame(){
    let mut t=Sample::load();t.labels.swap(1,2);assert!(t.read(1001).is_err());
    let mut t=Sample::load();t.labels[0].pop();assert!(t.read(1001).is_err());
    let mut t=Sample::load();t.lower[7]=1;assert!(t.read(1001).is_err());
    let mut t=Sample::load();t.image[54]^=1;assert!(t.read(1001).is_err());
    let mut t=Sample::load();t.image.push(0);assert!(t.read(1001).is_err());
}

/// Native test-only issuer. Its caller must release the original EX before
/// attempting a legitimate cold reader; production has no such writer here.
pub(super) fn register_fixture(s:&Selection)->(Record,std::fs::File,std::fs::File){
    use crate::pool::{PENDING,READY};
    let root=&s.roots[0].1;let(file,who)=root.anonymous_record().unwrap();file.try_lock().unwrap();
    // All native fields below are from the fresh held files, not the public
    // synthetic Linux numbers. Assemble the MAINT bytes before publishing either name.
    let layout=Record::new(s,OperationId::new().unwrap(),who,fs::Birth::read(&file).unwrap()).unwrap();
    let mut bytes=GENESIS.to_vec();bytes.extend_from_slice(&layout.bytes[MAGIC.len()..layout.bytes.len()-32]);
    let sum=crate::hash::digest(&bytes).unwrap();bytes.extend_from_slice(&sum);
    let rec=decode_empty(&bytes,fs::local_uid(),&fs::frontier(0,0,sum)).unwrap();
    fs::initial_frontier(&file,sum).unwrap();root.add_record(file.try_clone().unwrap(),PENDING,&bytes).unwrap();
    let other=std::fs::File::open(root.path.join("INITIALIZING")).unwrap();
    assert_eq!(io::Error::from(other.try_lock_shared().unwrap_err()).kind(),io::ErrorKind::WouldBlock);
    for(i,(_,d))in s.roots.iter().enumerate(){d.claim(&rec.label_in_domain(i,LABEL)).unwrap();}
    root.rename_registration(PENDING,READY).unwrap();root.file.sync_all().unwrap();
    assert_eq!(io::Error::from(other.try_lock_shared().unwrap_err()).kind(),io::ErrorKind::WouldBlock);
    (rec,file,other)
}

/// Emit a fresh synthetic *native* fixture at the selected original FD. It is
/// not a product writer, never relabels ID-LEAF-1 media, and does not prove
/// another implementation has written a usable pool. The public hex vector
/// above is the independent byte-level reference.
#[test]
#[ignore="two private Linux filesystems with export handles and native UUID"]
fn online_empty_keeps_the_original_regular_flock_not_the_directory_short_lock(){
    use crate::test_support::PrivateTemp;
    use crate::pool::READY;
    use std::fs as osfs;
    let a=PrivateTemp::under(&std::env::temp_dir());
    let b=PrivateTemp::under(Path::new(&std::env::var_os("ZXS_DISTINCT_TEST_TMPDIR").unwrap()));
    let sys=a.child("system");let data=a.child("data");let bin=b.child("recycle");
    let s=Selection::open(&sys,&bin,std::slice::from_ref(&data)).unwrap();
    let root=&s.roots[0].1;let(rec,file,other)=register_fixture(&s);
    let sum=rec.digest();let bytes=&rec.bytes;
    assert!(native::OnlineEmpty::open(&sys).is_err());drop(file);
    let first=native::OnlineEmpty::open(&sys).unwrap();
    let second=native::OnlineEmpty::open(&sys).unwrap();
    assert_eq!(io::Error::from(other.try_lock().unwrap_err()).kind(),io::ErrorKind::WouldBlock);
    // An idle online reader does NOT monopolize SYSTEM's short transaction lock.
    let short=HeldDir::open(&sys).unwrap();short.exclusive().unwrap();drop(short);
    let offline=bin.with_file_name("fixture-recycle-offline");osfs::rename(&bin,&offline).unwrap();
    assert_eq!(first.observe(rec.info.volumes[2].id).unwrap(),rec.info.volumes[2]);
    assert!(first.observe(rec.info.volumes[1].id).is_err());
    assert!(crate::inspect(&sys).is_err(),"old format is never selected for new media");
    // Substitute equal bytes and even the same frontier on another inode, with
    // the original lock still alive elsewhere. Neither cold nor held view accepts it.
    let original=sys.with_file_name("retained-original-registration");osfs::rename(sys.join("REGISTRATION"),&original).unwrap();
    let(impostor,_)=root.anonymous_record().unwrap();fs::initial_frontier(&impostor,sum).unwrap();
    root.add_record(impostor,READY,bytes).unwrap();
    assert!(first.observe(rec.info.volumes[2].id).is_err());assert!(native::OnlineEmpty::open(&sys).is_err());
    osfs::rename(sys.join("REGISTRATION"),sys.with_file_name("kept-impostor")).unwrap();
    osfs::rename(original,sys.join("REGISTRATION")).unwrap();
    drop(second);assert_eq!(io::Error::from(other.try_lock().unwrap_err()).kind(),io::ErrorKind::WouldBlock);
    drop(first);other.try_lock().unwrap();assert!(native::OnlineEmpty::open(&sys).is_err());
    drop(other);assert!(native::OnlineEmpty::open(&sys).is_ok());osfs::rename(offline,bin).unwrap();
}
