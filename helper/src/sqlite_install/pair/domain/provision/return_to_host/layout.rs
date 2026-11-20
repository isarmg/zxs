//! Prepare a never-before-named host instance from the original approved
//! ordinary plan. No Samba invocation, account, or running service.
use super::{PREFIX,Site,approval,bad,child,contents,live,mark,pin_dir,safe_dir};
use super::super::{FRESH_SITE,Times,cancelled,capsule};
use crate::{fs_tools::RuntimeFile,maintenance::HostMounts,pinned::Mount,
    smb_profile::{self,first::Plan},sqlite_install::{pair::{terminal::PublicConfirmation,native_run::Resolved},request::Id}};
use rustix::{fs::{FlockOperation,Mode,OFlags,RawDir,RenameFlags,StatVfsMountFlags,fchmod,fchown,
    fgetxattr,flock,fstatvfs,mkdirat,openat,renameat_with},io::Errno,process::{Gid,Uid}};
use sha2::{Digest,Sha256};
use std::{ffi::{CStr,CString},fs::File,io::{self,Read},mem::MaybeUninit,os::unix::fs::{FileExt,MetadataExt}};

const TEMPLATE:&[&str]=&["usr","share","local-ops-helper","initial-live.conf"];
const TOP:&str="/var/lib";
const OWNER:Mode=Mode::RWXU;
const INSTANCE:Mode=Mode::from_bits_retain(0o750);
const SEARCH:Mode=Mode::from_bits_retain(0o711);
const PUBLIC:Mode=Mode::from_bits_retain(0o444);
fn conflict()->io::Error{io::Error::new(io::ErrorKind::AlreadyExists,"layout or stage already exists; no takeover or automatic retry")}
fn noentry(parent:&File,n:&CStr)->io::Result<()>{
    match openat(parent,n,OFlags::PATH|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty()){
        Err(Errno::NOENT)=>Ok(()),Ok(_)=>Err(conflict()),Err(e)=>Err(e.into())
    }
}
fn same_name(parent:&File,n:&CStr,original:&File)->io::Result<()>{
    let now=File::from(openat(parent,n,OFlags::PATH|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty())?);
    let (a,m)=mark(original)?;let (p,pm)=mark(parent)?;
    if mark(&now)?!=(a,m)||pm!=m||p.dev!=a.dev||p==a{return Err(bad("different original layout name, entity or mount"));}
    Ok(())
}
// Do not infer write/search rights from group bits while a named Linux POSIX
// ACL or inherited default ACL may alter them. Unknown on this host is not
// permission to chown/chmod an existing object.
pub(in crate::sqlite_install::pair::domain::provision) fn bare_acl(f:&File,key:&CStr)->io::Result<()>{
    match fgetxattr(f,key,&mut [0u8;0][..]){
        Err(Errno::NODATA)=>Ok(()),
        Ok(_)|Err(Errno::RANGE)=>Err(bad("extended POSIX ACL is outside the first-install layout profile")),
        Err(e)=>Err(e.into()),
    }
}
// Opening a fixed spelling can reach another spelling on a casefolded FS.
// Read only directory entries on the already-selected parent's own stream;
// never open or return an unrelated entry. The independent lookup must still
// resolve to the held directory (a mount point need not have its dirent ino).
pub(super) fn exact_component(parent:&File,name:&CStr,selected:&File)->io::Result<()>{
    let before=Times::on(parent)?;
    let f=File::from(openat(parent,c".",OFlags::RDONLY|OFlags::CLOEXEC|OFlags::DIRECTORY
        |OFlags::NOFOLLOW|OFlags::NOATIME|OFlags::NONBLOCK,Mode::empty())?);
    if mark(&f)?!=mark(parent)?{return Err(bad("different parent in layout name check"));}
    let mut buf=[MaybeUninit::uninit();8192];let mut iter=RawDir::new(&f,&mut buf);
    let mut bytes=0usize;
    for _ in 0..4096{
        let Some(r)=iter.next()else{break};let r=r?;let entry=r.file_name().to_bytes();
        bytes+=entry.len()+1;if bytes>256*1024{break;}
        if entry==name.to_bytes(){
            if mark(&pin_dir(parent,name)?)?==mark(selected)?&&Times::on(parent)?==before{return Ok(());}
            return Err(bad("changed parent or child while checking exact layout spelling"));
        }
    }
    Err(bad("fixed host directory spelling not confirmed within the bounded original parent listing"))
}
// An already configured /var or /var/lib is an operating-system mount
// boundary, not an instruction to write through any existing bind at the
// instance's package/site directories. Keep every original descriptor.
fn one_mount(host:(crate::pinned::Entity,u64),d:(crate::pinned::Entity,u64))->bool{
    host.0.dev==d.0.dev && host.1==d.1
}
fn host_mount(site:&Site)->io::Result<()>{
    let mut parent=mark(&site.host)?;
    let mut seen=vec![parent.0];let mut table=None;
    for (level,d) in site.chain.iter().enumerate(){
        let current=mark(d)?;
        if seen.contains(&current.0){return Err(bad("aliased original instance ancestors"));}
        if !one_mount(parent,current){
            if level>=2||parent.1==current.1{
                return Err(io::Error::new(io::ErrorKind::Unsupported,
                    "an extra mount or device change at or below the dedicated package installation directories is not supported"));
            }
            let mount=Mount::on(d)?.filter(|m|m.id==current.1).ok_or_else(||bad("layout mount identity changed"))?;
            if table.is_none(){table=Some(HostMounts::current()?);}
            table.as_ref().expect("bounded mount snapshot").require_instance_parent_mount(level,mount,parent.1,current.0,
                crate::pinned::local_data_filesystem(d)?)?;
        }
        seen.push(current.0);parent=current;
    }
    Ok(())
}
fn creation_rights(status:&[u8])->bool{
    if status.len()>65536||!status.ends_with(b"\n"){return false;}
    fn field<'a>(s:&'a[u8],k:&[u8])->Option<&'a str>{
        let mut rows=s.split(|&c|c==b'\n').filter_map(|row|row.strip_prefix(k));
        let x=std::str::from_utf8(rows.next()?).ok()?;(rows.next().is_none()).then_some(x)
    }
    let Some(mask)=field(status,b"Umask:\t").filter(|v|v.len()==4&&v.as_bytes()[0]==b'0'&&v.bytes().all(|c|matches!(c,b'0'..=b'7')))
        .and_then(|v|u32::from_str_radix(v,8).ok())else{return false;};
    let Some(cap)=field(status,b"CapEff:\t").filter(|v|v.len()==16&&v.bytes().all(|c|c.is_ascii_hexdigit()))
        .and_then(|v|u64::from_str_radix(v,16).ok())else{return false;};
    // CHOWN, FOWNER and either DAC_OVERRIDE or DAC_READ_SEARCH. Do not
    // change the process mask or capabilities to make a site acceptable.
    mask&0o700==0 && cap&0x9==0x9 && cap&0x6!=0
}
fn creation_preflight(parent:&File)->io::Result<()>{
    if parent.metadata()?.mode()&0o2000!=0{return Err(bad("setgid on the immediate installation parent is not supported; not changing it"));}
    let mut status=Vec::new();crate::maintenance::thread_file(c"thread-self/status")?.take(65537).read_to_end(&mut status)?;
    if !creation_rights(&status){return Err(bad("current umask or effective privileges cannot create the exact new layout"));}
    Ok(())
}
fn access_chain(site:&Site,gid:u32)->io::Result<()>{
    site.check()?;host_mount(site)?;
    for path in std::iter::once(&site.host).chain(site.chain.iter()){
        crate::pinned::local_data_filesystem(path)?;
        let dir=child(path,c".")?;if mark(&dir)?!=mark(path)?{return Err(bad("unattributed installation ancestor"));}
        safe_dir(&dir)?;bare_acl(&dir,c"system.posix_acl_access")?;
        let m=dir.metadata()?;let search=if m.gid()==gid{0o010}else{0o001};
        if m.mode()&search==0{return Err(bad("storage subject cannot traverse the protected installation ancestors"));}
    }
    let mut p=&site.host;
    for (f,name) in site.chain.iter().zip(PREFIX){
        exact_component(p,name,f)?;p=f;
    }
    site.check()
}
pub(super) fn verify_registered_instance(site:&Site, gid:u32)->io::Result<()>{
    if site.chain.len()!=PREFIX.len()+1{return Err(bad("the full original instance path is required"));}
    access_chain(site,gid)?;
    let base=site.base();let m=base.metadata()?;
    if m.gid()!=gid||m.mode()&0o7777!=0o750{return Err(bad("not the protected original instance parent"));}
    exact_component(&site.chain[PREFIX.len()-1],&site.leaf,base)?;
    site.check()
}
fn check_template(template:&RuntimeFile)->io::Result<()>{
    template.recheck()?;let m=template.file.metadata()?;
    if !m.is_file()||m.uid()!=0||m.gid()!=0||m.mode()&0o7777!=0o444||m.nlink()!=1
        ||m.len()!=live::PROFILE.len() as u64{return Err(bad("not the original packaged public live template"));}
    live::exact(&template.file)?;template.recheck()
}
fn new_dir(parent:&File,n:&CStr)->io::Result<File>{
    // Only the actual successful creator goes on to open this name. Even an
    // already-empty directory is ineligible, never a candidate for chmod/chown.
    mkdirat(parent,n,OWNER)?;
    let f=child(parent,n)?;
    same_name(parent,n,&f)?;let m=f.metadata()?;
    if !m.is_dir()||m.uid()!=rustix::process::geteuid().as_raw()||m.nlink()==0||m.mode()&0o7777!=0o700{
        return Err(bad("new temporary directory was not private"));}
    bare_acl(&f,c"system.posix_acl_access")?;bare_acl(&f,c"system.posix_acl_default")?;
    contents(&f,&[])?;Ok(f)
}
fn attrs(f:&File,u:u32,g:u32,mode:Mode,directory:bool)->io::Result<()>{
    let md=f.metadata()?;
    if (if directory{!md.is_dir()||md.nlink()==0}else{!md.is_file()||md.nlink()!=1})
        ||md.uid()!=u||md.gid()!=g||md.mode()&0o7777!=mode.bits(){
        return Err(bad("original layout object does not have its exact owner, group or mode"));}
    Ok(())
}
fn owned(f:&File,u:u32,g:u32,m:Mode)->io::Result<()>{
    fchown(f,Some(Uid::from_raw(u)),Some(Gid::from_raw(g)))?;fchmod(f,m)?;Ok(())
}

struct NewTree<'p>{dirs:Vec<File>,private:File,state:File,account:File,cfg:File,uid:u32,gid:u32,
    plan:&'p Plan, live_sha:[u8;32]}
impl<'p> NewTree<'p>{
    fn base(&self)->&File{self.dirs.last().expect("first newly created directory")}
    fn build(top:File,names:&[&CStr],uid:u32,gid:u32,plan:&'p Plan)->io::Result<Self>{
        let bytes=smb_profile::render(plan).map_err(bad)?;
        let live_sha=Sha256::digest(bytes.as_bytes()).into();
        debug_assert!((1..=3).contains(&names.len()));
        let mut dirs=vec![top];
        for n in &names[1..]{let f=new_dir(dirs.last().expect("top"),n)?;dirs.push(f);}
        let base=dirs.last().expect("instance");
        // Keep this exact FD across publication, parent sync and final readback.
        // No lock on /var/lib or other global host directories.
        flock(base,FlockOperation::NonBlockingLockExclusive)?;
        let private=new_dir(base,c"private")?;let state=new_dir(base,c"state")?;
        let account=new_dir(base,c"accountdb")?;owned(&account,0,0,OWNER)?;
        let cfg=File::from(openat(base,c"smb.conf",OFlags::CREATE|OFlags::EXCL|OFlags::RDWR|OFlags::NOFOLLOW
            |OFlags::NONBLOCK|OFlags::CLOEXEC,Mode::RUSR|Mode::WUSR)?);
        same_name(base,c"smb.conf",&cfg)?;let m=cfg.metadata()?;
        if !m.is_file()||m.uid()!=0||m.nlink()!=1||m.len()!=0||m.mode()&0o7777!=0o600{
            return Err(bad("not an empty original private smb.conf"));}
        cfg.write_all_at(bytes.as_bytes(),0)?;owned(&cfg,0,0,PUBLIC)?;
        for f in [&private,&state]{owned(f,uid,gid,OWNER)?;}
        // Descendants first: the externally visible top stays root-only
        // until every name and owner beneath it has been set.
        let count=dirs.len();
        for(i,f)in dirs.iter().enumerate().rev(){
            let (g,m)=if i+1==count{(gid,INSTANCE)}else{(0,SEARCH)};
            owned(f,0,g,m)?;
        }
        Ok(Self{dirs,private,state,account,cfg,uid,gid,plan,live_sha})
    }
    fn verify(&self,parent:&File,top:&CStr,names:&[&CStr])->io::Result<()>{
        let base=self.base();let mut prior=parent;
        for(i,f)in self.dirs.iter().enumerate(){
            same_name(prior,if i==0{top}else{names[i]},f)?;
            bare_acl(f,c"system.posix_acl_access")?;bare_acl(f,c"system.posix_acl_default")?;
            if i+1==self.dirs.len(){attrs(f,0,self.gid,INSTANCE,true)?;contents(f,FRESH_SITE)?;}
            else{attrs(f,0,0,SEARCH,true)?;contents(f,&[names[i+1].to_bytes()])?;}
            prior=f;
        }
        for(name,f)in [(c"private",&self.private),(c"state",&self.state)]{
            same_name(base,name,f)?;attrs(f,self.uid,self.gid,OWNER,true)?;
            bare_acl(f,c"system.posix_acl_access")?;bare_acl(f,c"system.posix_acl_default")?;contents(f,&[])?;
        }
        same_name(base,c"accountdb",&self.account)?;attrs(&self.account,0,0,OWNER,true)?;
        bare_acl(&self.account,c"system.posix_acl_access")?;bare_acl(&self.account,c"system.posix_acl_default")?;
        contents(&self.account,&[])?;
        let roles=[mark(&self.private)?,mark(&self.state)?,mark(&self.account)?];
        if roles.iter().enumerate().any(|(i,x)|roles[..i].contains(x)){return Err(bad("aliased dedicated directories"));}
        same_name(base,c"smb.conf",&self.cfg)?;attrs(&self.cfg,0,0,PUBLIC,false)?;
        // Independently reopen only this already-matched public original.
        let found=live::Live::select(base,&self.live_sha,self.plan.origin)?;
        if found.plan()!=self.plan{return Err(bad("different complete installed first plan"));}
        found.require(base)
    }
    fn stamps(&self)->io::Result<Vec<Times>>{
        self.dirs.iter().chain([&self.private,&self.state,&self.account,&self.cfg]).map(Times::on).collect()
    }
    fn sync(&self,parent:&File,stage:&CStr,names:&[&CStr])->io::Result<Vec<Times>>{
        self.verify(parent,stage,names)?;let stamp=self.stamps()?;
        self.cfg.sync_all()?;self.private.sync_all()?;self.state.sync_all()?;self.account.sync_all()?;
        for f in self.dirs.iter().rev(){f.sync_all()?;}
        self.verify(parent,stage,names)?;
        if self.stamps()?!=stamp{return Err(bad("new layout changed while synchronizing"));}
        Ok(stamp)
    }
}
fn publish(parent:&File,stage:&CStr,dest:&CStr)->io::Result<()>{
    renameat_with(parent,stage,parent,dest,RenameFlags::NOREPLACE)?;Ok(())
}
pub(in crate::sqlite_install::pair) fn create_layout(resolved:&Resolved,reference:Id)->io::Result<[u8;32]>{
    let plan=resolved.plan();let instance=plan.origin.instance;
    if reference==[0;16]{return Err(bad("missing host layout approval reference"));}
    resolved.require()?;
    let bytes=smb_profile::render(plan).map_err(bad)?;
    let live_sha=Sha256::digest(bytes.as_bytes()).into();
    let host=approval::host_admin()?;let terminal=PublicConfirmation::select()?;
    let (actor,uid,gid)=approval::profile()?;
    let site=Site::walk(host,&instance,true)?;let present=site.chain.len();
    if present==PREFIX.len()+1{return Err(conflict());}
    let public_name=match present{2=>TOP,3=>"/var/lib/local-ops-helper",4=>"/var/lib/local-ops-helper/samba-instances",
        _=>return Err(bad("protected /var/lib must already exist"))};
    let all=[PREFIX[2],PREFIX[3],site.leaf.as_c_str()];let suffix=&all[present-2..];let first=suffix[0];
    let stage=CString::new(format!(".layout-{}.new",first.to_str().expect("fixed ASCII"))).expect("ASCII");
    host_mount(&site)?;crate::pinned::local_data_filesystem(site.base())?;
    let parent=child(site.base(),c".")?;let parent_id=mark(&parent)?;creation_preflight(&parent)?;
    let template=RuntimeFile::open(TEMPLATE,false,live::PROFILE.len()as u64)?;
    let release=capsule::Originals::open()?;
    helper_linux_evidence::diagnostic_process::watch_cancellation()?;
    let origin=||->io::Result<()>{
        cancelled()?;resolved.require()?;terminal.check()?;access_chain(&site,gid)?;creation_preflight(&parent)?;check_template(&template)?;release.slots()?;
        if approval::profile()? != (actor,uid,gid)||mark(&parent)?!=parent_id||mark(site.base())?!=parent_id{
            return Err(bad("installation subject or approved parent changed"));}
        let need=stage.to_bytes().len().max(32);
        if fstatvfs(&parent)?.f_flag.contains(StatVfsMountFlags::RDONLY)
            ||usize::from(helper_linux_evidence::portable_component_limit(&parent)?)<need{
            return Err(bad("the original parent cannot hold this no-replace staging name"));}
        bare_acl(&parent,c"system.posix_acl_default")
    };
    let unused=||->io::Result<()>{origin()?;noentry(&parent,first)?;noentry(&parent,&stage)};
    unused()?;
    terminal.confirm_layout([&instance,&actor,&reference],(uid,gid),public_name,parent_id.0,parent_id.1,
        &live_sha,&release.key(),resolved.preview(),&smb_profile::approval_text(plan))?;
    release.require()?;unused()?;
    // Failure from here may leave this exact stage or the final name. We never
    // clean it, adopt it, apply permission changes to an existing destination
    // or restart domain provision. RENAME_NOREPLACE is not a source inode CAS;
    // the selected trusted host forbids concurrent privileged substitutions.
    (||->io::Result<[u8;32]>{
        let top=new_dir(&parent,&stage)?;origin()?;same_name(&parent,&stage,&top)?;
        let tree=NewTree::build(top,suffix,uid,gid,plan)?;
        let mut synced=tree.sync(&parent,&stage,suffix)?;
        origin()?;noentry(&parent,first)?;tree.verify(&parent,&stage,suffix)?;
        if tree.stamps()?!=synced{return Err(bad("layout changed before no-replace publication"));}
        publish(&parent,&stage,first)?;origin()?;
        noentry(&parent,&stage)?;tree.verify(&parent,first,suffix)?;exact_component(&parent,first,&tree.dirs[0])?;
        // Renaming changes the top directory's ctime; no other sync evidence
        // may be replaced. The directory and all children remain held.
        synced[0].ctime=Times::on(&tree.dirs[0])?.ctime;
        if tree.stamps()?!=synced{return Err(bad("another object or change appeared during publication"));}
        let parent_stamp=Times::on(&parent)?;parent.sync_all()?;
        origin()?;noentry(&parent,&stage)?;tree.verify(&parent,first,suffix)?;exact_component(&parent,first,&tree.dirs[0])?;
        if tree.stamps()?!=synced||Times::on(&parent)?!=parent_stamp{return Err(bad("layout changed across final parent synchronization"));}
        Ok(live_sha)
    })().map_err(|e|io::Error::new(e.kind(),format!("new layout not confirmed; preserve any stage or final directory, do not retry or adopt: {e}")))
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn a_mount_or_device_change_is_not_plain_ancestry_even_if_objects_match(){
        let root=crate::pinned::Entity{dev:8,ino:2};let subtree=crate::pinned::Entity{dev:8,ino:71};
        assert!(one_mount((root,15),(subtree,15)));assert!(!one_mount((root,15),(subtree,16)));
        assert!(!one_mount((root,15),(crate::pinned::Entity{dev:9,ino:71},15)));
    }
    #[test]fn exact_owner_creation_rights_and_unambiguous_kernel_fields_are_needed(){
        assert!(creation_rights(b"Umask:\t0077\nCapEff:\t000000000000000b\n"));
        assert!(creation_rights(b"Umask:\t0022\nCapEff:\t000000000000000d\n"));
        for s in [b"Umask:\t0422\nCapEff:\t000000000000000f\n".as_slice(),
            b"Umask:\t0022\nUmask:\t0077\nCapEff:\t000000000000000f\n",
            b"Umask:\t0077\nCapEff:\t0000000000000009\n",
            b"Umask:\t0022\nCapEff:\t0000000000000007\n",b"Umask:\t0000\n"]{assert!(!creation_rights(s));}
    }
    #[test]fn an_existing_empty_or_populated_destination_is_never_merged_or_overwritten(){
        let scope=tempfile::tempdir().unwrap();let p=File::open(scope.path()).unwrap();
        let stage=new_dir(&p,c".layout-id.new").unwrap();
        let original=new_dir(&p,c"id").unwrap();
        assert!(noentry(&p,c"id").is_err());assert!(new_dir(&p,c"id").is_err());
        assert!(publish(&p,c".layout-id.new",c"id").is_err());same_name(&p,c"id",&original).unwrap();
        std::fs::write(scope.path().join("id/existing"),b"keep").unwrap();
        assert!(publish(&p,c".layout-id.new",c"id").is_err());same_name(&p,c".layout-id.new",&stage).unwrap();
        assert_eq!(std::fs::read(scope.path().join("id/existing")).unwrap(),b"keep");
    }
    #[test]fn an_alternate_spelling_is_not_the_original_entry_even_with_the_right_child_fd(){
        let scope=tempfile::tempdir().unwrap();let p=File::open(scope.path()).unwrap();
        let held=new_dir(&p,c"Samba-Instances").unwrap();
        exact_component(&p,c"Samba-Instances",&held).unwrap();
        assert!(exact_component(&p,c"samba-instances",&held).is_err());
    }
    #[test]fn stage_collision_symlink_and_replaced_parent_do_not_name_the_approved_original(){
        let scope=tempfile::tempdir().unwrap();let p=File::open(scope.path()).unwrap();
        let old=new_dir(&p,c"held").unwrap();std::fs::rename(scope.path().join("held"),scope.path().join("original")).unwrap();
        let _other=new_dir(&p,c"held").unwrap();assert!(same_name(&p,c"held",&old).is_err());
        std::os::unix::fs::symlink("/no/such/input",scope.path().join(".layout-id.new")).unwrap();
        assert!(noentry(&p,c".layout-id.new").is_err());assert!(new_dir(&p,c".layout-id.new").is_err());
    }
    #[test]
    #[ignore="dedicated local root fixture for actual chown; no system installation or service"]
    fn three_new_levels_are_held_synced_and_published_together(){
        let scope=tempfile::tempdir().unwrap();let p=File::open(scope.path()).unwrap();
        let names:[&CStr;3]=[c"local-ops-helper",c"samba-instances",c"0123456789abcdef0123456789abcdef"];
        let stage=c".layout-local-ops-helper.new";let top=new_dir(&p,stage).unwrap();
        use crate::smb_profile::first::{Endpoint,Folder,Mode,Origin,RecycleName};
        let plan=Plan::selected(Origin{pool:[1;16],init:[2;16],instance:[3;16],domain:[4;16],setup:[5;16]},
            vec![Endpoint::named("192.0.2.17/24").unwrap()],
            vec![Folder{volume:[9;16],name:"Docs".into(),mode:Mode::ReadOnly}],RecycleName::new("Recycle".into()).unwrap()).unwrap();
        let t=NewTree::build(top,&names,65534,65534,&plan).unwrap();
        t.sync(&p,stage,&names).unwrap();noentry(&p,names[0]).unwrap();
        publish(&p,stage,names[0]).unwrap();p.sync_all().unwrap();t.verify(&p,names[0],&names).unwrap();
        let _independent=child(t.base(),c".").unwrap();
        assert_eq!(flock(&_independent,FlockOperation::NonBlockingLockShared),Err(Errno::WOULDBLOCK));
        std::fs::write(scope.path().join("local-ops-helper/samba-instances/0123456789abcdef0123456789abcdef/private/foreign"),b"retain").unwrap();
        assert!(t.verify(&p,names[0],&names).is_err());
    }
}
