//! Argon2id verification for the caller's selected account.
use std::{
    ffi::{CString, c_char, c_int, c_void},
    io,
    sync::Mutex,
};

pub(crate) fn login_name(raw: &[u8]) -> io::Result<&str> {
    if !(1..=64).contains(&raw.len())
        || !(raw[0].is_ascii_lowercase() || raw[0].is_ascii_digit())
        || !raw
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(b))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid exact account login",
        ));
    }
    std::str::from_utf8(raw)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid login bytes"))
}
#[derive(Eq, PartialEq)]
pub(crate) struct Verifier {
    salt: [u8; 16],
    tag: [u8; 32],
}
impl Verifier {
    pub(crate) fn new(salt: [u8; 16], tag: [u8; 32]) -> Self {
        Self { salt, tag }
    }
    pub(crate) fn matches(&self, password: &str) -> io::Result<bool> {
        if password.is_empty() || password.len() > 1024 {
            return Ok(false);
        }
        #[cfg(test)]
        let _fixture = {
            static FIXTURE_KDF: Mutex<()> = Mutex::new(());
            FIXTURE_KDF.lock().expect("password fixture lock poisoned")
        };
        // A single process can allocate at most one 64 MiB verification at once.
        // Per-client/remote login throttling remains the network endpoint's job.
        static KDF: Mutex<()> = Mutex::new(());
        let _slot = KDF.try_lock().map_err(|_| {
            io::Error::new(io::ErrorKind::WouldBlock, "password verifier unavailable")
        })?;
        let encoded = CString::new(format!(
            "$argon2id$v=19$m=65536,t=3,p=4${}${}",
            b64(&self.salt),
            b64(&self.tag)
        ))
        .expect("canonical PHC contains no NUL");
        // SAFETY: verification reads the bounded PHC and borrowed password
        // synchronously; the default flags leave the input unchanged.
        let rc =
            unsafe { argon2id_verify(encoded.as_ptr(), password.as_ptr().cast(), password.len()) };
        match rc {
            0 => Ok(true),
            -35 => Ok(false),
            _ => Err(io::Error::other("password verifier unavailable")),
        }
    }
}
fn b64(raw: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for part in raw.chunks(3) {
        let b = |n: usize| u32::from(part.get(n).copied().unwrap_or(0));
        let word = b(0) << 16 | b(1) << 8 | b(2);
        for shift in [18, 12, 6, 0].into_iter().take(part.len() + 1) {
            out.push(char::from(ABC[((word >> shift) & 63) as usize]));
        }
    }
    out
}
// Resolved only by the private static archive compiled by build.rs.
unsafe extern "C" {
    fn argon2id_verify(encoded: *const c_char, pwd: *const c_void, pwdlen: usize) -> c_int;
}

#[cfg(test)]
#[path = "../tests/unit/attached/password/tests.rs"]
mod tests;
