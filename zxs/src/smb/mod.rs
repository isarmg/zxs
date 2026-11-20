//! Private, from-spec SMB 3.1.1 implementation. No listener or storage authority.
//! The existing server's startup, credentials and workers are not wired here.
#[allow(dead_code, reason = "input layer is staged before the network runtime")]
mod wire;

#[cfg(test)]
#[path = "../test/smb/mod.rs"]
mod tests;
