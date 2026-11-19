//! Accounts and explicit bootstrap management from the same admitted prefix.
//! These are verified original facts, not a writer, recovery token or FILE ACL.
use super::{credential::Account,Genesis,decision::request,fixed,id};
use crate::{OperationId,fs::invalid,identity::AccountId};
use std::{collections::BTreeMap,io,sync::Arc};

pub(super) const FIRST:u8=9;
pub(super) const ADD:u8=12;
#[derive(Clone,Copy,Eq,PartialEq)]
pub(super) enum Management{ReadGrants,ReadAndWriteGrants}
#[derive(Default)]
pub(super) struct Accounts{
    manager:Option<(AccountId,Management)>,
    by_id:BTreeMap<AccountId,Arc<Account>>,by_login:BTreeMap<String,AccountId>,
}
impl Accounts{
    pub(super) fn first(&self)->Option<&Arc<Account>>{self.manager.and_then(|(id,_)|self.get(id))}
    pub(super) fn get(&self,id:AccountId)->Option<&Arc<Account>>{self.by_id.get(&id)}
    pub(super) fn login(&self,name:&str)->Option<&Arc<Account>>{self.by_login.get(name).and_then(|id|self.get(*id))}

    /// Only a positively established manager can create another account,
    /// publish, or change ACLs. A normal account does not inherit this power.
    pub(super) fn management(&self,b:&mut &[u8])->io::Result<Management>{
        let actor=AccountId(id(b)?);let credential=fixed::<32>(b)?;
        let (manager,scope)=self.manager.ok_or_else(||invalid("no original managing account"))?;
        if manager!=actor||self.get(actor).is_none_or(|a|a.decision!=credential){
            return Err(invalid("wrong managing account or credential decision"));}
        Ok(scope)
    }
    pub(super) fn apply(&mut self,g:&Genesis,kind:u8,op:OperationId,original:[u8;32],body:&[u8],decision:[u8;32])->io::Result<()>{
        let mut b=body;
        let (scope,subject,domain)=match kind{
            FIRST if self.manager.is_none()&&self.by_id.is_empty()=>{
                let scope=match fixed::<1>(&mut b)?{
                    [1]=>Management::ReadGrants,[2]=>Management::ReadAndWriteGrants,
                    _=>return Err(invalid("unknown bootstrap management selection")),
                };
                let subject=AccountId(id(&mut b)?);
                let _operator_ref=id(&mut b)?; // non-secret correlation, not permission
                (Some(scope),subject,b"ZXS-PORTABLE-1/FIRST-ADMIN\0".as_slice())
            }
            ADD=>{
                self.management(&mut b)?;
                (None,AccountId(id(&mut b)?),b"ZXS-PORTABLE-1/ADD-ACCOUNT\0".as_slice())
            }
            _=>return Err(invalid("not a new original account action")),
        };
        let a=Account::credential(subject,op,decision,b)?;
        // After positive parsing the final 16+32 bytes are exactly the
        // random salt and verifier. Neither enters a queryable request hash.
        if request(domain,g.pool,op,&body[..body.len()-48])?!=original{
            return Err(invalid("account action differs from original request"));}
        if self.by_id.contains_key(&subject)||self.by_login.contains_key(a.login()){
            return Err(invalid("original account ID or login is already used"));}
        self.by_login.insert(a.login().to_owned(),subject);
        self.by_id.insert(subject,Arc::new(a));
        if let Some(s)=scope{self.manager=Some((subject,s));}
        Ok(())
    }
}
