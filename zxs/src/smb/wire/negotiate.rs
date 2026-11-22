//! MS-SMB2 §§2.2.3, 2.2.3.1, 3.3.5.4. This is a *client offer*, not a
//! selected dialect, completed transcript, signing key or authenticated peer.
pub(in crate::smb) const MAX_OPENING_BYTES: usize = 68 * 1024;

use super::{Error, Result, bytes, field, message::{Compound, SIGNED}, u16le, u32le};

const DIALECTS: usize = 64 + 36;
const PREAUTH: u16 = 1;
const ENCRYPTION: u16 = 2;
const COMPRESSION: u16 = 3;
const RDMA: u16 = 7;
const SIGNING: u16 = 8;
const SHA512: u16 = 1;
const SMB311: u16 = 0x0311;
const MAX_CONTEXTS: usize = 64;

#[derive(Clone, Copy)]
pub(in crate::smb) struct AlgorithmIds<'a>(&'a [u8]);

impl<'a> AlgorithmIds<'a> {
    // All counts are u16, so count*2 fits usize on the Linux target. A
    // nonzero little-endian array is bounded by its own context, never by a
    // sibling or by an assumed end of the containing TCP stream.
    fn bounded(data: &'a [u8], at: usize, count: u16) -> Result<Self> {
        if count == 0 {
            return Err(Error::InvalidRequest);
        }
        Ok(Self(bytes(data, at, usize::from(count) * 2)?))
    }

    pub(in crate::smb) fn contains(self, id: u16) -> bool {
        let wanted = id.to_le_bytes();
        self.0.as_chunks::<2>().0.contains(&wanted)
    }

    // Cipher/signing contexts have a count followed by the algorithm array.
    fn optional(data: Option<&'a [u8]>) -> Result<Option<Self>> {
        data.map(|v| Self::bounded(v, 2, u16le(v, 0)?)).transpose()
    }
}

/// This profile signs with AES-128-CMAC in every case. When a syntactically
/// valid signing context is processed, the response must contain exactly one
/// selected ID, even if the peers had no common ID (MS-SMB2 §3.3.5.4).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) enum CmacChoice {
    Absent,
    Offered,
    NoCommon,
}

pub(in crate::smb) struct Offer<'a> {
    // Exact received bytes. Neither the raw request nor its security material
    // should be printed; no Debug or serialization for these views.
    pub(in crate::smb) request: &'a [u8],
    pub(in crate::smb) dialects_le: &'a [u8], // exact original order, not just 3.1.1 presence
    pub(in crate::smb) client_guid: [u8; 16],
    pub(in crate::smb) security_mode: u16,
    pub(in crate::smb) capabilities: u32,
    pub(in crate::smb) salt: &'a [u8],
    signing: Option<&'a [u8]>,
    encryption: Option<&'a [u8]>,
}

impl<'a> Offer<'a> {
    /// Initial, unbound direct-TCP NEGOTIATE. This profile only admits a
    /// standalone initial request: a fresh connection starts with one credit.
    pub(in crate::smb) fn parse(envelope: &Compound<'a>) -> Result<Self> {
        let [msg] = envelope.messages() else {
            return Err(Error::InvalidRequest);
        };
        let h = &msg.header;
        let p = msg.raw;
        if h.command != 0 || h.flags & SIGNED != 0 || h.session != 0 || u16le(p, 64)? != 36 {
            return Err(Error::InvalidRequest);
        }
        if p.len() > MAX_OPENING_BYTES {
            return Err(Error::Limit);
        }
        let dialects = AlgorithmIds::bounded(p, DIALECTS, u16le(p, 66)?)?;
        if !dialects.contains(SMB311) {
            // For older-only offers the last fixed 8 bytes are not context
            // fields. Don't parse them, and never fall back to another dialect.
            return Err(Error::UnsupportedDialect);
        }
        let mut pos = u32le(p, 92)? as usize;
        let count = usize::from(u16le(p, 96)?);
        if count == 0 || !pos.is_multiple_of(8) || pos < DIALECTS + dialects.0.len() {
            return Err(Error::InvalidRequest);
        }
        if count > MAX_CONTEXTS {
            return Err(Error::Limit);
        }

        let mut seen = 0u16;
        let mut preauth = None;
        let mut signing = None;
        let mut encryption = None;
        for i in 0..count {
            let hdr = bytes(p, pos, 8)?;
            let kind = u16le(hdr, 0)?;
            let size = usize::from(u16le(hdr, 2)?);
            // A successful header borrow proves pos+8 fits inside p.
            let start = pos + hdr.len();
            let data = bytes(p, start, size)?;
            let end = start + data.len();

            // MS-SMB2 explicitly lists these duplicate-context errors. Do
            // not reject a new client extension simply because it is unknown.
            if matches!(kind, PREAUTH | ENCRYPTION | COMPRESSION | RDMA | SIGNING) {
                let bit = 1u16 << kind;
                if seen & bit != 0 {
                    return Err(Error::InvalidRequest);
                }
                seen |= bit;
            }
            match kind {
                PREAUTH => {
                    let ids = AlgorithmIds::bounded(data, 4, u16le(data, 0)?)?;
                    let salt_len = usize::from(u16le(data, 2)?);
                    preauth = Some((ids, bytes(data, 4 + ids.0.len(), salt_len)?));
                }
                SIGNING => signing = Some(data),
                ENCRYPTION => encryption = Some(data),
                // Unimplemented contexts are ignored after envelope validation.
                // No client-provided string or feature bit authenticates a host.
                _ => {}
            }
            if i + 1 < count {
                // end <= p.len() <= 68 KiB; skipped alignment bytes need not
                // be zero on receipt and are retained in the raw transcript.
                pos = (end + 7) & !7;
            }
        }
        let (hashes, salt) = preauth.ok_or(Error::InvalidRequest)?;
        if !hashes.contains(SHA512) {
            return Err(Error::NoPreauthHashInCommon);
        }
        Ok(Self {
            request: p, dialects_le: dialects.0, client_guid: field(p, 76)?, security_mode: u16le(p, 68)?,
            capabilities: u32le(p, 72)?, salt,
            signing, encryption,
        })
    }

    /// Process optional algorithms only when the server actually implements
    /// that context; a syntactically valid wire envelope grants no capability.
    pub(in crate::smb) fn signing_algorithms(&self) -> Result<Option<AlgorithmIds<'a>>> {
        AlgorithmIds::optional(self.signing)
    }

    pub(in crate::smb) fn cmac_choice(&self) -> Result<CmacChoice> {
        Ok(match self.signing_algorithms()? {
            None => CmacChoice::Absent,
            Some(ids) if ids.contains(1) => CmacChoice::Offered,
            Some(_) => CmacChoice::NoCommon,
        })
    }

    pub(in crate::smb) fn ciphers(&self) -> Result<Option<AlgorithmIds<'a>>> {
        AlgorithmIds::optional(self.encryption)
    }
}
