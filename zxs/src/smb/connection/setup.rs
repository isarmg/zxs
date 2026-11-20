//! Synchronous admission of the first SESSION_SETUP. Only one actual received
//! frame, its paid ledger and that same transport's preauth base enter. No
//! GSS continuation output, SID allocation, verifier or authenticated result.
use super::Received;
use crate::smb::session::envelope::{self,Failure,HaveNegotiate,NeedSelection};
use std::sync::atomic::Ordering;

#[must_use]
pub(in crate::smb) enum Start<'raw> {
    Other(Received<'raw>), // no SETUP anywhere; accounting is not authorization
    Existing(Existing<'raw>), // nonzero SID alone never finds or revives a session
    Refused(Refused<'raw>),
    Select(Select<'raw>), // must request the SPNEGO list MIC; no authenticated subject
    Type1(Type1<'raw>),   // must obtain a genuine challenge and trusted verifier
    TransportGone,
}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::smb) enum Why { SetupCompound, Mechanism(Failure) }

/// These wrappers intentionally never return two separately usable handles
/// for the mechanism stage and its network/credit origin. No Clone or public
/// construction. Later phase transitions must consume the wrapper and recheck
/// this original connection before publishing anything.
pub(in crate::smb) struct Existing<'raw> { pub(super) original: Received<'raw> }
pub(in crate::smb) struct Refused<'raw> { original: Received<'raw>, reason: Why }
pub(in crate::smb) struct Select<'raw> { pub(super) original: Received<'raw>, pub(super) phase: Option<NeedSelection> }
pub(in crate::smb) struct Type1<'raw> { original: Received<'raw>, phase: HaveNegotiate }
impl Refused<'_> {
    pub(in crate::smb) fn reason(&self) -> Why { self.reason }
}

impl<'raw> Received<'raw> {
    /// Consume the receipt once. There is no (connection hash, raw frame, MID)
    /// overload. Reserve/return only phase states, never access to an account.
    pub(in crate::smb) fn start_setup(self) -> Start<'raw> {
        if self.debit.origin.closed.load(Ordering::Acquire) { return Start::TransportGone; }
        let members = self.frame.messages();
        if !members.iter().any(|m| m.header.command == 1) { return Start::Other(self); }
        if !self.debit.owed { return Start::TransportGone; }
        let [m] = members else {
            return Start::Refused(Refused { original:self, reason:Why::SetupCompound });
        };
        if m.header.session != 0 { return Start::Existing(Existing { original:self }); }
        if !crate::smb::wire::setup_reply::plain_header(m) {
            return Start::Refused(Refused{original:self,reason:Why::Mechanism(Failure::Stage)});
        }
        let phase = envelope::begin(&self.debit.origin.hash,self.frame);
        if self.debit.origin.closed.load(Ordering::Acquire) { return Start::TransportGone; }
        match phase {
            Ok(envelope::First::Select(phase)) => Start::Select(Select { original:self,phase:Some(phase) }),
            Ok(envelope::First::Negotiate(phase)) => Start::Type1(Type1 { original:self,phase }),
            Err(why) => Start::Refused(Refused { original:self,reason:Why::Mechanism(why) }),
        }
    }
}

#[cfg(test)]
#[path = "../../test/smb/setup_origin.rs"]
pub(in crate::smb::connection) mod tests;
