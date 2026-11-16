//! Names for old engine fixtures that are unrelated to mount-path spelling.
//! Production constructors still require their caller to supply every Name.
use name_policy::Name;
use std::path::PathBuf;

pub(crate) fn disks(
    roots: impl IntoIterator<Item = PathBuf>,
) -> impl Iterator<Item = (Name, PathBuf)> {
    roots.into_iter().enumerate().map(|(index, root)| {
        let name = Name::parse(&format!("Test disk {}", index + 1)).unwrap();
        (name, root)
    })
}
