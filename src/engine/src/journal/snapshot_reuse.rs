//! Exact canonical-byte reuse, not a parsed-object or filesystem-stamp cache.
use super::*;

// An optimization budget, not an admission or disk-format limit. Larger roots
// simply use complete preparation next time. No page bytes are retained here.
const MAX_REUSE_DESCRIPTOR_BYTES: usize = 8 * 1024 * 1024;

impl SnapshotReuse {
    pub(crate) fn matches_snapshot(&self, hash: &str) -> bool {
        self.hash == hash
    }
}
impl PreparedSnapshot {
    pub(crate) fn reuse_basis(&self) -> Option<SnapshotReuse> {
        let root = self.objects.last()?;
        if root.kind != "snapshot" || root.bytes.len() > MAX_REUSE_DESCRIPTOR_BYTES {
            return None;
        }
        Some(SnapshotReuse {
            descriptor: self.descriptor.clone(),
            hash: self.hash.clone(),
            page_maxima: self
                .fields
                .iter()
                .map(|(field, plan)| (field.clone(), plan.max_page_bytes))
                .collect(),
        })
    }
    /// Frozen fresh bytes and verified reused bytes share one bounded visitor.
    /// Only the requested field is reconstructed if no healthy source exists.
    /// Callback failures (including collision/sync failure) never enter fallback.
    pub(crate) fn visit_field_pages(
        &self,
        field: &str,
        members: &[DiskConfig],
        state: &State,
        mut visit: impl FnMut(&str, &[u8]) -> Result<()>,
    ) -> Result<()> {
        let Some(plan) = self.fields.get(field) else {
            if self.descriptor.pages.contains_key(field) {
                return Err(Error::new(
                    storage_contract::ErrorCode::StateInvalid,
                    "snapshot field lacks an installation plan",
                ));
            }
            return Ok(());
        };
        let expected = self.descriptor.pages.get(field).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "snapshot field lacks its bound references",
            )
        })?;
        if let Some(range) = &plan.objects {
            if range.len() != expected.len() {
                return Err(invalid_reconstruction());
            }
            for (object, hash) in self.objects[range.clone()].iter().zip(expected) {
                if object.kind != "state-page" || object.hash != *hash {
                    return Err(invalid_reconstruction());
                }
                visit(hash, &object.bytes)?;
            }
            return Ok(());
        }
        for (first_missing, hash) in expected.iter().enumerate() {
            match read_published_page(members, hash, plan.max_page_bytes.min(self.maximum)) {
                Ok(bytes) => visit(hash, &bytes)?,
                Err(_) => {
                    // Reconstruct once and verify the entire original sequence,
                    // but never repeat successful callbacks for the prefix.
                    // This invocation already completed their exact barriers.
                    let mut index = 0;
                    snapshot::reconstruct_field(state, field, self.maximum, |object| {
                        if expected.get(index) != Some(&object.hash) {
                            return Err(invalid_reconstruction());
                        }
                        let current = index;
                        index += 1;
                        observe_buffer(object.bytes.len());
                        if current < first_missing {
                            Ok(())
                        } else {
                            visit(&object.hash, &object.bytes)
                        }
                    })?;
                    if index != expected.len() {
                        return Err(invalid_reconstruction());
                    }
                    return Ok(());
                }
            }
        }
        Ok(())
    }
    pub(crate) fn store(&self, members: &[DiskConfig], state: &State) -> Result<String> {
        self.store_with(members, state, |disk, hash, bytes| {
            write_object_bytes(disk, "state-page", hash, bytes)
        })
    }
    fn store_with(
        &self,
        members: &[DiskConfig],
        state: &State,
        mut page: impl FnMut(&DiskConfig, &str, &[u8]) -> Result<()>,
    ) -> Result<String> {
        for field in self.fields.keys() {
            self.visit_field_pages(field, members, state, |hash, bytes| {
                if !members.is_empty() && bytes.len() > MAX_METADATA {
                    return Err(Error::new(
                        storage_contract::ErrorCode::ResourceLimit,
                        "serialized metadata exceeds reader limit",
                    ));
                }
                for disk in members {
                    page(disk, hash, bytes)?;
                }
                Ok(())
            })?;
        }
        // A root cannot become durable before every exact page has completed
        // its per-member file and directory barriers, including reused pages.
        let root = self.objects.last().ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "snapshot root is absent",
            )
        })?;
        if root.kind != "snapshot" || root.hash != self.hash {
            return Err(invalid_reconstruction());
        }
        root.store(members)?;
        Ok(self.hash.clone())
    }
}
fn invalid_reconstruction() -> Error {
    Error::new(
        storage_contract::ErrorCode::ObjectCorrupt,
        "reconstructed snapshot field differs from its exact bound references",
    )
}

fn read_published_page(members: &[DiskConfig], hash: &str, maximum: usize) -> Result<Vec<u8>> {
    let mut first_error = None;
    for disk in members {
        let loaded = (|| {
            let bytes =
                object_storage::read_object_bytes_bounded(disk, "state-page", hash, maximum)?;
            observe_buffer(bytes.len());
            // encode_hashed_object("state-page", page) hashes exactly these
            // canonical bytes after this domain prefix. Expected hashes here
            // originate in a successful same-process publication, never in a
            // generic semantic-hash load or an untrusted restart cache. Under
            // that provenance, raw digest equality proves byte identity with
            // the original already-valid JSON; parsing would add no evidence.
            let mut digest = blake3::Hasher::new();
            digest.update(b"zxs:state-page\0");
            digest.update(&bytes);
            if digest.finalize().to_hex().as_str() != hash {
                return Err(Error::new(
                    storage_contract::ErrorCode::ObjectCorrupt,
                    "published snapshot page bytes differ",
                ));
            }
            Ok(bytes)
        })();
        match loaded {
            Ok(bytes) => return Ok(bytes),
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    Err(first_error.unwrap_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::ObjectCorrupt,
            "published snapshot page has no source",
        )
    }))
}
#[inline]
fn observe_buffer(bytes: usize) {
    #[cfg(test)]
    REUSED_PAGE_PEAK_BYTES.with(|peak| peak.set(peak.get().max(bytes)));
    #[cfg(not(test))]
    let _ = bytes;
}
#[cfg(test)]
thread_local! {
    pub(super) static REUSED_PAGE_PEAK_BYTES: std::cell::Cell<usize> = const {std::cell::Cell::new(0)};
}
#[cfg(test)]
#[path = "../../../../test/engine/unit/journal/snapshot_reuse_tests.rs"]
mod tests;
