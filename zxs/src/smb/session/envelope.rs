//! Token sequencing for a fresh, bounded, connection-oriented NTLMv2
//! attempt. It stops at an UNVERIFIED authenticate token. No authentication,
//! challenge generator, exported key, session issuance or network output.
use super::{der, ntlm_shape as ntlm};
use crate::smb::wire::{
    message::Compound,
    preauth::{AwaitingRequest, ConnectionHash, RequestKeyContext, SetupRequestHash},
};
use zeroize::Zeroize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) enum Failure { Malformed, Limit, Mechanism, Unsupported, Stage }
pub(in crate::smb) type Result<T> = std::result::Result<T, Failure>;

/// Only across this bounded attempt, for later MIC verification. No Debug,
/// Clone, or unbounded history. Checked range lengths originate in the u16
/// SMB security field (list additionally <=4096); do not log token bytes.
pub(super) struct Saved(Vec<u8>);
impl Saved {
    pub(super) fn from_wire(p: &[u8]) -> Result<Self> {
        if p.len() > usize::from(u16::MAX) { return Err(Failure::Limit); }
        let mut v = Vec::new();
        v.try_reserve_exact(p.len()).map_err(|_| Failure::Limit)?;
        v.extend_from_slice(p);
        Ok(Self(v))
    }
    pub(super) fn bytes(&self) -> &[u8] { &self.0 }
}
impl Drop for Saved { fn drop(&mut self) { self.0.as_mut_slice().zeroize(); } }

pub(super) enum Layer {
    // The Linux kernel's direct NTLMSSP path has no SPNEGO list. It is a
    // separate first-token branch; it is never a fallback after a DER error.
    Direct,
    Spnego { list: Saved, require_list_mic: bool },
}

#[must_use]
pub(in crate::smb) enum First { Select(NeedSelection), Negotiate(HaveNegotiate) }

#[must_use]
pub(in crate::smb) struct NeedSelection { request: SetupRequestHash, layer: Layer }
#[must_use]
pub(in crate::smb) struct AwaitNegotiate { next: AwaitingRequest, layer: Layer }
#[must_use]
pub(in crate::smb) struct HaveNegotiate {
    request: SetupRequestHash, layer: Layer, type1: Saved, first_reply: bool,
}
#[must_use]
pub(in crate::smb) struct AwaitAuthenticate {
    next: AwaitingRequest, layer: Layer, type1: Saved, type2: Saved,
}

/// These fields must be consumed TOGETHER by the strictly bound
/// mechanism verifier with its own originally issued challenge/expiry and
/// configured account epoch. Neither this object, its SID, nor a structurally
/// valid MIC is proof. There is no conversion from this to VerifiedExchange.
#[must_use]
pub(in crate::smb) struct Unverified<'a> {
    pub(super) request: RequestKeyContext,
    pub(super) layer: Layer,
    pub(super) type1: Saved,
    pub(super) type2: Saved,
    pub(super) type3: ntlm::Authenticate<'a>,
    pub(super) list_mic: Option<&'a [u8]>,
}

pub(in crate::smb) fn begin(connection: &ConnectionHash, input: &Compound<'_>) -> Result<First> {
    let (request, blob) = connection.new_attempt().received_token(input).map_err(|_| Failure::Stage)?;
    let (layer, token) = if blob.starts_with(ntlm::MAGIC) {
        (Layer::Direct, Some(blob))
    } else {
        let init = der::initiator_initial(blob)?;
        let layer = Layer::Spnego { list: Saved::from_wire(init.list)?, require_list_mic: !init.preferred };
        // An optimistic token belongs only to the first offered OID. Never
        // decode another mechanism's token as NTLM, whatever its contents.
        (layer, if init.preferred { init.optimistic } else { None })
    };
    if let Some(type1) = token {
        ntlm::negotiate(type1)?;
        Ok(First::Negotiate(HaveNegotiate {
            request, layer, type1: Saved::from_wire(type1)?, first_reply: true,
        }))
    } else {
        Ok(First::Select(NeedSelection { request, layer }))
    }
}

impl NeedSelection {
    /// Match the actual server MORE response to this request's MID/SID and
    /// examine its original field. No optimistic Type2 before a real Type1.
    pub(in crate::smb) fn selected(self, serialized: &[u8]) -> Result<AwaitNegotiate> {
        let (next, blob) = self.request.more_token(serialized).map_err(|_| Failure::Stage)?;
        let mut layer = self.layer;
        let Layer::Spnego { require_list_mic, .. } = &mut layer else { return Err(Failure::Stage); };
        let r = der::subsequent(blob)?;
        if !r.selection || !matches!(r.state, Some(1 | 3)) || r.token.is_some() || r.mic.is_some() ||
            (*require_list_mic && r.state != Some(3)) { return Err(Failure::Stage); }
        *require_list_mic |= r.state == Some(3);
        Ok(AwaitNegotiate { next, layer })
    }
}

impl AwaitNegotiate {
    pub(in crate::smb) fn received(self, input: &Compound<'_>) -> Result<HaveNegotiate> {
        let (request, blob) = self.next.received_token(input).map_err(|_| Failure::Stage)?;
        let r = der::subsequent(blob)?;
        if r.selection || r.mic.is_some() || !matches!(r.state, None | Some(1)) { return Err(Failure::Stage); }
        let type1 = r.token.ok_or(Failure::Stage)?;
        ntlm::negotiate(type1)?;
        Ok(HaveNegotiate { request, layer: self.layer, type1: Saved::from_wire(type1)?, first_reply: false })
    }
}

pub(super) enum ChallengeLayer { Direct, FirstSpnego, LaterSpnego }
impl HaveNegotiate {
    pub(super) fn challenge_offer(&self) -> Result<(u32,ChallengeLayer)> {
        let layer=match &self.layer {
            Layer::Direct => ChallengeLayer::Direct,
            Layer::Spnego { .. } if self.first_reply => ChallengeLayer::FirstSpnego,
            Layer::Spnego { .. } => ChallengeLayer::LaterSpnego,
        };
        Ok((ntlm::negotiate_flags(self.type1.bytes())?,layer))
    }
    /// Parse only the already serialized server MORE reply. This does not
    /// generate, register, expire or authenticate a challenge.
    pub(in crate::smb) fn challenged(self, serialized: &[u8]) -> Result<AwaitAuthenticate> {
        let (next, blob) = self.request.more_token(serialized).map_err(|_| Failure::Stage)?;
        let mut layer = self.layer;
        let type2 = match &mut layer {
            Layer::Direct => blob,
            Layer::Spnego { require_list_mic, .. } => {
                let r = der::subsequent(blob)?;
                let valid_state = if self.first_reply { matches!(r.state, Some(1 | 3)) }
                                  else { matches!(r.state, None | Some(1)) };
                if !valid_state || r.selection != self.first_reply || r.mic.is_some() { return Err(Failure::Stage); }
                *require_list_mic |= r.state == Some(3);
                r.token.ok_or(Failure::Stage)?
            }
        };
        ntlm::challenge(type2)?;
        Ok(AwaitAuthenticate { next, layer, type1: self.type1, type2: Saved::from_wire(type2)? })
    }
}

impl AwaitAuthenticate {
    pub(super) fn actual_challenge(&self) -> &[u8] { self.type2.bytes() }

    pub(in crate::smb) fn received<'a>(self, input: &Compound<'a>) -> Result<Unverified<'a>> {
        let (request, blob) = self.next.received_token(input).map_err(|_| Failure::Stage)?;
        let mut layer = self.layer;
        let (type3, list_mic) = match &mut layer {
            Layer::Direct => (ntlm::authenticate(blob)?, None),
            Layer::Spnego { require_list_mic, .. } => {
                let r = der::subsequent(blob)?;
                if r.selection || !matches!(r.state, None | Some(0 | 1)) { return Err(Failure::Stage); }
                if r.mic.is_some_and(|v| v.len() != 16) { return Err(Failure::Malformed); }
                let auth = ntlm::authenticate(r.token.ok_or(Failure::Stage)?)?;
                // MS-SPNG §3.1.5.1/Appendix A <8>: a MIC-bearing NTLM
                // AUTHENTICATE can require a mechListMIC even with NTLM first.
                // RFC 4178 §5(c)(V): after an explicit request-mic, a
                // missing list MIC ends this three-token NTLM negotiation.
                // This narrow profile likewise never advances a Windows
                // wrapped exchange with a required but missing MIC.
                *require_list_mic |= auth.mic_indicated || r.mic.is_some();
                if *require_list_mic && r.mic.is_none() { return Err(Failure::Stage); }
                (auth, r.mic)
            }
        };
        Ok(Unverified { request: request.into_kdf_context(), layer, type1: self.type1,
                        type2: self.type2, type3, list_mic })
    }
}

#[cfg(test)]
#[path = "../../test/smb/envelope.rs"]
pub(super) mod tests;
