//! Fixed foreground setup and role-specific resets. Neither a bare SID,
//! password, alternate program/path nor arbitrary switch can be supplied.
use super::{Id,bad,domain::{create_layout,provision_fresh_domain},native_run::{self,Choice,Intent,Resolved,Role},
    native_sql::{Original,transaction::ResultRow,web_reset}};
use crate::{path_rules::Kind,smb_profile::first::{Endpoint,Mode,Origin,MAX_ENDPOINTS,MAX_FOLDERS},sqlite_install::id};
use sha2::{Digest,Sha256};
use std::{ffi::{OsStr,OsString},io,path::PathBuf};

#[derive(Debug)]
pub(crate) struct Setup(Form);
#[derive(Debug)]
enum Form{
    Layout{system:PathBuf,origin:Origin,reference:Id,endpoints:Vec<Endpoint>,folders:Vec<Choice>},
    Provision{system:PathBuf,pool:Id,init:Id,instance:Id,op:Id,setup:Id,reference:Id,live:[u8;32]},
    Smb(native_run::Selection),
    Web(web_reset::Selection),
}
pub(super) fn value(it:&mut impl Iterator<Item=OsString>,flag:&str)->Result<OsString,&'static str>{
    if it.next().as_deref()!=Some(OsStr::new(flag)){return Err("expected named setup fields in the displayed order");}
    it.next().ok_or("missing setup field")
}
pub(super) fn key(it:&mut impl Iterator<Item=OsString>,flag:&str)->Result<Id,&'static str>{
    id(value(it,flag)?).ok_or("setup identifiers must be 32 lowercase nonzero hex digits")
}
pub(super) fn sha(it:&mut impl Iterator<Item=OsString>,flag:&str)->Result<[u8;32],&'static str>{
    value(it,flag)?.to_str().filter(|s|s.bytes().all(|b|b.is_ascii_digit()||matches!(b,b'a'..=b'f')))
        .and_then(crate::fs_tools::hex).filter(|x|*x!=[0;32]).ok_or("expected the independent original receipt SHA-256 (64 lowercase hex digits)")
}
fn name(it:&mut impl Iterator<Item=OsString>,flag:&str)->Result<Vec<u8>,&'static str>{
    let s=value(it,flag)?;let b=s.as_encoded_bytes();
    if !super::login(b){return Err("expected a bounded lowercase ASCII login name");}Ok(b.to_vec())
}
fn generation(it:&mut impl Iterator<Item=OsString>)->Result<i64,&'static str>{
    let v=value(it,"--previous-generation")?;
    let t=v.to_str().ok_or("expected canonical positive decimal generation")?;
    t.parse::<i64>().ok().filter(|n|(1..i64::MAX).contains(n)&&n.to_string()==t).ok_or("expected the exact prior, incrementable generation")
}
fn intent(it:&mut impl Iterator<Item=OsString>,resume:bool)->Result<Intent,&'static str>{
    match it.next(){
        None=>Ok(Intent::New),
        Some(f) if f=="--receipt-only"&&it.next().is_none()=>Ok(Intent::Lookup),
        Some(f) if resume&&f=="--resume"&&it.next().is_none()=>Ok(Intent::Resume),
        _=>Err("one final explicit --receipt-only or supported --resume only"),
    }
}
impl Setup{
    pub(crate) fn parse(it:impl Iterator<Item=OsString>)->Result<Self,&'static str>{
        let mut it=it.peekable();let m=it.next().ok_or("choose provision, enroll, reset-web or reset-smb")?;
        let mode=m.to_str().ok_or("invalid setup command")?;
        if !["provision","enroll","reset-web","reset-smb"].contains(&mode){
            return Err("choose provision, enroll, reset-web or reset-smb");
        }
        let layout=mode=="provision"&&it.peek().is_some_and(|v|v=="--create-layout");
        if layout{it.next();}
        let system=PathBuf::from(value(&mut it,"--system")?);
        Kind::StorageRoot.check_path(system.as_os_str().as_encoded_bytes())?;
        if system.to_str().is_none(){return Err("SYSTEM is not losslessly representable by the SQLite VFS");}
        let pool=key(&mut it,"--pool")?;let init=key(&mut it,"--init")?;
        if mode=="reset-web"{
            let setup=key(&mut it,"--setup-op")?;let op=key(&mut it,"--op")?;let reference=key(&mut it,"--approval")?;
            let account=key(&mut it,"--web-account")?;let previous=key(&mut it,"--previous-op")?;
            let generation=generation(&mut it)?;let lookup=intent(&mut it,false)?==Intent::Lookup;
            return Ok(Self(Form::Web(web_reset::Selection{system,pool,init,setup,op,reference,account,previous,generation,lookup})));
        }
        let instance=key(&mut it,"--instance")?;
        if layout{
            let domain=key(&mut it,"--domain-op")?;let setup=key(&mut it,"--setup-op")?;
            let reference=key(&mut it,"--approval")?;
            let mut endpoints=Vec::new();let mut folders=Vec::new();
            while let Some(flag)=it.next(){
                if flag=="--listen"&&endpoints.len()<MAX_ENDPOINTS{
                    let a=it.next().ok_or("missing selected listener address and prefix")?;
                    endpoints.push(Endpoint::named(a.to_str().ok_or("expected an exact IP address and prefix")?)?);
                }else if flag=="--share"&&folders.len()<MAX_FOLDERS{
                    let p=PathBuf::from(it.next().ok_or("missing selected original data root")?);
                    Kind::StorageRoot.check_path(p.as_os_str().as_encoded_bytes())?;
                    let label=it.next().and_then(|s|s.into_string().ok()).ok_or("missing exact UTF-8 folder name")?;
                    let mode=match it.next().as_deref().and_then(OsStr::to_str){
                        Some("ro")=>Mode::ReadOnly,Some("rw")=>Mode::ReadWrite,_=>return Err("expected the explicit folder mode ro or rw"),
                    };
                    folders.push(Choice{path:p,folder:label,mode});
                }else{return Err("only bounded repeated --listen ADDR/PREFIX and --share DIR NAME ro|rw are accepted");}
            }
            if endpoints.is_empty()||folders.is_empty(){return Err("select at least one LAN listener and one original ordinary folder");}
            return Ok(Self(Form::Layout{system,origin:Origin{pool,init,instance,domain,setup},reference,endpoints,folders}));
        }
        if mode=="provision"{
            let op=key(&mut it,"--op")?;let setup=key(&mut it,"--setup-op")?;
            let reference=key(&mut it,"--approval")?;let live=sha(&mut it,"--live-sha256")?;
            if init==op||op==setup||init==setup||it.next().is_some(){return Err("provision requires distinct domain and original setup operations");}
            return Ok(Self(Form::Provision{system,pool,init,instance,op,setup,reference,live}));
        }
        let provision=key(&mut it,"--domain-op")?;
        let bound_sha=sha(&mut it,"--bound-sha256")?;
        let setup=if mode=="reset-smb"{Some(key(&mut it,"--setup-op")?)}else{None};
        let op=key(&mut it,"--op")?;let reference=key(&mut it,"--approval")?;
        let role=if let Some(setup)=setup{
            let account=key(&mut it,"--smb-account")?;let smb=name(&mut it,"--smb-login")?;
            let previous=key(&mut it,"--previous-op")?;let generation=generation(&mut it)?;
            if !super::distinct_roles(&[],&smb){return Err("not the single ordinary SMB login name");}
            Role::Reset{setup,account,previous,generation,smb}
        }else{
            let web=name(&mut it,"--web-login")?;let smb=name(&mut it,"--smb-login")?;
            if !super::distinct_roles(&web,&smb){return Err("expected two different non-guest protocol names");}
            Role::Birth{web,smb}
        };
        if init==provision||op==init||op==provision{return Err("select distinct original operations");}
        let intent=intent(&mut it,true)?;
        Ok(Self(Form::Smb(native_run::Selection{system,pool,init,instance,provision,op,reference,role,intent,bound_sha})))
    }
    pub(crate) fn run(self)->io::Result<(u8,String)>{
        match self.0{
            Form::Layout{system,origin,reference,endpoints,folders}=>{
                let resolved=Resolved::select(&system,origin,endpoints,folders)?;
                let live=create_layout(&resolved,reference)?;
                Ok((0,format!("layout=prepared instance={} plan_sha256={} live_sha256={}; no pool, SAM domain, account or service changed",
                    hex(&origin.instance),hex(&resolved.plan().sha()),hex(&live))))
            }
            Form::Provision{system,pool,init,instance,op,setup,reference,live}=>{
                let original=provision_fresh_domain(&system,pool,init,instance,op,setup,reference,live)?;
                Ok((0,domain_receipt(&pool,&instance,&op,&original)))
            }
            Form::Smb(selection)=>{
                let id=selection.op;
                let role=if matches!(&selection.role,Role::Birth{..}){"pair"}else{"smb-reset"};
                receipt(&id,role,native_run::run(selection)?)
            }
            Form::Web(selection)=>{
                let id=selection.op;
                let row=match selection.run()?{
                    Original::Absent=>Original::Absent,
                    Original::Committed(n)=>Original::Committed(ResultRow::Reset(n)),
                    Original::Pending(_)=>return Err(bad("Web reset cannot be a native pending order")),
                };
                receipt(&id,"web-reset",row)
            }
        }
    }
}
fn hex(x:&[u8])->String{x.iter().map(|b|format!("{b:02x}")).collect()}
fn domain_receipt(pool:&Id,instance:&Id,op:&Id,original:&[u8;128])->String{
    let hash:[u8;32]=Sha256::digest(original).into();
    format!("domain-binding=confirmed pool={} instance={} domain_op={} bound_sha256={}; no accounts or service started",
        hex(pool),hex(instance),hex(op),hex(&hash))
}
fn receipt(op:&Id,role:&str,h:Original<ResultRow>)->io::Result<(u8,String)>{
    let r=match h{
        Original::Absent=>(1,format!("{role}=absent op={}; no effect is confirmed",hex(op))),
        Original::Pending(_)=>(1,format!("{role}=pending op={}; not active; only an explicitly authorized original resume may advance it",hex(op))),
        Original::Committed(ResultRow::Setup(ids))=>(0,format!("pair=confirmed op={} web_account={} smb_account={}; not a login check",
            hex(op),hex(&ids.web_value()),hex(&ids.smb_value()))),
        Original::Committed(ResultRow::Reset(generation))=>(0,format!("{role}=confirmed op={} generation={generation}; not a login check",hex(op))),
    };Ok(r)
}

#[cfg(test)]
mod tests{
    use super::*;
    fn args(s:&str)->impl Iterator<Item=OsString>+'_{s.split_ascii_whitespace().map(OsString::from)}
    fn k(n:u8)->String{hex(&[n;16])}
    fn root()->String{format!("--system /pool/system --pool {} --init {}",k(1),k(2))}
    fn view()->String{format!("{} --instance {} --domain-op {} --bound-sha256 {}",root(),k(3),k(4),"2b".repeat(32))}
    #[test]fn successful_domain_receipt_exports_only_the_digest_of_the_verified_original(){
        let original=[0x5a;128];
        let reply=domain_receipt(&[1;16],&[2;16],&[3;16],&original);
        assert_eq!(reply,format!("domain-binding=confirmed pool={} instance={} domain_op={} bound_sha256={}; no accounts or service started",
            k(1),k(2),k(3),"349d65e9ba1de7b0a13f9a3eadcc5b0202f15d6008fe9477f2a7b80f6194b20f"));
        assert!(!reply.contains(&"5a".repeat(128)));
    }
    #[test]fn original_paths_and_explicit_scope_prepare_only_one_layout(){
        let good=format!("provision --create-layout {} --instance {} --domain-op {} --setup-op {} --approval {} --listen 192.0.2.17/24 --share /original/data Photos rw",root(),k(3),k(4),k(5),k(6));
        assert!(Setup::parse(args(&good)).is_ok());
        assert!(Setup::parse(args(&good.replace("192.0.2.17/24","0.0.0.0/0"))).is_err());
        assert!(Setup::parse(args(&good.replace("/original/data","/../data"))).is_err());
        assert!(Setup::parse(args(&good.replace("Photos rw","Photos guest"))).is_err());
        assert!(Setup::parse(args(&format!("provision --create-layout --instance {} --approval {}",k(3),k(6)))).is_err());
    }
    #[test]fn role_actions_never_accept_passwords_or_alternate_targets(){
        let birth=format!("enroll {} --op {} --approval {} --web-login web.admin --smb-login files.user",view(),k(5),k(6));
        let smb=format!("reset-smb {} --setup-op {} --op {} --approval {} --smb-account {} --smb-login files.user --previous-op {} --previous-generation 1",
            view(),k(5),k(7),k(8),k(9),k(5));
        let web=format!("reset-web {} --setup-op {} --op {} --approval {} --web-account {} --previous-op {} --previous-generation 1",
            root(),k(5),k(7),k(8),k(9),k(5));
        for form in [&birth,&smb,&web]{
            assert!(Setup::parse(args(form)).is_ok());
            assert!(Setup::parse(args(&format!("{form} --receipt-only"))).is_ok());
            assert!(Setup::parse(args(&format!("{form} --password ignore"))).is_err());
            assert!(Setup::parse(args(&format!("{form} --sid 0000"))).is_err());
        }
        assert!(Setup::parse(args(&format!("{smb} --resume"))).is_ok());
        assert!(Setup::parse(args(&format!("{web} --resume"))).is_err());
        assert!(Setup::parse(args(&smb.replace("--previous-generation 1","--previous-generation 01"))).is_err());
        assert!(Setup::parse(args(&birth.replace("files.user","guest"))).is_err());
        assert!(Setup::parse(args(&birth.replace(&format!(" --bound-sha256 {}","2b".repeat(32)),""))).is_err());
        assert!(Setup::parse(args(&smb.replace(&"2b".repeat(32),&"00".repeat(32)))).is_err());
        assert!(Setup::parse(args(&smb.replace(&"2b".repeat(32),&"2B".repeat(32)))).is_err());
    }
}
