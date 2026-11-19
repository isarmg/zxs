//! The two explicit stages of first enrollment share one local CLI. An
//! invocation never falls through from lookup into domain allocation, issues
//! only this fixed product pair, and has no input for a password or a SID.
use super::{domain::{provision_fresh_domain,SelectedSetup},publish::{Session,Start},History};
use crate::{path_rules::Kind,sqlite_install::{approval,bad,id,request::Id}};
use std::{ffi::{OsStr,OsString},io,path::PathBuf};

#[derive(Debug)]
pub(crate) struct Setup{system:PathBuf,pool:Id,init:Id,instance:Id,action:Action}
#[derive(Debug)]
enum Action{
    Provision{op:Id,reference:Id},
    Enroll{domain:Id,op:Id,reference:Id,web:Vec<u8>,smb:Vec<u8>,receipt_only:bool},
}

fn value(it:&mut impl Iterator<Item=OsString>,flag:&str)->Result<OsString,&'static str>{
    if it.next().as_deref()!=Some(OsStr::new(flag)){return Err("expected named setup fields in the displayed order");}
    it.next().ok_or("missing setup field")
}
fn key(it:&mut impl Iterator<Item=OsString>,flag:&str)->Result<Id,&'static str>{
    id(value(it,flag)?).ok_or("setup identifiers must be 32 lowercase nonzero hex digits")
}
fn name(it:&mut impl Iterator<Item=OsString>,flag:&str)->Result<Vec<u8>,&'static str>{
    let s=value(it,flag)?;let b=s.as_encoded_bytes();
    if !super::login(b){return Err("expected a bounded lowercase ASCII login name");}Ok(b.to_vec())
}
impl Setup{
    pub(crate) fn parse(mut it:impl Iterator<Item=OsString>)->Result<Self,&'static str>{
        let mode=it.next().ok_or("choose provision or enroll")?;
        if mode!="provision"&&mode!="enroll"{return Err("choose provision or enroll");}
        let system=PathBuf::from(value(&mut it,"--system")?);
        Kind::StorageRoot.check_path(system.as_os_str().as_encoded_bytes())?;
        if system.to_str().is_none(){return Err("SYSTEM is not losslessly representable by the SQLite VFS");}
        let pool=key(&mut it,"--pool")?;let init=key(&mut it,"--init")?;let instance=key(&mut it,"--instance")?;
        let action=if mode=="provision"{
            let op=key(&mut it,"--op")?;let reference=key(&mut it,"--approval")?;
            if init==op||it.next().is_some(){return Err("provision requires its own operation and no other fields");}
            Action::Provision{op,reference}
        }else{
            let domain=key(&mut it,"--domain-op")?;let op=key(&mut it,"--op")?;let reference=key(&mut it,"--approval")?;
            let web=name(&mut it,"--web-login")?;let smb=name(&mut it,"--smb-login")?;
            if init==domain||op==init||op==domain||!super::distinct_roles(&web,&smb){
                return Err("require distinct original operations and two different non-guest protocol roles");}
            let receipt_only=match it.next(){None=>false,Some(f) if f=="--receipt-only"&&it.next().is_none()=>true,
                _=>return Err("only the final --receipt-only flag is accepted")};
            Action::Enroll{domain,op,reference,web,smb,receipt_only}
        };Ok(Self{system,pool,init,instance,action})
    }
    pub(crate) fn run(self)->io::Result<(u8,String)>{
        let Self{system,pool,init,instance,action}=self;
        match action{
            Action::Provision{op,reference}=>{
                // No alternative location, imported domain, replacement, default
                // permission or prior nonempty instance accepted.
                let _=provision_fresh_domain(&system,pool,init,instance,op,reference)?;
                Ok((0,format!("domain-binding=confirmed pool={} instance={} domain_op={}; no accounts or service started",
                    hex(&pool),hex(&instance),hex(&op))))
            }
            Action::Enroll{domain,op,reference,web,smb,receipt_only:true}=>{
                // No credential policy/DSO, KDF budget, secret-interaction
                // channel, approval for a write, or RW connection on this arm.
                let selected=SelectedSetup::select(&system,pool,init,instance,domain)?;
                let(uid,gid)=selected.storage_identity();approval::drop_to_storage(uid,gid)?;
                let installed=selected.after_drop()?;let(_,history)=installed.history(op,reference,&web,&smb)?;
                receipt(history,&op,Outcome::Lookup)
            }
            Action::Enroll{domain,op,reference,web,smb,receipt_only:false}=>{
                let mut session=Session::select(&system,pool,init,instance,domain)?;
                let (history,outcome)=match session.original_or_confirm(op,reference,&web,&smb)?{
                    Start::Original(h)=>(h,Outcome::Lookup),
                    Start::New(order)=>(order.complete()?,Outcome::Created),
                };
                receipt(history,&op,outcome)
            }
        }
    }
}
enum Outcome{Lookup,Created}
fn hex(x:&Id)->String{x.iter().map(|b|format!("{b:02x}")).collect()}
fn receipt(h:History,op:&Id,action:Outcome)->io::Result<(u8,String)>{
    match(h,action){
        (History::Absent,Outcome::Lookup)=>Ok((1,format!("pair=absent setup_op={}; no passwords requested, nothing changed",hex(op)))),
        (History::Absent,Outcome::Created)=>Err(bad("no original receipt after the setup attempt; query the same operation")),
        (History::Committed{web,smb},outcome)=>Ok((0,format!("pair={} setup_op={} web_account={} smb_account={}; not a login or file-access check",
            match outcome{Outcome::Lookup=>"original",Outcome::Created=>"confirmed"},hex(op),hex(&web),hex(&smb)))),
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    fn args(s:&str)->impl Iterator<Item=OsString>+'_{s.split_ascii_whitespace().map(OsString::from)}
    fn base()->String{format!("--system /pool/system --pool {} --init {} --instance {}",
        "01".repeat(16),"02".repeat(16),"03".repeat(16))}
    fn enroll()->String{format!("enroll {} --domain-op {} --op {} --approval {} --web-login web.admin --smb-login network.user",
        base(),"04".repeat(16),"05".repeat(16),"06".repeat(16))}
    #[test]fn the_three_paths_are_explicit_and_never_accept_bare_passwords_or_sids(){
        let p=format!("provision {} --op {} --approval {}",base(),"04".repeat(16),"06".repeat(16));
        assert!(matches!(Setup::parse(args(&p)).unwrap().action,Action::Provision{..}));
        assert!(matches!(Setup::parse(args(&enroll())).unwrap().action,Action::Enroll{receipt_only:false,..}));
        assert!(matches!(Setup::parse(args(&format!("{} --receipt-only",enroll()))).unwrap().action,Action::Enroll{receipt_only:true,..}));
        for bad in [format!("{p} --receipt-only"),format!("{p} --import-domain any"),format!("{} --password test",enroll()),
            format!("{} --sid S-1-5-21-1-2-3-1000",enroll()),format!("{} --receipt-only --receipt-only",enroll()),
            enroll().replace("network.user","guest"),enroll().replace("network.user","web.admin"),
            enroll().replace("0404","0202"),enroll().replace("0505","0404"),
            enroll().replace("/pool/system","/pool/../pool"),enroll().replace("0606","0000"),enroll().replace("--web-login","--other")]{
            assert!(Setup::parse(args(&bad)).is_err());
        }
    }
    #[test]fn exact_absence_is_not_success_and_never_claims_two_new_accounts(){
        let op=[32;16];let a=[1;16];let b=[2;16];
        assert_eq!(receipt(History::Absent,&op,Outcome::Lookup).unwrap().0,1);
        assert!(receipt(History::Absent,&op,Outcome::Created).is_err());
        let older=receipt(History::Committed{web:a,smb:b},&op,Outcome::Lookup).unwrap();
        let new=receipt(History::Committed{web:a,smb:b},&op,Outcome::Created).unwrap();
        assert_eq!((older.0,new.0),(0,0));assert!(older.1.starts_with("pair=original "));assert!(new.1.starts_with("pair=confirmed "));
    }
}
