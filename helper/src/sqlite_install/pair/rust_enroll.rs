//! Offline native Rust-vault credential records; no separate server or legacy installer.
pub(super) mod crypto;
pub(super) mod manifest;
pub(super) mod receipt;
pub(super) mod secret_order;
use super::{Id,bad};
