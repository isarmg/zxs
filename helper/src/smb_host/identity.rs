//! Actual files-NSS leaves in the private smbd view, unrelated to the two
//! product accounts or to the host's /etc. The site supplies the immutable
//! role mapping separately from its approved program digests.
use super::{no,origins::{file_at,ident,mid,ro}};
use rustix::fs::OFlags;
use sha2::{Digest,Sha256};
use std::{fs::{File,Metadata},io::{self,Read},os::unix::fs::MetadataExt};

const SITE:&str="zxs-smb-identities";
const MAX_SITE:u64=256;
const MAX_LEAF:u64=512;
const SWITCH:&[u8]=b"aliases: files\nethers: files\ngroup: files\ngroup_compat: files\ngshadow: files\nhosts: files\ninitgroups: files\nnetgroup: files\nnetworks: files\npasswd: files\npasswd_compat: files\nprotocols: files\npublickey: files\nrpc: files\nservices: files\nshadow: files\nshadow_compat: files\n";
const PASSWD:&str="root:!:0:0::/nonexistent:/usr/sbin/nologin\nnobody:!:65534:65534::/nonexistent:/usr/sbin/nologin\n";
const GROUP:&str="root:!:0:\nnogroup:!:65534:\n";
type Pair=(u32,u32);
fn number(s:&str)->io::Result<u32>{
    let b=s.as_bytes();
    if !(1..=10).contains(&b.len())||!matches!(b.first().copied(),Some(b'1'..=b'9'))||!b.iter().all(u8::is_ascii_digit){return Err(no());}
    let v=s.parse::<u32>().map_err(|_|no())?;
    if matches!(v,65534|u32::MAX){return Err(no());}Ok(v)
}
fn profile(raw:&[u8])->io::Result<[Pair;2]>{
    if raw.is_empty()||raw.len()>MAX_SITE as usize||!raw.ends_with(b"\n"){return Err(no());}
    let s=std::str::from_utf8(raw).map_err(|_|no())?;let mut lines=s.split_terminator('\n');
    if lines.next()!=Some("ZXS-SMB-UNIX-2"){return Err(no());}
    let mut values=[(0,0);2];
    for ((uid,gid),name) in values.iter_mut().zip(["storage","smb-execution"]){
        let mut words=lines.next().ok_or_else(no)?.split(' ');
        if words.next()!=Some(name){return Err(no());}
        *uid=number(words.next().ok_or_else(no)?)?;
        *gid=number(words.next().ok_or_else(no)?)?;
        if words.next().is_some(){return Err(no());}
    }
    if lines.next().is_some()||values.iter().enumerate().any(|(i,(u,g))|
        values[..i].iter().any(|(old_u,old_g)|old_u==u||old_g==g)){return Err(no());}
    Ok(values)
}
fn records((u,g):Pair)->(String,String){
    (format!("{PASSWD}zxs-smb-client:!:{u}:{g}::/nonexistent:/usr/sbin/nologin\n"),
     format!("{GROUP}zxs-smb-client:!:{g}:\n"))
}
fn stamp(m:&Metadata)->(u64,i64,i64,i64,i64){(m.len(),m.ctime(),m.ctime_nsec(),m.mtime(),m.mtime_nsec())}
fn under(root:&File,file:&File)->io::Result<Metadata>{
    let m=file.metadata()?;
    if !m.is_file()||m.uid()!=0||m.gid()!=0||m.mode()!=0o100644||m.nlink()!=1||
        !(1..=MAX_LEAF).contains(&m.len())||mid(root)?!=mid(file)?||!ro(file)?{return Err(no());}
    Ok(m)
}
fn parent(root:&File)->io::Result<File>{
    let pin=file_at(root,"etc",OFlags::PATH|OFlags::DIRECTORY|OFlags::NOFOLLOW|OFlags::CLOEXEC)?;
    let m=pin.metadata()?;
    if !m.is_dir()||m.uid()!=0||m.gid()!=0||m.mode()&0o7022!=0||m.nlink()==0||
        mid(root)?!=mid(&pin)?||!ro(&pin)?{return Err(no());}
    Ok(pin)
}
fn exact(root:&File,path:&str)->io::Result<Vec<u8>>{
    let pin=OFlags::PATH|OFlags::NOFOLLOW|OFlags::CLOEXEC;
    let original=file_at(root,path,pin)?;
    let want=stamp(&under(root,&original)?);
    let file=crate::pinned::smb_nss_reader(&original,mid(root)?,want.0)?;
    let check=|test:&File|->io::Result<()>{
        if stamp(&under(root,test)?)!=want||ident(test)?!=ident(&original)?{return Err(no());}Ok(())
    };
    check(&file)?;
    let mut b=Vec::new();(&file).take(want.0+1).read_to_end(&mut b)?;
    if b.len() as u64!=want.0{return Err(no());}
    check(&file)?;check(&original)?;check(&file_at(root,path,pin)?)?;Ok(b)
}
pub(super) fn require(daemon:&File,storage:&File,approved:[u8;32],owner:Pair)->io::Result<()>{
    let etc=parent(daemon)?;let mark=stamp(&etc.metadata()?);
    let bytes=exact(&etc,SITE)?;
    if [0;32]==approved||<[u8;32]>::from(Sha256::digest(&bytes))!=approved{return Err(no());}
    let [s,e]=profile(&bytes)?;
    if s!=owner{return Err(no());}
    let (passwd,group)=records(e);
    if exact(&etc,"nsswitch.conf")?!=SWITCH||exact(&etc,"passwd")?!=passwd.as_bytes()||
        exact(&etc,"group")?!=group.as_bytes(){return Err(no());}
    let named=parent(daemon)?;
    if !super::same(&etc,&named)?||mark!=stamp(&etc.metadata()?)||mark!=stamp(&named.metadata()?){return Err(no());}
    match file_at(storage,"etc/zxs-smb-identities",OFlags::PATH|OFlags::NOFOLLOW|OFlags::CLOEXEC){
        Err(e) if e.kind()==io::ErrorKind::NotFound=>Ok(()),_=>Err(no()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const GOOD:&[u8]=b"ZXS-SMB-UNIX-2\nstorage 2001 2002\nsmb-execution 4001 4002\n";
    #[test] fn approved_fixed_fields_do_not_create_account_passwords_or_supplementary_groups(){
        assert_eq!(profile(GOOD).unwrap(),[(2001,2002),(4001,4002)]);
        let (p,g)=records((4001,4002));
        assert_eq!(p,format!("{PASSWD}zxs-smb-client:!:4001:4002::/nonexistent:/usr/sbin/nologin\n"));
        assert_eq!(g,format!("{GROUP}zxs-smb-client:!:4002:\n"));
        let sample=std::str::from_utf8(GOOD).unwrap();
        for (before,after) in [("4001","2001"),("4002","2002"),("4002","65534"),("4001","4294967295"),("4001","0"),
            ("4001","04001"),("4001","4001\r"),(" 4001","  4001"),("smb-execution","storage")]{
            assert!(profile(sample.replace(before,after).as_bytes()).is_err());
        }
        assert!(profile(&GOOD[..GOOD.len()-1]).is_err());
    }
}
