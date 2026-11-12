//! Prospective physical slice geometry, separate from the active storage format.
pub(super) const RESERVE_BYTES: u64 = 16;
pub(super) const UUID_COMPONENT_OVERHEAD: usize = 34;
// Product bounds for the projected source chain plus fixed slice directories only.
// These are not filesystem PATH_MAX evidence and exclude object suffixes. Passing
// this preview never proves that a complete object path is writable or executable.
pub(super) const MAX_PATH_BYTES: usize = 4096;
pub(super) const MAX_PATH_COMPONENTS: usize = 64;
pub(super) const SLICE_ROOTS: [&str; 4] = [
    "common-data",
    "deleted-data",
    "zxs-data/common-data",
    "zxs-data/deleted-data",
];
