//! Argon2id verification for the caller's selected account.
use std::{
    ffi::{CString, c_char, c_int, c_void},
    io,
    sync::{Mutex, MutexGuard, TryLockError},
    thread,
    time::{Duration, Instant},
};

const SLOT_WAIT: Duration = Duration::from_millis(500);
const SLOT_POLL: Duration = Duration::from_millis(10);

fn unavailable() -> io::Error {
    io::Error::new(io::ErrorKind::WouldBlock, "password verifier unavailable")
}

fn acquire_slot<'a>(
    slot: &'a Mutex<()>,
    budget: Duration,
    stopping: &dyn Fn() -> bool,
) -> io::Result<MutexGuard<'a, ()>> {
    let start = Instant::now();
    let mut contended = false;
    loop {
        if stopping() {
            return Err(unavailable());
        }
        match slot.try_lock() {
            Ok(held) => {
                return if stopping() || (contended && start.elapsed() >= budget) {
                    Err(unavailable())
                } else {
                    Ok(held)
                };
            }
            Err(TryLockError::WouldBlock) => contended = true,
            Err(TryLockError::Poisoned(_)) => {
                return Err(io::Error::other("password verifier unavailable"));
            }
        }
        let remaining = budget.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            return Err(unavailable());
        }
        thread::sleep(SLOT_POLL.min(remaining));
        if start.elapsed() >= budget {
            return Err(unavailable());
        }
    }
}

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
    pub(crate) fn matches(&self, password: &str, stopping: &dyn Fn() -> bool) -> io::Result<bool> {
        // A single process can allocate at most one 64 MiB verification at once.
        // Per-client/remote login throttling remains the network endpoint's job.
        static KDF: Mutex<()> = Mutex::new(());
        if password.is_empty() || password.len() > 1024 {
            return Ok(false);
        }
        #[cfg(test)]
        let _fixture = {
            static FIXTURE_KDF: Mutex<()> = Mutex::new(());
            FIXTURE_KDF.lock().expect("password fixture lock poisoned")
        };
        let _slot = acquire_slot(&KDF, SLOT_WAIT, stopping)?;
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
