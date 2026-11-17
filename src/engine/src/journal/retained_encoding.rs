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
    // Absent only when the bytes actually read were the exact canonical bytes.
    distinct_raw: Option<Arc<[u8]>>,
    canonical: Arc<[u8]>,
    digest: CanonicalMetadataDigest,
}

pub(crate) struct AuthenticatedMetadataRead<'a> {
    raw: &'a [u8],
    object: AuthenticatedMetadataObject,
}

/// A lightweight allocation plan borrowing the freshly authenticated input.
pub(crate) struct MetadataRetention<'a> {
    domain: MetadataDomain,
    distinct_raw: Option<&'a [u8]>,
    canonical: &'a [u8],
    digest: &'a str,
}

/// Canonical recipe bytes borrowed from one immutable owner. This is not a
/// retained disk read: every installation still uses the ordinary held writer.
pub(crate) struct BorrowedMetadataEncoding<'a> {
    kind: &'static str,
    hash: &'a str,
    bytes: &'a [u8],
}

impl BorrowedMetadataEncoding<'_> {
    pub(crate) fn store_at_root(
        &self,
        root: &crate::disk::DiskRoot,
        disk: &crate::DiskConfig,
    ) -> Result<()> {
        super::store_prepared_bytes_at(root, disk, self.kind, self.hash, self.bytes)
    }
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

    /// Keep only a borrow of the exact canonical input after the original
    /// parser, size and expected-digest gates. Equivalent noncanonical JSON is
    /// not a recipe; the caller retains its existing noncanonical-input error.
    pub(crate) fn into_canonical_encoding(self) -> Result<Option<BorrowedMetadataEncoding<'a>>> {
        let kind = self.domain.0;
        let hash = self.expected.0;
        let bytes = self.raw;
        let object = self.authenticate(None)?.into_object().into_prepared(super::MAX_METADATA)?;
        Ok((object.bytes() == bytes).then_some(BorrowedMetadataEncoding { kind, hash, bytes }))
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
        let raw = self.distinct_raw.as_deref().unwrap_or(&self.canonical);
        self.domain == read.domain && raw == read.raw
    }

    pub(crate) fn payload_bytes(&self) -> Option<usize> {
        payload_bytes(
            self.distinct_raw.as_deref().map_or(0, <[u8]>::len),
            self.canonical.len(),
            self.digest.0.len(),
        )
    }
}

impl MetadataRetention<'_> {
    pub(crate) fn payload_bytes(&self) -> Option<usize> {
        payload_bytes(
            self.distinct_raw.map_or(0, <[u8]>::len),
            self.canonical.len(),
            self.digest.len(),
        )
    }

    /// No owned allocation until the single shared account has admitted this
    /// exact payload and its entry storage.
    pub(crate) fn retain(self) -> RetainedMetadataEncoding {
        RetainedMetadataEncoding {
            domain: self.domain,
            distinct_raw: self.distinct_raw.map(Arc::from),
            canonical: Arc::from(self.canonical),
            digest: CanonicalMetadataDigest(Arc::from(self.digest)),
        }
    }
}

impl AuthenticatedMetadataRead<'_> {
    pub(crate) fn retention(&self) -> MetadataRetention<'_> {
        let canonical = self.object.bytes();
        MetadataRetention {
            domain: MetadataDomain(self.object.kind()),
            distinct_raw: (self.raw != canonical).then_some(self.raw),
            canonical,
            digest: self.object.hash(),
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
