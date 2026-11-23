//! The one installed v20 shared account has one new or historical operation.
//! The command never selects an alternate site, identity, database or vault.
use super::rust_credentials::transact_deployed;
use crate::sqlite_install::{bad,pair::{Id,command::{key,sha,value},domain::DeployedSite,
    fresh_issuer::require_private_install,native_sql::rust_vault::{Desired,RustOrder},
    rust_enroll::manifest::{Authority,DeployedScope,Intent,Manifest},terminal::PublicConfirmation},stage::random};
use std::{ffi::OsString,io};

#[derive(Debug)]
pub(crate) struct Selection{
    intent:Intent,op:Id,approval:Id,previous:Id,generation:i64,helper:[u8;32],
    providers:Option<([u8;32],[u8;32])>,
}
impl Selection{
    pub(crate) fn parse(mut it:impl Iterator<Item=OsString>)->Result<Self,&'static str>{
        if !cfg!(feature="private-fresh-install"){return Err("private v20 shared-account maintenance is disabled");}
        let intent=match it.next().as_deref().and_then(|s|s.to_str()){
            Some("new")=>Intent::Fresh,Some("receipt")=>Intent::Lookup,
            _=>return Err("choose reset-account new or receipt explicitly"),
        };
        let op=key(&mut it,"--op")?;let approval=key(&mut it,"--approval")?;
        let previous=key(&mut it,"--previous")?;
        let generation=value(&mut it,"--previous-generation")?;
        let generation=generation.to_str().filter(|n|n.len()<=19&&n.as_bytes().first().is_some_and(|v|matches!(v,b'1'..=b'9'))
            &&n.bytes().all(|v|v.is_ascii_digit())).and_then(|n|n.parse::<i64>().ok()).filter(|n|(1..i64::MAX).contains(n))
            .ok_or("expected the exact positive previous shared-account generation")?;
        let helper=sha(&mut it,"--helper-sha256")?;
        let providers=if intent==Intent::Fresh{Some((sha(&mut it,"--gcrypt-sha256")?,sha(&mut it,"--gpgerror-sha256")?))}else{None};
        if it.next().is_some()||op==approval||previous==op{return Err("expected just the exact ordered reset and independent digests");}
        Ok(Self{intent,op,approval,previous,generation,helper,providers})
    }
    pub(crate) fn run(self)->io::Result<u8>{
        require_private_install()?;
        // Select the exact installed original and local SYSTEM first,
        // while the original process still holds only stdio.
        let site=DeployedSite::select_for_account_reset()?;
        let tty=PublicConfirmation::select()?;
        let (system,original)=site.original();let e=system.entity();
        let m=Manifest{intent:self.intent,nonce:random()?,system:system.path().to_owned(),
            entity:[e.dev,e.ino,system.require_same_named_mount()?.id],plan:site.plan().clone(),
            order:RustOrder{actor:site.payload().subject.tuple().0,approval:self.approval,op:self.op,
                role:Desired::Reset{previous:self.previous,generation:self.generation}},
            authority:Authority::Deployed(DeployedScope{subject:site.payload().subject,site:site.site_sha(),
                helper:self.helper,providers:self.providers})};
        let (observed,evidence)=transact_deployed(&site,m,&tty)?;
        // Receipt is proof of this original operation only, not proof that
        // its vault is current, nor permission to resume or allocate any name.
        let (code,report)=match observed{
            Some(r) if r.state==3=>(
                0,format!("原共用账户密码操作 {} 已确认（{}；前序 {}／代次 {}，本代次 {}，原申请 SHA-256 {}）。未启动服务，也未验证客户端登录。\n",
                    hex(&self.op),if self.intent==Intent::Fresh{"Web 与 SMB 共用的新代次"}else{"只读历史回执，不判断当前密码"},
                    hex(&self.previous),self.generation,r.target.generation,hex(&r.action))),
            Some(r) if r.state==2&&self.intent==Intent::Lookup=>(
                2,format!("同一原共用账户操作 {} 尚未确认（原申请 SHA-256 {}）。未推进或清理；两个入口均不回退到旧凭据。\n",
                    hex(&self.op),hex(&r.action))),
            None if self.intent==Intent::Lookup=>(2,"本次未在原库查到精确原共用账户操作；未写数据库或凭据，不确认可重试或删除任何材料。\n".to_owned()),
            _=>return Err(bad("not an admitted result for this original shared maintenance mode")),
        };
        let report_error=|e:io::Error|match self.intent{
            Intent::Fresh=>io::Error::new(e.kind(),format!(
                "共用账户重置结果未送达；仅查询同一原操作 {} / 批准 {}，勿再发起或清理。{e}",hex(&self.op),hex(&self.approval))),
            Intent::Lookup=>e,
        };
        match (self.intent,evidence.as_ref()){
            (Intent::Fresh,Some(exact))=>exact.require(original).map_err(&report_error)?,
            (Intent::Lookup,None)=>(),
            _=>return Err(report_error(bad("missing original shared-account completion evidence"))),
        }
        tty.report(&report).map_err(report_error)?;
        Ok(code)
    }
}
fn hex(bytes:&[u8])->String{bytes.iter().map(|b|format!("{b:02x}")).collect()}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn shared_reset_only_fixed_operation_order_and_exact_expected_predecessor(){
        let parse=|a:&[&str]|Selection::parse(a.iter().map(|s|OsString::from(*s)));
        let (op,approval,previous,d)=(hex(&[1;16]),hex(&[2;16]),hex(&[3;16]),hex(&[4;32]));
        let base=["receipt","--op",&op,"--approval",&approval,"--previous",&previous,"--previous-generation","2","--helper-sha256",&d];
        if !cfg!(feature="private-fresh-install"){assert!(parse(&base).is_err());return;}
        assert!(parse(&base).unwrap().providers.is_none());
        let mut branch=base.to_vec();branch[0]="new";
        assert!(parse(&branch).is_err());
        branch.extend(["--gcrypt-sha256",&d,"--gpgerror-sha256",&d]);
        assert!(parse(&branch).unwrap().providers.is_some());
        branch[0]="receipt";assert!(parse(&branch).is_err());
        for bad in ["0","01","+2","9223372036854775807","٢"]{
            let mut wrong=base;wrong[8]=bad;assert!(parse(&wrong).is_err());
        }
        for extra in ["--password","--system","--site","--login","--resume","--delete","--web"]{
            let mut wrong=base.to_vec();wrong.extend([extra,"fixture"]);assert!(parse(&wrong).is_err());
        }
        let mut no_resume=base;no_resume[0]="resume";assert!(parse(&no_resume).is_err());
    }
}
