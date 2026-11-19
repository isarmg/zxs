//! Authentication observation at the exact first-account frontier. The
//! holder cannot operate a FILE, create a share, issue credentials or hand
//! the original FD to a caller. Future grants need their own real decisions.
use super::*;
use crate::{identity::AccountId,ordinary::first_account::{First,MAX_FIRST}};

fn denied()->io::Error{io::Error::new(io::ErrorKind::PermissionDenied,"not authorized")}

pub(crate) struct FirstAuthority{held:Ready,birth:First}
impl FirstAuthority{
    /// Both arguments come from a trusted service configuration. No network
    /// interface is provided here; a client must never supply a SYSTEM path.
    pub(crate) fn open(system:&Path,expected_pool:PoolId)->io::Result<Self>{
        let(held,birth)=Ready::open(system,MAX_RECORD+MAX_FIRST,|record,tail,floor|{
            if record.info.id!=expected_pool{return Err(denied());}
            First::confirmed(expected_pool,record.info.initialized_by,record.digest(),tail,floor)
        })?;
        Ok(Self{held,birth})
    }
    pub(crate) fn authenticate(&self,login:&str,password:&str)->io::Result<Principal<'_>>{
        self.authenticate_at(login,password,||Ok(()))
    }
    fn authenticate_at(&self,login:&str,password:&str,after:impl FnOnce()->io::Result<()>)->io::Result<Principal<'_>>{
        // No SYSTEM directory lock during the expensive password check, but
        // the original regular SH remains held. A change cannot turn the old
        // KDF result into an authentication against a later account cut.
        if !self.birth.password_matches(login,password)?{return Err(denied());}
        after()?;self.held.at_cut(||Ok(Principal{source:self}))
    }
}
/// A borrowed proof from this native reader, not a printable bearer token.
/// It cannot survive the original register holder or a changed prefix.
pub(crate) struct Principal<'a>{source:&'a FirstAuthority}
#[derive(Clone,Copy)]
pub(crate) enum Intent{ManageAccounts,PublishShares,ManageReadGrants,Browse,ReadCurrent,ReadVersion}
#[derive(Debug,Eq,PartialEq)]
pub(crate) struct IdentityObservation{
    pub(crate) pool:PoolId,pub(crate) account:AccountId,pub(crate) credential_epoch:u64,
    pub(crate) credential_operation:OperationId,pub(crate) credential_decision:[u8;32],
}
impl Principal<'_>{
    /// Observes authority; it does not perform any of the management actions.
    /// A first-account-only prefix contains no published share and no read grant.
    pub(crate) fn observe(&self,intent:Intent)->io::Result<IdentityObservation>{
        let a=self.source;
        a.held.at_cut(||match intent{
            Intent::ManageAccounts|Intent::PublishShares|Intent::ManageReadGrants=>Ok(IdentityObservation{
                pool:a.held.record.info.id,account:a.birth.account,credential_epoch:1,
                credential_operation:a.birth.operation,credential_decision:a.birth.decision,
            }),
            Intent::Browse|Intent::ReadCurrent|Intent::ReadVersion=>Err(denied()),
        })
    }
}

#[cfg(test)]
#[path="../../tests/unit/first_account.rs"]
mod tests;
