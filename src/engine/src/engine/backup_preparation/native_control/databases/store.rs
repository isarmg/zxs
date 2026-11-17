//! Object-specific scope checks over the common closed, immutable byte store.
use super::materials::{Namespace, PreparedFiles};
use super::*;
use crate::engine::backup_preparation::native_control::completion_fanout::MaterialMode;

fn files(prepared: &PreparedDatabases) -> Result<PreparedFiles<'_>> {
    PreparedFiles::new(
        Namespace::Objects,
        prepared.manifest()?,
        prepared
            .databases()
            .iter()
            .map(|database| (database.path(), database.bytes())),
    )
}
pub(super) fn require_capacity(
    root: &DiskRoot,
    prepared: &PreparedDatabases,
    stop: &AtomicBool,
) -> Result<()> {
    materials::require_capacity(root, &files(prepared)?, stop)
}
pub(super) fn install(root: &DiskRoot, prepared: &PreparedDatabases, mode: MaterialMode, stop: &AtomicBool) -> Result<()> {
    materials::install_with(root, &files(prepared)?, mode, stop)?;
    verify(root, prepared, stop)
}
pub(super) fn verify(
    root: &DiskRoot,
    prepared: &PreparedDatabases,
    stop: &AtomicBool,
) -> Result<()> {
    materials::verify(root, &files(prepared)?, stop)?;
    for database in prepared.databases() {
        crate::history_package::preparation_checkpoint(stop)?;
        database.verify(&root.read_bytes(
            &root.path().join("object-databases").join(database.path()),
            database.bytes().len(),
        )?)?;
    }
    Ok(())
}
