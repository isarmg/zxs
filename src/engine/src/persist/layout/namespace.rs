//! Ordinary user components remain below an independently held namespace parent.
use name_policy::LookupName;
use platform_io::fs_safety::EntryName;

pub(crate) fn directory(name: &LookupName) -> EntryName {
    EntryName::new(name.as_str())
        .expect("invariant: an admitted ordinary component is one relative entry")
}
