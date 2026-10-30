//! Shared content-open manifest accounting before checksum reads allocate bytes.
use crate::ApiError;
use std::sync::atomic::{AtomicUsize, Ordering};

// One maximum-format manifest can be admitted even during its encoded/decoded
// overlap. Other content opens share this ceiling; it is not a per-handle grant.
const LIMIT: usize = 2 * (512 * 1024 * 1024 + storage_contract::CHECKSUM_HEADER_BYTES);
static BUDGET: Budget = Budget(AtomicUsize::new(0));

struct Budget(AtomicUsize);
pub(crate) struct Reservation<'a> {
    budget: &'a Budget,
    bytes: usize,
}

pub(crate) fn reserve(encoded_bytes: usize) -> Result<Reservation<'static>, ApiError> {
    BUDGET.reserve(encoded_bytes)
}

impl Budget {
    fn reserve(&self, encoded_bytes: usize) -> Result<Reservation<'_>, ApiError> {
        let bytes = encoded_bytes
            .checked_mul(2)
            .filter(|bytes| *bytes <= LIMIT)
            .ok_or_else(|| ApiError::limit("content checksum proof exceeds memory budget"))?;
        self.0
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                used.checked_add(bytes).filter(|total| *total <= LIMIT)
            })
            .map_err(|_| ApiError::limit("content checksum proof memory capacity reached"))?;
        Ok(Reservation {
            budget: self,
            bytes,
        })
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        self.budget.0.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/content_proof_budget.rs"]
mod tests;
