//! Fixed foreground setup and role-specific resets. Neither a bare SID,
//! password, alternate program/path nor arbitrary switch can be supplied.
use super::{Id,bad,domain::{create_layout,provision_fresh_domain},native_run::{self,Intent,Role},
    native_sql::{Original,transaction::ResultRow,web_reset}};
use crate::{path_rules::Kind,sqlite_install::id};
use std::{ffi::{OsStr,OsString},io,path::PathBuf};

#[derive(Debug)]
pub(crate) struct Setup(Form);
#[derive(Debug)]
enum Form{
    Layout{instance:Id,reference:Id},
    Provision{system:PathBuf,pool:Id,init:Id,instance:Id,op:Id,reference:Id},
    Smb(native_run::Selection),
    Web(web_reset::Selection),
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
        if mode=="provision"&&it.peek().is_some_and(|v|v=="--create-layout"){
            it.next();let instance=key(&mut it,"--instance")?;let reference=key(&mut it,"--approval")?;
            if it.next().is_some(){return Err("layout accepts only its instance and approval reference");}
            return Ok(Self(Form::Layout{instance,reference}));
        }
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
        if mode=="provision"{
            let op=key(&mut it,"--op")?;let reference=key(&mut it,"--approval")?;
            if init==op||it.next().is_some(){return Err("provision requires its own operation and no other fields");}
            return Ok(Self(Form::Provision{system,pool,init,instance,op,reference}));
        }
        let provision=key(&mut it,"--domain-op")?;
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
        Ok(Self(Form::Smb(native_run::Selection{system,pool,init,instance,provision,op,reference,role,intent})))
    }
    pub(crate) fn run(self)->io::Result<(u8,String)>{
        match self.0{
            Form::Layout{instance,reference}=>{
                create_layout(instance,reference)?;
                Ok((0,format!("layout=prepared instance={}; no pool, SAM domain, accounts or service changed",hex(&instance))))
            }
            Form::Provision{system,pool,init,instance,op,reference}=>{
                provision_fresh_domain(&system,pool,init,instance,op,reference)?;
                Ok((0,format!("domain-binding=confirmed pool={} instance={} domain_op={}; no accounts or service started",
                    hex(&pool),hex(&instance),hex(&op))))
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
fn hex(x:&Id)->String{x.iter().map(|b|format!("{b:02x}")).collect()}
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
    fn view()->String{format!("{} --instance {} --domain-op {}",root(),k(3),k(4))}
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
    }
}
