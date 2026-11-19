//! Read the protected, declared guest files only. Observe foreign names without
//! opening them. No untrusted filesystem is mounted, no binary/import executed.
use super::{Layout,Leaf,Stamp,invalid};
use rustix::fs::{Mode,OFlags,RawDir,openat};
use sha2::{Digest,Sha256};
use std::{collections::{BTreeMap,BTreeSet},fs::File,io::{self,Read},mem::MaybeUninit,os::unix::fs::MetadataExt};

fn pinned(parent:&File,name:&str)->io::Result<File>{
    Ok(File::from(openat(parent,name,OFlags::PATH|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty())?))
}
fn directory(fd:&File)->io::Result<File>{
    Ok(File::from(openat(fd,c".",OFlags::RDONLY|OFlags::DIRECTORY|OFlags::NOFOLLOW|OFlags::NONBLOCK|OFlags::CLOEXEC,Mode::empty())?))
}
fn names(dir:&File,want:&BTreeSet<String>)->io::Result<()>{
    let stream=directory(dir)?;if Stamp::on(&stream)?!=Stamp::on(dir)?{return Err(invalid());}
    let mut seen=BTreeSet::new();let mut buf=[MaybeUninit::uninit();4096];let mut it=RawDir::new(&stream,&mut buf);
    while let Some(entry)=it.next(){
        let entry=entry?;let raw=entry.file_name().to_bytes();if matches!(raw,b"."|b".."){continue;}
        let s=std::str::from_utf8(raw).map_err(|_|invalid())?;
        if !want.contains(s)||!seen.insert(s.to_owned()){return Err(invalid());}
    }if seen!=*want{return Err(invalid());}Ok(())
}
fn exact_content(mut data:impl Read,expected:&Leaf)->io::Result<()>{
    let mut h=Sha256::new();let mut buffer=[0;32*1024];let mut left=expected.size;
    while left>0{let n=left.min(buffer.len()as u64)as usize;data.read_exact(&mut buffer[..n])?;h.update(&buffer[..n]);left-=n as u64;}
    loop{match data.read(&mut buffer[..1]){Ok(0)=>break,Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue,
        Ok(_)=>return Err(invalid()),Err(e)=>return Err(e)}}
    if <[u8;32]>::from(h.finalize())!=expected.sha{return Err(invalid());}Ok(())
}
fn file(pin:&File,row:&Leaf,mark:&Stamp,bytes:bool)->io::Result<()>{
    if mark.mode!=(0o100000|row.mode)||mark.uid!=0||pin.metadata()?.gid()!=0||mark.links!=1||mark.len!=row.size{return Err(invalid());}
    if bytes{
        let mut f=crate::pinned::installed_regular_reader(pin)?;if Stamp::on(&f)?!=*mark{return Err(invalid());}
        exact_content(&mut f,row)?;if Stamp::on(&f)?!=*mark{return Err(invalid());}
    }Ok(())
}
fn visit(dir:&File,at:&str,root:&Stamp,l:&Layout,bytes:bool,samples:&mut BTreeMap<String,Stamp>)->io::Result<()>{
    let stamp=Stamp::on(dir)?;
    if stamp.mount!=root.mount||stamp.entity.dev!=root.entity.dev||stamp.mode!=0o40755||stamp.uid!=0
        ||dir.metadata()?.gid()!=0||stamp.links==0{return Err(invalid());}
    let want=l.dirs.get(at).ok_or_else(invalid)?;names(dir,want)?;
    samples.insert(at.to_owned(),stamp);
    for name in want{
        let path=if at.is_empty(){name.clone()}else{format!("{at}/{name}")};
        let pin=pinned(dir,name)?;let before=Stamp::on(&pin)?;
        if before.mount!=root.mount||before.entity.dev!=root.entity.dev{return Err(invalid());}
        if let Some(leaf)=l.files.get(&path){
            file(&pin,leaf,&before,bytes)?;samples.insert(path,before);
        }else{
            if before.mode!=0o40755||before.uid!=0||pin.metadata()?.gid()!=0{return Err(invalid());}
            let data=directory(&pin)?;if Stamp::on(&data)?!=before{return Err(invalid());}
            visit(&data,&path,root,l,bytes,samples)?;
        }
        if Stamp::on(&pin)?!=before||Stamp::on(&pinned(dir,name)?)?!=before{return Err(invalid());}
    }
    if stamp!=Stamp::on(dir)?{return Err(invalid());}Ok(())
}
pub(super) fn check(dir:&File,layout:&Layout,bytes:bool)->io::Result<BTreeMap<String,Stamp>>{
    let root=Stamp::on(dir)?;let mut all=BTreeMap::new();visit(dir,"",&root,layout,bytes,&mut all)?;Ok(all)
}
#[cfg(test)]mod tests{
    use super::*;use std::fs;
    #[test]fn every_byte_real_eof_and_empty_python_source_are_separate_checks(){
        let row=Leaf{size:3,mode:0o644,sha:Sha256::digest(b"abc").into()};
        exact_content(&b"abc"[..],&row).unwrap();
        for data in [b"ab".as_slice(),b"abcd",b"abx"]{assert!(exact_content(data,&row).is_err());}
        let zero=Leaf{size:0,mode:0o644,sha:Sha256::digest(b"").into()};
        exact_content(&b""[..],&zero).unwrap();assert!(exact_content(&b"x"[..],&zero).is_err());
    }
    // Privileged fixture ONLY in a dedicated scratch local FS; does not
    // execute either file, inspect system software or use a real capsule.
    #[test]
    #[ignore="explicit isolated root-owned package verification fixture"]
    fn same_name_same_size_wrong_bytes_and_unlisted_entries_do_not_survive_actual_full_read(){
        use std::os::unix::fs::PermissionsExt;
        assert!(rustix::process::geteuid().is_root()&&rustix::process::getegid().is_root());
        let parent=std::env::var_os("HELPER_TEST_SAM_PACKAGE_PARENT").expect("dedicated scratch mount");
        let area=tempfile::tempdir_in(parent).unwrap();let p=area.path();
        for sub in ["","bin","lib64"]{fs::create_dir_all(p.join(sub)).unwrap();fs::set_permissions(p.join(sub),fs::Permissions::from_mode(0o755)).unwrap();}
        let line=|n:&str|format!("usr/{n}\t0755\t3\t{:x}\ttest-package\tusr/member\t-\n",Sha256::digest(b"abc"));
        let a="bin/python3.13";let b="lib64/ld-linux-x86-64.so.2";
        for name in [a,b]{fs::write(p.join(name),b"abc").unwrap();fs::set_permissions(p.join(name),fs::Permissions::from_mode(0o755)).unwrap();}
        let layout=Layout::parse(format!("{}\n{}{}",super::super::TITLE,line(a),line(b)).as_bytes()).unwrap();
        let root=File::open(p).unwrap();let first=check(&root,&layout,true).unwrap();
        assert!(check(&root,&layout,false).unwrap()==first);
        fs::write(p.join(a),b"xyz").unwrap();assert!(check(&root,&layout,true).is_err());
        fs::write(p.join(a),b"abc").unwrap();check(&root,&layout,true).unwrap();
        let old=File::open(p.join(b)).unwrap();fs::rename(p.join(b),p.join("unlisted")).unwrap();
        fs::write(p.join(b),b"abc").unwrap();fs::set_permissions(p.join(b),fs::Permissions::from_mode(0o755)).unwrap();
        assert!(check(&root,&layout,false).is_err());fs::remove_file(p.join("unlisted")).unwrap();
        let still=check(&root,&layout,true).unwrap();assert_ne!(still[b].entity,first[b].entity);
        assert_eq!(old.metadata().unwrap().ino(),first[b].entity.ino);
    }
    #[test]fn an_unlisted_fifo_or_late_addition_is_observed_by_name_only(){
        // No data open (a FIFO would block on a blocking read); this test
        // exercises only raw-name completeness, not owner/mount authorization.
        let d=tempfile::tempdir().unwrap();let dir=File::open(d.path()).unwrap();
        fs::write(d.path().join("python"),b"fixture").unwrap();let want=BTreeSet::from(["python".into()]);
        names(&dir,&want).unwrap();rustix::fs::mkfifoat(&dir,c"extra",Mode::RUSR|Mode::WUSR).unwrap();
        assert!(names(&dir,&want).is_err());assert!(d.path().join("extra").exists());
    }
}
