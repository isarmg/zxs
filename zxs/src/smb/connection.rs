//! Credit/sequence state for one direct-TCP/445 transport. No socket,
//! credential, live session, file table or command/response authority here.
//! It is intentionally impossible to construct a negotiated state from
//! peer-supplied IDs, a grant integer or an externally supplied transcript.
use super::wire::{
    credit_fields::{Cost, Limits, direct_445_grant, split_sync_reply},
    direct::MAX_FRAME_BYTES, message::{Compound, MAX_COMMANDS},
    negotiate::Offer, preauth::{ConnectionHash, NegotiateRequestHash},
};
use super::wire::{negotiate::CmacChoice, negotiate_reply::{OriginalHeader, Selection}};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

pub(in crate::smb) mod negotiate;
pub(in crate::smb) mod setup;
pub(in crate::smb) mod sessions;

const CAP: u16 = 64; // bound on live number span AND advertised + in-flight requests
const CANCEL: u16 = 0x000c;
const K64: u64 = 65_536;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) enum Fault {
    Opening, NoDialect, NoPreauthHash, Identity, Entropy, Clock, NoCommonSigning, SetupPhase, Closed, Range, FrameSize,
    Resources, Exhausted, WrongBatch, NoReply, WrongReply, SessionLimit, SessionState,
}
pub(in crate::smb) type Result<T> = std::result::Result<T, Fault>;

/// Received public metadata on the original connection, not an identity or
/// grant. MS-SMB2 3.3.5.4 records the actual ClientDialects array, rather
/// than one 3.1.1 bit. FSCTL_VALIDATE_NEGOTIATE_INFO on a 3.1.1 connection
/// requires disconnect (3.3.5.15.12); this state never enables that IOCTL.
struct ClientOffer { dialects_le: Vec<u8>, guid: [u8;16], security_mode: u16, capabilities: u32 }

/// A fresh direct 3.1.1 offer. Before NEGOTIATE the sole available MID is 0.
/// A multi-protocol SMB1 opener is NOT accepted as a fresh direct connection.
#[must_use]
pub(in crate::smb) struct Opening { hash: NegotiateRequestHash, grant: u16, client: ClientOffer, original: OriginalHeader }
impl Opening {
    pub(in crate::smb) fn received(frame: &Compound<'_>) -> Result<Self> {
        let offer = Offer::parse(frame).map_err(|e| match e {
            super::wire::Error::UnsupportedDialect => Fault::NoDialect,
            super::wire::Error::NoPreauthHashInCommon => Fault::NoPreauthHash,
            _ => Fault::Opening,
        })?;
        let [m] = frame.messages() else { return Err(Fault::Opening); };
        let hash = NegotiateRequestHash::received(&offer).map_err(|_| Fault::Opening)?;
        let mut dialects_le = Vec::new();
        dialects_le.try_reserve_exact(offer.dialects_le.len()).map_err(|_| Fault::Resources)?;
        dialects_le.extend_from_slice(offer.dialects_le);
        let client = ClientOffer { dialects_le, guid: offer.client_guid, security_mode: offer.security_mode,
                                   capabilities: offer.capabilities };
        let original = OriginalHeader::from_offer(&offer).map_err(|_| Fault::Opening)?;
        Ok(Self { hash, grant: m.header.credit_request.clamp(1,CAP), client, original })
    }
    /// Initial request may ask for 0; NEGOTIATE must always grant at least 1.
    pub(in crate::smb) fn credit_response(&self) -> u16 { self.grant }
    // Only our serializer can reach this production path. Take exclusive
    // ownership so the bytes checked and hashed below cannot be replaced.
    fn bind(self, actual: Vec<u8>, instance: &sessions::RunningServerGuid) -> Result<OpeningSend> {
        let (actual_grant, limits) = direct_445_grant(&actual).map_err(|_| Fault::Opening)?;
        if actual_grant != self.grant { return Err(Fault::Opening); }
        let server = Selection::emitted(&actual).map_err(|_| Fault::Opening)?;
        if &server.guid != instance.wire() { return Err(Fault::Identity); }
        let signing = self.hash.signing_choice();
        let hash = self.hash.reply(&actual).map_err(|_| Fault::Opening)?;
        Ok(OpeningSend { actual, hash, signing, server, run:instance.share(), grant: self.grant, limits, client: self.client })
    }
}
#[must_use]
pub(in crate::smb) struct OpeningSend {
    actual: Vec<u8>, hash: ConnectionHash, signing: CmacChoice, server: Selection, run: Arc<sessions::Run>,
    grant: u16, limits: Limits, client: ClientOffer,
}
impl OpeningSend {
    pub(in crate::smb) fn bytes(&self) -> &[u8] { &self.actual }
    /// Called by the same transport owner only after successful submission of
    /// these immutable bytes. Failure/partial send: drop, do not open.
    pub(in crate::smb) fn submitted(self) -> Result<Connection> {
        // MS-SMB2 returns a CMAC selection when no signer overlaps. The
        // conformant client rejects it. Do not expose a usable session fork
        // on this server even if a client ignores its own list.
        if self.signing == CmacChoice::NoCommon { return Err(Fault::NoCommonSigning); }
        Ok(Connection { limits: self.limits, client: self.client, server: self.server, in_flight: 0,
            sessions: Vec::new(), origin: Arc::new(Origin { closed: AtomicBool::new(false), hash: self.hash, run:self.run }),
            window: Window { low: 1, next: 1 + u64::from(self.grant), bits: ones(self.grant) } })
    }
}

fn ones(n: u16) -> u64 { if n == CAP { u64::MAX } else { (1u64 << n) - 1 } }

/// Private speculative copy; the live connection itself is not Clone.
/// low is the least available MID (or next when empty). Bits are sparse and
/// never span more than CAP. next is the first never-granted ID. Never issue
/// u64::MAX, used for server-originated notifications; never wrap to zero.
#[derive(Clone, Copy)]
struct Window { low: u64, next: u64, bits: u64 }
impl Window {
    fn available(self) -> u16 { self.bits.count_ones() as u16 }
    fn take(&mut self, id: u64, n: u16) -> bool {
        if !(1..=CAP).contains(&n) { return false; }
        let Some(start) = id.checked_sub(self.low) else { return false; };
        let Some(end) = id.checked_add(u64::from(n)) else { return false; };
        if end > self.next || start >= u64::from(CAP) { return false; }
        let mask = ones(n) << start;
        if self.bits & mask != mask { return false; }
        self.bits &= !mask;
        if self.bits == 0 { self.low = self.next; } else {
            let by = self.bits.trailing_zeros();
            self.low += u64::from(by);
            self.bits >>= by;
        }
        true
    }
    fn grant(&mut self, requested: u16, in_flight: u16) -> Result<u16> {
        let span = self.next.checked_sub(self.low).filter(|n| *n <= u64::from(CAP)).ok_or(Fault::Resources)?;
        let available = self.available();
        let slots = CAP.checked_sub(in_flight).and_then(|n| n.checked_sub(available)).ok_or(Fault::Resources)?;
        let room = (u64::from(CAP) - span)
            .min(u64::from(slots))
            .min(u64::MAX - self.next);
        let wanted = requested.max(u16::from(available == 0));
        let n = u64::from(wanted).min(room) as u16;
        if n == 0 && available == 0 { return Err(Fault::Exhausted); }
        if n != 0 {
            self.bits |= ones(n) << span;
            self.next += u64::from(n);
        }
        Ok(n)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) enum Payload {
    CancelExempt, NoVariableRule, Covered, InvalidParameter,
}
struct Member { mid: u64, cmd: u16, ask: u16, payload: Payload }

/// Accounting record for exactly one original frame, in original member
/// order. No authorization, request execution, cancellation action, key or
/// Open. Async/cancel processing still requires the real RequestList/owner.
#[must_use]
struct Debits {
    origin: Arc<Origin>, entries: Vec<Member>, owed: bool,
}
impl Debits {
    fn payload(&self, index: usize) -> Option<Payload> {
        self.entries.get(index).map(|m| m.payload)
    }
}
impl Drop for Debits {
    fn drop(&mut self) {
        // Abandoning an ordinary debit without a reply cannot recycle IDs or
        // fabricate a grant. A live owner must discard this transport.
        if self.owed { self.origin.closed.store(true, Ordering::Release); }
    }
}

struct Origin { closed: AtomicBool, hash: ConnectionHash, run:Arc<sessions::Run> }

/// The immutable parsed frame, its ledger and that exact connection's
/// negotiated transcript travel together. No public raw/debit pair, no Clone,
/// no self-reference: the original TCP buffer and parsed view outlive this
/// synchronous receipt. A future asynchronous path must own immutable bytes.
#[must_use]
pub(in crate::smb) struct Received<'raw> { frame: &'raw Compound<'raw>, debit: Debits }
impl Received<'_> {
    pub(in crate::smb) fn payload(&self, i: usize) -> Option<Payload> { self.debit.payload(i) }
}

#[must_use]
pub(in crate::smb) struct Connection {
    origin: Arc<Origin>, window: Window, limits: Limits, client: ClientOffer, server: Selection, in_flight: u16,
    sessions: Vec<sessions::Local>, // same run / original transport; InProgress entries only for now
}
impl Drop for Connection {
    fn drop(&mut self) { self.origin.closed.store(true,Ordering::Release); }
}
impl Connection {
    fn alive(&self) -> Result<()> {
        if self.origin.closed.load(Ordering::Acquire) { Err(Fault::Closed) } else { Ok(()) }
    }
    fn stop<T>(&self, f: Fault) -> Result<T> {
        self.origin.closed.store(true,Ordering::Release); Err(f)
    }
    #[cfg(test)]
    fn negotiation(&self) -> Result<&ConnectionHash> {
        self.alive()?; Ok(&self.origin.hash)
    }

    /// This is the ordinary entry. No separately supplied transcript or raw
    /// message can be substituted on the subsequent SESSION_SETUP path.
    pub(in crate::smb) fn receive<'raw>(&mut self, frame: &'raw Compound<'raw>) -> Result<Received<'raw>> {
        let debit = self.debit(frame)?;
        Ok(Received { frame, debit })
    }

    /// Try every original member against ONLY the credits already submitted
    /// to this peer. An invalid span closes the transport: no partial batch
    /// dispatch, no replenishment from later/prospective replies. All members
    /// (including related ones) retain their own header's MID. A correctly
    /// spent but insufficient payload charge is a command error, not a replay.
    fn debit(&mut self, frame: &Compound<'_>) -> Result<Debits> {
        self.alive()?;
        let total: usize = frame.messages().iter().map(|m| m.raw.len()).sum();
        if total as u64 > u64::from(self.limits.transact) + 256 { return self.stop(Fault::FrameSize); }
        let mut shadow = self.window;
        let mut pending = self.in_flight;
        let mut out = Vec::new();
        if out.try_reserve_exact(frame.messages().len()).is_err() { return self.stop(Fault::Resources); }
        let mut owed = false;
        for m in frame.messages() {
            let h = &m.header;
            if h.command == 0 { return self.stop(Fault::Opening); } // already negotiated: no second reply
            let cost = m.credit_cost();
            if matches!(cost, Cost::Small) && m.raw.len() > 68 * 1024 { return self.stop(Fault::FrameSize); }
            let payload = if h.command == CANCEL {
                // Both sync and async forms exempt. Do not find, cancel, free,
                // answer or give credit for any operation by this exemption.
                Payload::CancelExempt
            } else {
                let spend = h.credit_charge.max(1);
                if !shadow.take(h.message,spend) { return self.stop(Fault::Range); }
                if pending == CAP { return self.stop(Fault::Resources); }
                pending += 1;
                owed = true;
                match cost {
                    Cost::Small => Payload::NoVariableRule,
                    Cost::Bytes(n) if n <= u64::from(spend) * K64 => Payload::Covered,
                    Cost::Bytes(_) | Cost::Malformed => Payload::InvalidParameter,
                }
            };
            out.push(Member { mid: h.message, cmd: h.command, ask: h.credit_request, payload });
        }
        self.window = shadow;
        self.in_flight = pending;
        Ok(Debits { origin: Arc::clone(&self.origin), entries: out, owed })
    }

    /// One policy: grant separately per synchronous original response, in
    /// order, up to the requested amount and local sparse-window/slot budget.
    /// Never strand a responsive client at zero. An outstanding debit is not
    /// a credit advertised to the client. No asynchronous grants on this path.
    pub(in crate::smb) fn plan<'a,'b>(&'a mut self, original: &'b mut Received<'_>) -> Result<ReplyPlan<'a,'b>> {
        self.alive()?;
        if !Arc::ptr_eq(&self.origin,&original.debit.origin) { return Err(Fault::WrongBatch); }
        // A generic accounting consumer cannot bind a forged SETUP success
        // (or turn setup+I/O into an authenticated command chain). The phase
        // owner will settle its original request with the verified reply.
        if original.frame.messages().iter().any(|m| m.header.command == 1) { return Err(Fault::SetupPhase); }
        self.plan_debit(&mut original.debit)
    }
    fn plan_debit<'a,'b>(&'a mut self, batch: &'b mut Debits) -> Result<ReplyPlan<'a,'b>> {
        self.alive()?;
        if !Arc::ptr_eq(&self.origin,&batch.origin) { return Err(Fault::WrongBatch); }
        if !batch.owed { return Err(Fault::NoReply); }
        let mut shadow = self.window;
        let mut pending = self.in_flight;
        let mut grants = [0; MAX_COMMANDS];
        for (i,m) in batch.entries.iter().enumerate() {
            if m.cmd != CANCEL {
                pending = pending.checked_sub(1).ok_or(Fault::WrongBatch)?;
                grants[i] = match shadow.grant(m.ask,pending) {
                    Ok(v) => v, Err(e) => return self.stop(e),
                };
            }
        }
        Ok(ReplyPlan { owner: self, batch, shadow, pending, grants })
    }
}

#[must_use]
pub(in crate::smb) struct ReplyPlan<'a,'b> {
    owner: &'a mut Connection, batch: &'b mut Debits, shadow: Window, pending: u16,
    grants: [u16; MAX_COMMANDS],
}
impl<'a,'b> ReplyPlan<'a,'b> {
    /// Original member index; a CANCEL has no corresponding response.
    pub(in crate::smb) fn credit_response(&self, i: usize) -> Option<u16> {
        self.batch.entries.get(i).filter(|m| m.cmd != CANCEL).map(|_| self.grants[i])
    }
    /// Validate the fully serialized, immutable reply/compound. Does NOT
    /// decide success, sign it, validate a session, or perform any command.
    pub(in crate::smb) fn bind<'p>(self, actual: &'p [u8]) -> Result<ReplySend<'a,'b,'p>> {
        self.bind_bytes(std::borrow::Cow::Borrowed(actual))
    }
    fn bind_owned(self, actual: Vec<u8>) -> Result<ReplySend<'a,'b,'static>> {
        self.bind_bytes(std::borrow::Cow::Owned(actual))
    }
    fn bind_bytes<'p>(self, actual: std::borrow::Cow<'p,[u8]>) -> Result<ReplySend<'a,'b,'p>> {
        self.owner.alive()?;
        if actual.len() > MAX_FRAME_BYTES { return Err(Fault::WrongReply); }
        let mut rest = actual.as_ref();
        for (i,m) in self.batch.entries.iter().enumerate() {
            if m.cmd == CANCEL { continue; }
            let (r, tail) = split_sync_reply(rest).map_err(|_| Fault::WrongReply)?;
            if r.mid != m.mid || r.command != m.cmd || r.grant != self.grants[i] ||
                (m.payload == Payload::InvalidParameter && r.status >> 30 != 3) {
                return Err(Fault::WrongReply);
            }
            rest = tail;
        }
        if !rest.is_empty() { return Err(Fault::WrongReply); }
        Ok(ReplySend { plan: self, actual, settled: false })
    }
}

#[must_use]
pub(in crate::smb) struct ReplySend<'a,'b,'p> {
    plan: ReplyPlan<'a,'b>, actual: std::borrow::Cow<'p,[u8]>, settled: bool,
}
impl ReplySend<'_,'_,'_> {
    pub(in crate::smb) fn bytes(&self) -> &[u8] { self.actual.as_ref() }
    /// Only after successful submission of these bytes on THIS transport.
    /// The exclusive borrow holds its ledger between selecting and publishing
    /// credits; no competing response can publish the same sequence range.
    pub(in crate::smb) fn submitted(self) -> Result<()> { self.publish_with(|_| Ok(())) }
    // Stage-specific callers can install only state already prepared for the
    // exact bytes. Any late failure drops this uncertain transport and clears
    // its pending sessions; no new credit range or active phase survives.
    fn publish_with(mut self, install: impl FnOnce(&mut Connection)->Result<()>) -> Result<()> {
        self.plan.owner.alive()?;
        install(self.plan.owner)?;
        self.plan.owner.alive()?;
        self.plan.owner.window = self.plan.shadow;
        self.plan.owner.in_flight = self.plan.pending;
        self.plan.batch.owed = false;
        self.settled = true;
        Ok(())
    }
}
impl Drop for ReplySend<'_,'_,'_> {
    fn drop(&mut self) {
        if !self.settled {
            self.plan.owner.origin.closed.store(true, Ordering::Release);
            self.plan.owner.sessions.clear();
        }
    }
}

#[cfg(test)]
#[path = "../test/smb/credits.rs"]
mod tests;
