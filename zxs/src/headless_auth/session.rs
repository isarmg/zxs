//! Short-lived authority. No credential, token or rate state is persistent.
use super::AuthError;
use crate::hash::Sha256;
use std::{
    collections::VecDeque,
    ffi::{c_uint, c_void},
    io,
    sync::Mutex,
    time::{Duration, Instant},
};

pub(super) const EXPIRES_SECONDS: u64 = 900;
const MAX_SESSIONS: usize = 64;
const LOGIN_ATTEMPTS: usize = 8;
const LOGIN_WINDOW: Duration = Duration::from_secs(60);
const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
const TOKEN_LEN: usize = 43;
const GRND_NONBLOCK: c_uint = 1;

unsafe extern "C" {
    fn getrandom(out: *mut c_void, length: usize, flags: c_uint) -> isize;
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Scope(pub(super) [u8; 16], pub(super) [u8; 16]);

struct Entry {
    digest: [u8; 32],
    scope: Scope,
    expires: Instant,
}

#[derive(Default)]
pub(super) struct Sessions {
    entries: Mutex<Vec<Entry>>,
    attempts: Mutex<VecDeque<Instant>>,
}

impl Sessions {
    pub(super) fn allow_login(
        &self,
        clock: impl FnOnce() -> Instant,
    ) -> Result<(), super::LoginError> {
        use super::LoginError;

        let mut attempts = self
            .attempts
            .lock()
            .map_err(|_| LoginError::Auth(AuthError::Unavailable))?;
        let now = clock();
        while attempts
            .front()
            .is_some_and(|old| now.saturating_duration_since(*old) >= LOGIN_WINDOW)
        {
            let _ = attempts.pop_front();
        }
        if attempts.len() == LOGIN_ATTEMPTS {
            let earliest = attempts
                .front()
                .ok_or(LoginError::Auth(AuthError::Unavailable))?;
            let wait = LOGIN_WINDOW.saturating_sub(now.saturating_duration_since(*earliest));
            let seconds = wait.as_secs() + u64::from(wait.subsec_nanos() != 0);
            return Err(LoginError::Throttled(seconds));
        }
        attempts.push_back(now);
        Ok(())
    }

    pub(super) fn issue(&self, scope: Scope) -> Result<String, AuthError> {
        let raw = random().map_err(|_| AuthError::Unavailable)?;
        self.register(scope, Instant::now(), raw)
    }

    fn register(&self, scope: Scope, now: Instant, entropy: [u8; 32]) -> Result<String, AuthError> {
        if entropy == [0; 32] {
            return Err(AuthError::Unavailable);
        }
        let token = encoded(&entropy);
        let digest = token_digest(&token)?;
        let expires = now
            .checked_add(Duration::from_secs(EXPIRES_SECONDS))
            .ok_or(AuthError::Unavailable)?;
        let mut entries = self.entries.lock().map_err(|_| AuthError::Unavailable)?;
        entries.retain(|row| row.expires > now);
        if entries.len() == MAX_SESSIONS
            || entries.iter().any(|row| equal_digest(&row.digest, &digest))
        {
            return Err(AuthError::Unavailable);
        }
        entries.push(Entry {
            digest,
            scope,
            expires,
        });
        Ok(token)
    }

    pub(super) fn prove(
        &self,
        scope: Scope,
        now: Instant,
        value: &str,
    ) -> Result<Instant, AuthError> {
        let (scheme, token) = value.split_once(' ').ok_or(AuthError::Invalid)?;
        if !scheme.eq_ignore_ascii_case("Bearer") {
            return Err(AuthError::Invalid);
        }
        let digest = token_digest(token)?;
        let mut entries = self.entries.lock().map_err(|_| AuthError::Unavailable)?;
        entries.retain(|row| row.expires > now);
        let mut expires = None;
        for row in &*entries {
            if equal_digest(&row.digest, &digest) && row.scope == scope {
                expires = Some(row.expires);
            }
        }
        expires.ok_or(AuthError::Invalid)
    }
}

fn token_digest(token: &str) -> Result<[u8; 32], AuthError> {
    fn digit(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'-' => Some(62),
            b'_' => Some(63),
            _ => None,
        }
    }
    let raw = token.as_bytes();
    if raw.len() != TOKEN_LEN
        || raw.iter().any(|&byte| digit(byte).is_none())
        || raw
            .last()
            .and_then(|&byte| digit(byte))
            .is_none_or(|n| n & 3 != 0)
    {
        return Err(AuthError::Invalid);
    }
    let mut hash = Sha256::new();
    hash.update(b"ZXS-SESSION-V1\0")
        .and_then(|()| hash.update(raw))
        .map_err(|_| AuthError::Unavailable)?;
    Ok(hash.finish())
}

fn equal_digest(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter()
        .zip(b)
        .fold(0, |difference, (&a, &b)| difference | (a ^ b))
        == 0
}

fn encoded(entropy: &[u8; 32]) -> String {
    let mut token = String::with_capacity(TOKEN_LEN);
    for part in entropy.chunks(3) {
        let digit = |i| u32::from(part.get(i).copied().unwrap_or(0));
        let word = digit(0) << 16 | digit(1) << 8 | digit(2);
        for shift in [18, 12, 6, 0].into_iter().take(part.len() + 1) {
            token.push(char::from(ABC[((word >> shift) & 63) as usize]));
        }
    }
    token
}

fn random() -> io::Result<[u8; 32]> {
    let mut bytes = [0; 32];
    loop {
        // SAFETY: a fixed live writable array is passed to the Linux OS RNG.
        let written = unsafe { getrandom(bytes.as_mut_ptr().cast(), bytes.len(), GRND_NONBLOCK) };
        if usize::try_from(written).ok() == Some(bytes.len()) {
            return Ok(bytes);
        }
        if written < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        return Err(io::Error::other("OS session randomness was incomplete"));
    }
}

#[cfg(test)]
#[path = "../../tests/unit/headless_sessions.rs"]
mod tests;
