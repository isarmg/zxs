//! One foreground local administrator authorizes one already-pinned set. No
//! browser, environment, SQL row, plain UID argument or saved grant can issue it.
use super::{originals::Originals,request::{Id,Request}};
use crate::{disk_web::require_terminal,fs_tools::RuntimeFile,maintenance::{diagnostic_worker,thread_file},pinned::{Entity,Mount}};
use rustix::{fs::{Mode,OFlags,PROC_SUPER_MAGIC,ResolveFlags,fstatfs,open,openat,openat2},process::{Gid,Uid,getgid,getuid,getegid,geteuid},
    thread::{set_keep_capabilities,set_no_new_privs,set_thread_groups,set_thread_res_gid,set_thread_res_uid}};
use std::{ffi::CStr,fs::File,io::{self,BufRead,Read,Write},os::{fd::AsFd,unix::{ffi::OsStrExt,fs::MetadataExt}},path::PathBuf};
const PROFILE:&[&str]=&["etc","local-ops-helper","installer.identity"];

pub(super) struct Approved {request:Request, originals:Originals}
impl Approved{
    /// Non-secret op/reference may be recorded BEFORE any write; they are not authority.
    pub(super) fn foreground(op:Id,reference:Id,paths:Vec<PathBuf>)->io::Result<Self>{
        let host=host_admin()?;let (actor,uid,gid)=profile()?;
        let request=Request::new(op,actor,reference,uid,paths)?;
        // Check paths/SQLite capabilities before even showing a proposal. No SQL
        // connection, data read, directory allocation or privilege is granted here.
        request.database(super::engine::path_limit()?)?;
        let originals=Originals::pin(&request,host)?;
        let stdin=io::stdin();let stderr=io::stderr();
        same_terminal(&stdin,&stderr)?;
        let mut tty=stderr.lock();let dig=hex(&request.sha());
        writeln!(tty,"仅本机首次建池。不会格式化或清空卷；将新建唯一 SYSTEM.sqlite3 并认领下面所有专用目录。")?;
        writeln!(tty,"原操作 {}，审批参考 {}，本机维护主体 {}，存储 UID/GID {uid}/{gid}。请先保存这两个操作参考。",hex(&op),hex(&reference),hex(&actor))?;
        for (i,n) in originals.selected.iter().enumerate(){
            let label=match i {0=>"SYSTEM",1=>"RECYCLE",_=>"FILE"};
            writeln!(tty,"{label}  {}  [dev {}, inode {}, mount {}]",escaped(n.path().as_os_str().as_bytes()),n.entity().dev,n.entity().ino,n.mount()?.id)?;
        }
        writeln!(tty,"仅当以上为本机受控的全新专用目录、承载可靠本地锁/同步、回收卷与所有其他角色确为独立文件系统时，输入 INIT {dig}。其他输入不操作。")?;tty.flush()?;
        let mut text=Vec::new();let n=stdin.lock().take(72).read_until(b'\n',&mut text)?;
        if n!=70 || text!=format!("INIT {dig}\n").as_bytes(){return Err(io::Error::new(io::ErrorKind::PermissionDenied,"the local operator did not approve this exact installation"));}
        drop(tty);same_terminal(&stdin,&stderr)?;
        let host_again=host_admin()?;originals.require_host(&host_again)?;drop(host_again);
        if profile()? != (actor,uid,gid){return Err(denied());}
        // End the privileged phase BEFORE opening any storage directory for data or
        // reserving a file. This code runs only in a fresh, single-threaded CLI.
        // Any partial transition terminates the operation; no recovery as root.
        set_no_new_privs(true)?;set_keep_capabilities(false)?;set_thread_groups(&[])?;
        let g=Gid::from_raw(gid);let u=Uid::from_raw(uid);
        set_thread_res_gid(g,g,g)?;set_thread_res_uid(u,u,u)?;
        diagnostic_worker(uid,gid)?;  // all four UIDs/GIDs, no extra groups, caps, or threads
        Ok(Self{request,originals})
    }
    pub(super) fn into_parts(self)->(Request,Originals){(self.request,self.originals)}
}

fn hex(v:&[u8])->String{v.iter().map(|b|format!("{b:02x}")).collect()}
fn escaped(raw:&[u8])->String{raw.iter().copied().flat_map(std::ascii::escape_default).map(char::from).collect()}
fn denied()->io::Error{io::Error::new(io::ErrorKind::PermissionDenied,"exact protected host administrator, profile and controlling terminal required")}
fn same_terminal(input:&io::Stdin,output:&io::Stderr)->io::Result<()>{
    require_terminal(input.as_fd())?;require_terminal(output.as_fd())?;
    let a=rustix::fs::fstat(input)?;let b=rustix::fs::fstat(output)?;
    if a.st_dev!=b.st_dev||a.st_ino!=b.st_ino||a.st_rdev!=b.st_rdev{return Err(denied());}Ok(())
}
fn fields(raw:&[u8])->impl Iterator<Item=&[u8]>{raw.split(u8::is_ascii_whitespace).filter(|s|!s.is_empty())}
fn kernel(name:&CStr,max:u64)->io::Result<Vec<u8>>{
    let mut v=Vec::new();thread_file(name)?.take(max+1).read_to_end(&mut v)?;
    if v.len()as u64>max||!v.ends_with(b"\n"){return Err(denied());}Ok(v)
}
// Linux 6.12 reserves these actual nsfs inode numbers for the initial
// PID and USER namespaces. A self-consistent isolated namespace is not host
// authority. For MNT we compare to the now independently identified host PID 1.
fn require_host_namespaces(current:[Entity;3],pid1:[Entity;3])->io::Result<()>{
    const INITIAL_PID:u64=0xefff_fffc;const INITIAL_USER:u64=0xefff_fffd;
    if current!=pid1 || current[0].ino!=INITIAL_PID || current[1].ino!=INITIAL_USER {
        return Err(denied());
    }
    Ok(())
}
fn host_admin()->io::Result<File>{
    if !(getuid().is_root()&&geteuid().is_root()&&getgid().is_root()&&getegid().is_root()){return Err(denied());}
    let s=kernel(c"thread-self/status",65536)?;
    for (key,value) in [("Threads:","1"),("Uid:","0 0 0 0"),("Gid:","0 0 0 0")]{
        let mut rows=s.split(|&v|v==b'\n').filter_map(|l|l.strip_prefix(key.as_bytes()));
        if !rows.next().is_some_and(|line|fields(line).eq(fields(value.as_bytes())))||rows.next().is_some(){return Err(denied());}
    }
    // Tiny subordinate user namespaces are not the host administrative identity.
    for n in [c"thread-self/uid_map",c"thread-self/gid_map"]{
        if !fields(&kernel(n,4096)?).eq([b"0".as_slice(),b"0",b"4294967295"]){return Err(denied());}
    }
    let flags=OFlags::PATH|OFlags::DIRECTORY|OFlags::NOFOLLOW|OFlags::CLOEXEC;
    let proc=File::from(open(c"/proc",flags,Mode::empty())?);
    let mount=Mount::on(&proc)?.filter(|m|m.at_root==Some(true)).ok_or_else(denied)?;
    if fstatfs(&proc)?.f_type!=PROC_SUPER_MAGIC{return Err(denied());}
    // Comparing just "/" is insufficient: two mount namespaces can expose
    // the same root but different mounts below it. Verify the genuine nsfs
    // objects from this procfs' PID 1 and from the current thread.
    let namespace=|who:&CStr,name:&CStr|->io::Result<Entity>{
        let dir=File::from(openat2(&proc,who,flags,Mode::empty(),
            ResolveFlags::BENEATH|ResolveFlags::NO_XDEV|ResolveFlags::NO_MAGICLINKS)?);
        if Mount::on(&dir)?.is_none_or(|m|m.id!=mount.id){return Err(denied());}
        let link=File::from(openat(&dir,name,OFlags::PATH|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty())?);
        if !link.metadata()?.file_type().is_symlink()||Mount::on(&link)?.is_none_or(|m|m.id!=mount.id){return Err(denied());}
        let ns=File::from(openat(&dir,name,OFlags::PATH|OFlags::CLOEXEC,Mode::empty())?);
        if fstatfs(&ns)?.f_type as u64!=0x6e736673{return Err(denied());} // Linux NSFS_MAGIC
        Entity::of(&ns.metadata()?)
    };
    let triple=|who:&CStr|->io::Result<[Entity;3]>{
        Ok([namespace(who,c"pid")?,namespace(who,c"user")?,namespace(who,c"mnt")?])
    };
    require_host_namespaces(triple(c"thread-self/ns")?,triple(c"1/ns")?)?;
    let pid=File::from(openat(&proc,c"1",flags,Mode::empty())?);
    if Mount::on(&pid)?.is_none_or(|m|m.id!=mount.id){return Err(denied());}
    // Verify the actual procfs magic link before following it, including protection
    // against an ordinary replacement mount. The resulting O_PATH is retained.
    let link=File::from(openat(&pid,c"root",OFlags::PATH|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty())?);
    if !link.metadata()?.file_type().is_symlink()||Mount::on(&link)?.is_none_or(|m|m.id!=mount.id){return Err(denied());}
    let host=File::from(openat(&pid,c"root",OFlags::PATH|OFlags::DIRECTORY|OFlags::CLOEXEC,Mode::empty())?);
    let here=File::from(open(c"/",flags,Mode::empty())?);
    if Entity::of(&host.metadata()?)?!=Entity::of(&here.metadata()?)?||Mount::on(&host)?!=Mount::on(&here)?{return Err(denied());}
    Ok(host)
}
fn profile()->io::Result<(Id,u32,u32)>{
    let f=RuntimeFile::open(PROFILE,false,128)?;
    let m=f.file.metadata()?;if m.mode()&0o7777!=0o600||m.nlink()!=1||m.uid()!=0{return Err(denied());}
    let mut bytes=Vec::new();(&f.file).take(129).read_to_end(&mut bytes)?;f.recheck()?;
    decode_profile(&bytes)
}
fn decode_profile(bytes:&[u8])->io::Result<(Id,u32,u32)>{
    let s=std::str::from_utf8(bytes).map_err(|_|denied())?.strip_suffix('\n').ok_or_else(denied)?;
    let mut words=s.split(' ');if words.next()!=Some("HOST-INSTALL-1"){return Err(denied());}
    let h=words.next().filter(|h|h.len()==32&&h.bytes().all(|c|c.is_ascii_digit()||(b'a'..=b'f').contains(&c))).ok_or_else(denied)?;
    let mut id=[0u8;16];for(i,b)in id.iter_mut().enumerate(){*b=u8::from_str_radix(&h[2*i..2*i+2],16).map_err(|_|denied())?;}
    let number=|s:&str|s.parse::<u32>().ok().filter(|&n|n>0&&n<u32::MAX&&n.to_string()==s).ok_or_else(denied);
    let uid=number(words.next().ok_or_else(denied)?)?;let gid=number(words.next().ok_or_else(denied)?)?;
    if id==[0;16]||words.next().is_some()||bytes.len()>128{return Err(denied());}Ok((id,uid,gid))
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn only_one_registered_nonroot_service_identity_matches_the_fixed_profile(){
        let id="02020202020202020202020202020202";
        assert_eq!(decode_profile(format!("HOST-INSTALL-1 {id} 1001 103\n").as_bytes()).unwrap(),([2;16],1001,103));
        for bad in [format!("HOST-INSTALL-1 {id} 0 103\n"),format!("HOST-INSTALL-1 {id} 1001 0\n"),
            format!("HOST-INSTALL-1 {id} 01001 103\n"),format!("HOST-INSTALL-1 {id} 1001 103\nmore\n"),
            format!("HOST-INSTALL-1 {id} 1001 103"),format!("HOST-INSTALL-1 {} 1000 1001\n","0".repeat(32))]{
            assert!(decode_profile(bad.as_bytes()).is_err());
        }
    }
    #[test]fn a_consistent_private_pid_or_user_view_does_not_establish_the_host(){
        let id=|ino|Entity{dev:9,ino};
        let host=[id(0xefff_fffc),id(0xefff_fffd),id(4026531841)];
        assert!(require_host_namespaces(host,host).is_ok());
        for i in [0,1]{
            let mut private=host;private[i]=id(4026680001);
            assert!(require_host_namespaces(private,private).is_err());
        }
        let mut other=host;other[2].ino+=1;
        assert!(require_host_namespaces(other,host).is_err());
        let mut foreign=host;foreign[2].dev+=1;
        assert!(require_host_namespaces(foreign,host).is_err());
    }
    #[test]fn preview_escapes_opaque_or_terminal_control_bytes(){assert_eq!(escaped(b"/x/\xff\x1b"),r"/x/\xff\x1b");}
}
