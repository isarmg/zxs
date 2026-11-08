//! Original-scope bytes share the object stage's no-clobber filesystem policy.
use super::super::materials::{self, Namespace, PreparedFiles};
use super::*;

fn files(prepared: &PreparedScopes) -> Result<PreparedFiles<'_>> {
    PreparedFiles::new(
        Namespace::Scopes,
        prepared.manifest()?,
        prepared
            .databases()
            .iter()
            .map(|database| (database.path(), database.bytes())),
    )
}
pub(super) fn require_capacity(
    root: &DiskRoot,
    prepared: &PreparedScopes,
    stop: &AtomicBool,
) -> Result<()> {
    materials::require_capacity(root, &files(prepared)?, stop)
}
pub(super) fn install(root: &DiskRoot, prepared: &PreparedScopes, stop: &AtomicBool) -> Result<()> {
    materials::install(root, &files(prepared)?, stop)
}
pub(super) fn verify(root: &DiskRoot, prepared: &PreparedScopes, stop: &AtomicBool) -> Result<()> {
    materials::verify(root, &files(prepared)?, stop)
}
