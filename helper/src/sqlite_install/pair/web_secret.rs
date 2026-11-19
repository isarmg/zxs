//! One Web-specific, in-memory preparation. Not a CLI, input channel, passdb
//! provider, authentication result, setup authority, or database writer.
use argon2::{Algorithm, Argon2, Block, Params, Version};
use rustix::rand::{getrandom, GetRandomFlags};
use std::io;
use zeroize::Zeroizing;

const KIB: u32 = 65_536;
const PASSES: u32 = 3;
const LANES: u32 = 4;
const OUTPUT: usize = 32;

struct WebInput(Zeroizing<Box<[u8]>>);
impl WebInput {
    fn take(bytes: Zeroizing<Box<[u8]>>) -> io::Result<Self> {
        if !(1..=1024).contains(&bytes.len()) || std::str::from_utf8(&bytes).is_err() {
            // Never put the supplied bytes, their digest, or an internal error containing them in diagnostics.
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "expected 1–1024 UTF-8 input bytes for the Web credential"));
        }
        Ok(Self(bytes))
    }
}

/// No Debug/Clone, representation or export to callers. Until a real
/// independently authorized pair issuer exists, no production path consumes
/// any actual password or publishes these values.
pub(super) struct PreparedWeb {
    salt: [u8; 16],
    verifier: Zeroizing<Box<[u8]>>,
}

impl PreparedWeb {
    /// Accept only an owned, non-reallocating input that will be wiped on drop.
    /// Its future production producer must also provide a protected terminal /
    /// process and prevent plaintext spill before this boundary. No argv/env,
    /// conversion through a String, external parameters, or password digest.
    pub(super) fn fresh(bytes: Zeroizing<Box<[u8]>>) -> io::Result<Self> {
        let input = WebInput::take(bytes)?;
        let mut salt = [0; 16];
        loop {
            match getrandom(&mut salt, GetRandomFlags::NONBLOCK) {
                Err(rustix::io::Errno::INTR) => continue,
                Err(e) => return Err(e.into()),
                Ok(16) if salt != [0; 16] => break,
                _ => return Err(io::Error::other("OS did not supply one complete, nonzero random Web salt")),
            }
        }
        Self::derive(input, salt)
    }

    // Deterministic only for the exact already-chosen salt; never offered as an
    // operator choice. A lost or unknown account transaction is a history query,
    // not grounds to re-enter this function, rehash or mint another salt.
    fn derive(input: WebInput, salt: [u8; 16]) -> io::Result<Self> {
        let fail = |_| io::Error::other("fixed Web credential derivation did not complete");
        let params = Params::new(KIB, PASSES, LANES, Some(OUTPUT)).map_err(fail)?;
        let count = params.block_count();
        if count != KIB as usize || Block::SIZE != 1024 {
            return Err(io::Error::other("unexpected Argon2 workspace shape"));
        }
        // Allocate the whole workspace before any blocks can contain password-
        // dependent state. The RustCrypto Block implements Zeroize; this owner
        // clears the entire allocation on success, ordinary errors or unwind.
        let mut workspace = Zeroizing::new(Vec::<Block>::new());
        workspace.try_reserve_exact(count).map_err(|_| io::Error::other("no room for the fixed 64 MiB Web workspace"))?;
        workspace.resize(count, Block::new());
        let mut verifier = Zeroizing::new(vec![0u8; OUTPUT].into_boxed_slice());
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password_into_with_memory(&input.0, &salt, &mut verifier, workspace.as_mut_slice()).map_err(fail)?;
        Ok(Self { salt, verifier })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input(b: &[u8]) -> Zeroizing<Box<[u8]>> { Zeroizing::new(b.to_vec().into_boxed_slice()) }
    fn value(key: &str) -> Vec<u8> {
        include_str!("../../../tests/sqlite/first-account-v1.txt").lines()
            .filter_map(|l|l.split_once('=')).find(|p|p.0==key).unwrap().1
            .as_bytes().chunks_exact(2).map(|p|u8::from_str_radix(std::str::from_utf8(p).unwrap(),16).unwrap()).collect()
    }

    #[test]
    fn upstream_raw_web_verifier_matches_the_other_implementations_public_example() {
        let salt: [u8;16]=value("web_salt").try_into().unwrap();
        let secret=WebInput::take(input(b"correct horse battery staple")).unwrap();
        let made=PreparedWeb::derive(secret,salt).unwrap();
        assert_eq!(made.salt,salt);
        // Do not make a real issuer's secret-bearing type printable to run this fixture.
        assert!(&**made.verifier == value("web_verifier").as_slice());
    }

    #[test]
    fn byte_limits_and_verbatim_input_do_not_normalize_trim_or_truncate() {
        assert!(WebInput::take(input(b"")).is_err()); assert!(WebInput::take(input(&[0xff])).is_err());
        assert!(WebInput::take(input(&[b'x';1025])).is_err());
        for b in [b" leading and trailing ".as_slice(), b"\0", b"e\xcc\x81", &[b'x';1024]] {
            let admitted=WebInput::take(input(b)).unwrap(); assert!(&**admitted.0 == b);
        }
    }
}
