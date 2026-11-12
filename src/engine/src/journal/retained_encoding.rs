//! Retain pure canonical encoding, never a target's admission or decoded JSON.
use crate::journal::{AuthenticatedMetadataObject, PreparedMetadataObject};
use crate::Result;
use std::sync::Arc;

/// Encoding domain, not proof that a filesystem object address was admitted.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct MetadataDomain(&'static str);

// The parent journal's existing value boundary also constructs this expectation.
// It deliberately adds no parsing or validation ahead of canonical encoding.
pub(super) struct ExpectedMetadataDigest<'a>(pub(super) &'a str);

#[derive(Clone)]
struct CanonicalMetadataDigest(Arc<str>);

pub(crate) struct RawMetadataRead<'a> {
    domain: MetadataDomain,
    expected: ExpectedMetadataDigest<'a>,
    raw: &'a [u8],
    value: json_codec::Value,
}

/// Only this module can construct a retained encoding from authenticated fresh
/// input. A cache can retain or clone it, but cannot replace any of its parts.
#[derive(Clone)]
pub(crate) struct RetainedMetadataEncoding {
    domain: MetadataDomain,
    raw: Arc<[u8]>,
    canonical: Arc<[u8]>,
    digest: CanonicalMetadataDigest,
}

pub(crate) struct AuthenticatedMetadataRead<'a> {
    raw: &'a [u8],
    object: AuthenticatedMetadataObject,
}

impl<'a> RawMetadataRead<'a> {
    /// Address admission and bounded fresh read/accounting belong to the caller.
    /// Parse only JSON here, preserving its position before canonical encoding.
    pub(crate) fn parse(
        kind: &'static str,
        expected: &'a str,
        raw: &'a [u8],
    ) -> Result<Self> {
        let value = storage_contract::strict_json(raw)?;
        Ok(Self {
            domain: MetadataDomain(kind),
            expected: ExpectedMetadataDigest(expected),
            raw,
            value,
        })
    }

    pub(crate) fn domain(&self) -> MetadataDomain {
        self.domain
    }

    pub(crate) fn raw(&self) -> &[u8] {
        self.raw
    }

    pub(crate) fn authenticate(
        self,
        retained: Option<&RetainedMetadataEncoding>,
    ) -> Result<AuthenticatedMetadataRead<'a>> {
        // A lookup key is only a candidate. Recheck both exact inputs inside
        // the authority before combining retained encoding with the fresh Value.
        let (bytes, actual) = match retained.filter(|encoding| encoding.matches(&self)) {
            Some(encoding) => (
                encoding.canonical.as_ref().to_vec(),
                encoding.digest.0.as_ref().to_owned(),
            ),
            None => storage_contract::encode_hashed_value(self.domain.0, &self.value)?,
        };
        let object = AuthenticatedMetadataObject::authenticate_prepared(
            self.expected,
            PreparedMetadataObject { kind: self.domain.0, hash: actual, bytes },
            self.value,
        )?;
        Ok(AuthenticatedMetadataRead { raw: self.raw, object })
    }
}

impl RetainedMetadataEncoding {
    pub(crate) fn matches(&self, read: &RawMetadataRead<'_>) -> bool {
        self.domain == read.domain && self.raw.as_ref() == read.raw
    }

    /// Heap payload only; the shared account must also charge its key/value
    /// storage. Arc handles do not allocate another copy of these payloads.
    pub(crate) fn payload_bytes(&self) -> Option<usize> {
        payload_bytes(self.raw.len(), self.canonical.len(), self.digest.0.len())
    }
}

impl AuthenticatedMetadataRead<'_> {
    pub(crate) fn retained_payload_bytes(&self) -> Option<usize> {
        payload_bytes(self.raw.len(), self.object.bytes().len(), self.object.hash().len())
    }

    /// The caller reserves the single shared memo budget before copying payloads.
    pub(crate) fn retain_encoding(&self) -> RetainedMetadataEncoding {
        RetainedMetadataEncoding {
            domain: MetadataDomain(self.object.kind()),
            raw: Arc::from(self.raw),
            canonical: Arc::from(self.object.bytes()),
            digest: CanonicalMetadataDigest(Arc::from(self.object.hash())),
        }
    }

    pub(crate) fn into_object(self) -> AuthenticatedMetadataObject {
        self.object
    }
}

fn payload_bytes(raw: usize, canonical: usize, digest: usize) -> Option<usize> {
    raw.checked_add(canonical)?.checked_add(digest)
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/journal/retained_encoding_tests.rs"]
mod tests;
