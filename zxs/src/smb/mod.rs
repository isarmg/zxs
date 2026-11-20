//! Private, from-spec SMB 3.1.1 implementation. No listener or storage authority.
//! The existing server's startup, credentials and workers are not wired here.
#[allow(dead_code, reason = "input layer is staged before the network runtime")]
mod wire;
mod local;
#[allow(dead_code, reason = "per-transport accounting is staged before the network runtime")]
mod connection;
#[allow(dead_code, reason = "signing is staged before the authenticated network runtime")]
mod session;
#[allow(dead_code, reason = "share votes are staged before real storage-open integration")]
mod backend;

#[cfg(test)]
#[path = "../test/smb/mod.rs"]
mod tests;
