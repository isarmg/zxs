//! Names for old engine fixtures that are unrelated to mount-path spelling.
//! Production constructors still require their caller to supply every DiskName.
use std::path::PathBuf;
use storage_engine::DiskName;

pub(crate) fn disks(
    roots: impl IntoIterator<Item = PathBuf>,
) -> impl Iterator<Item = (DiskName, PathBuf)> {
    roots.into_iter().enumerate().map(|(index, root)| {
        let name = DiskName::parse(&format!("Test disk {}", index + 1)).unwrap();
        (name, root)
    })
}
