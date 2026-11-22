//! One Web-specific, in-memory preparation. Not a CLI, input channel, passdb
//! provider, authentication result, setup authority, or database writer.
use argon2::{Algorithm, Argon2, Block, Params, Version};
use rusqlite::{params, Transaction};
use rustix::process::{Resource, getrlimit};
use rustix::rand::{getrandom, GetRandomFlags};
use std::{fs::File,io,os::unix::fs::{FileExt,MetadataExt}};
use zeroize::Zeroizing;

const KIB: u32 = 65_536;
const WORKSPACE_BYTES: u64 = KIB as u64 * 1024;
const PASSES: u32 = 3;
const LANES: u32 = 4;
const OUTPUT: usize = 32;
const JOINT_BYTES:usize=16+OUTPUT;
fn bad_joint_record()->io::Error{io::Error::new(io::ErrorKind::PermissionDenied,"malformed derived Web material for the original low peer")}

struct WebInput<'a>(&'a [u8]);
impl<'a> WebInput<'a> {
    fn check(bytes: &'a [u8]) -> io::Result<Self> {
        if !(1..=1024).contains(&bytes.len()) || std::str::from_utf8(bytes).is_err() {
            // Never put the supplied bytes, their digest, or an internal error containing them in diagnostics.
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "expected 1–1024 UTF-8 input bytes for the Web credential"));
        }
        Ok(Self(bytes))
    }
    fn joint(owner:&'a Zeroizing<Box<[u8]>>,used:usize)->io::Result<Self>{
        let invalid=||io::Error::new(io::ErrorKind::InvalidInput,"invalid shared protected password input");
        let exact=owner.get(..used).ok_or_else(invalid)?;
        if exact.iter().any(|&b|b<32||b==127){return Err(invalid());}
        Self::check(exact)
    }
}

/// No Debug/Clone, representation or export to callers. Until a real
/// independently authorized pair issuer exists, no production path consumes
/// any actual password or publishes these values.
pub(super) struct PreparedWeb {
    salt: [u8; 16],
    verifier: Zeroizing<Box<[u8]>>,
}

// Construct only after mlockall(CURRENT|FUTURE), before entering any password.
pub(super) struct WebWorkspace { blocks: Zeroizing<Vec<Block>>, params: Params }
impl WebWorkspace {
    /// Reject only a guaranteed shortfall before changing site state. The
    /// lowered credential worker still has to lock and reserve real memory.
    pub(super) fn check_inherited_lock_budget() -> io::Result<()> {
        check_lock_budget(getrlimit(Resource::Memlock).current)
    }

    pub(super) fn reserve() -> io::Result<Self> {
        let params = Params::new(KIB, PASSES, LANES, Some(OUTPUT))
            .map_err(|_| io::Error::other("invalid fixed Web credential parameters"))?;
        let count = params.block_count();
        if count != KIB as usize || Block::SIZE != 1024 {
            return Err(io::Error::other("unexpected Argon2 workspace shape"));
        }
        let mut blocks = Zeroizing::new(Vec::<Block>::new());
        blocks.try_reserve_exact(count)
            .map_err(|_| io::Error::other("cannot reserve the complete locked 64 MiB Web workspace"))?;
        blocks.resize(count, Block::new());
        Ok(Self { blocks, params })
    }
}

fn check_lock_budget(soft_limit: Option<u64>) -> io::Result<()> {
    if soft_limit.is_some_and(|bytes| bytes <= WORKSPACE_BYTES) {
        return Err(io::Error::other(
            "the inherited soft MEMLOCK limit must cover the 64 MiB password workspace plus the worker's other locked memory; no site files were created",
        ));
    }
    Ok(())
}

impl PreparedWeb {
    /// Borrow exactly the bytes from the first already-confirmed zeroizing
    /// TTY buffer. Do not consume/copy it before the SMB provider borrows it.
    pub(super) fn fresh_from_joint(owner:&Zeroizing<Box<[u8]>>,used:usize,workspace:WebWorkspace)->io::Result<Self>{
        Self::with_new_salt(WebInput::joint(owner,used)?,workspace)
    }

    fn with_new_salt(input:WebInput<'_>,workspace:WebWorkspace)->io::Result<Self>{
        let mut salt = [0; 16];
        loop {
            match getrandom(&mut salt, GetRandomFlags::NONBLOCK) {
                Err(rustix::io::Errno::INTR) => continue,
                Err(e) => return Err(e.into()),
                Ok(16) if salt != [0; 16] => break,
                _ => return Err(io::Error::other("OS did not supply one complete, nonzero random Web salt")),
            }
        }
        Self::derive(input, salt, workspace)
    }

    // Deterministic only for the exact already-chosen salt; never offered as an
    // operator choice. A lost or unknown account transaction is a history query,
    // not grounds to re-enter this function, rehash or mint another salt.
    fn derive(input: WebInput<'_>, salt: [u8; 16], workspace: WebWorkspace) -> io::Result<Self> {
        let WebWorkspace { mut blocks, params } = workspace;
        let fail = |_| io::Error::other("fixed Web credential derivation did not complete");
        let mut verifier = Zeroizing::new(vec![0u8; OUTPUT].into_boxed_slice());
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password_into_with_memory(input.0, &salt, &mut verifier, blocks.as_mut_slice()).map_err(fail)?;
        Ok(Self { salt, verifier })
    }
    // Internal one-hop export of only the salted Web verifier, never the
    // password, SMB key or generic digest. The sending type is minted only
    // after both derivations succeeded in one actual low invocation.
    pub(super) fn seal_for_joint_peer(self,_:super::terminal::JointPacking)->io::Result<File>{
        let mut record=Zeroizing::new([0;JOINT_BYTES]);
        if self.verifier.len()!=OUTPUT||self.salt==[0;16]{return Err(bad_joint_record());}
        record[..16].copy_from_slice(&self.salt);
        record[16..].copy_from_slice(&self.verifier);
        let file=super::domain::sealed(&record[..])?;
        rustix::fs::fchmod(&file,rustix::fs::Mode::RUSR)?;
        Ok(file)
    }

    // Called only after real original low peer+action and closed one-shot
    // framing. Keep the exact content in the credential worker only.
    pub(super) fn from_joint_peer(file:&File,uid:u32,gid:u32,_:super::native_run::joint_material::ReceivePacking)->io::Result<Self>{
        use rustix::fs::{OFlags,SealFlags,fcntl_get_seals,fcntl_getfl};
        use rustix::io::{FdFlags,fcntl_getfd};
        let expected=SealFlags::WRITE|SealFlags::GROW|SealFlags::SHRINK|SealFlags::EXEC|SealFlags::SEAL;
        let exact=||->io::Result<MetadataIdentity>{
            let m=file.metadata()?;
            let flags=fcntl_getfl(file)?;
            if uid==0||gid==0||!m.is_file()||m.nlink()!=0||m.mode()!=0o100400
                ||m.uid()!=uid||m.gid()!=gid||m.len()!=JOINT_BYTES as u64
                ||flags.contains(OFlags::PATH)||!matches!(flags&OFlags::ACCMODE,OFlags::RDONLY|OFlags::RDWR)
                ||!fcntl_getfd(file)?.contains(FdFlags::CLOEXEC)||!fcntl_get_seals(file)?.contains(expected)
                ||rustix::fs::fstatfs(file)?.f_type as u64!=u64::from(linux_raw_sys::general::TMPFS_MAGIC){
                return Err(bad_joint_record());
            }
            Ok(MetadataIdentity{dev:m.dev(),ino:m.ino(),times:[m.ctime(),m.ctime_nsec(),m.mtime(),m.mtime_nsec()]})
        };
        let original=exact()?;
        let mut data=Zeroizing::new([0u8;JOINT_BYTES]);
        let mut eof=Zeroizing::new([0u8;1]);
        file.read_exact_at(&mut data[..],0)?;
        if file.read_at(&mut eof[..],JOINT_BYTES as u64)?!=0||exact()?!=original||data[..16]==[0;16]{
            return Err(bad_joint_record());
        }
        let salt=data[..16].try_into().expect("fixed Web salt");
        let mut verifier=Zeroizing::new(vec![0u8;OUTPUT].into_boxed_slice());
        verifier.copy_from_slice(&data[16..]);
        Ok(Self{salt,verifier})
    }

    // Bind only to the fixed first Web credential row. Never return verifier
    // bytes to the generic operation receipt or to the SMB provider.
    pub(super) fn insert(&self, tx: &Transaction<'_>, op: &[u8;16]) -> rusqlite::Result<usize> {
        tx.execute("INSERT INTO main.web_password(slot,credential_op,generation,salt,verifier) VALUES(1,?1,1,?2,?3)",
            params![&op[..],&self.salt[..],&self.verifier[..]])
    }
    pub(super) fn reset(&self,tx:&Transaction<'_>,op:&[u8;16],previous:&[u8;16],generation:i64,old:i64)->rusqlite::Result<usize>{
        tx.execute("UPDATE main.web_password SET credential_op=?1,generation=?2,salt=?3,verifier=?4
            WHERE slot=1 AND credential_op=?5 AND generation=?6",params![&op[..],generation,&self.salt[..],&self.verifier[..],&previous[..],old])
    }
    /// Exact shape and values from the retained low material. No output,
    /// digest or allocation of unprotected temporary verifier bytes.
    pub(super) fn require_stored(&self,tx:&Transaction<'_>,op:&[u8;16],generation:i64)->io::Result<()>{
        use subtle::ConstantTimeEq;
        let bad=|_|bad_joint_record();
        let mut select=tx.prepare("SELECT salt,verifier FROM main.web_password
          WHERE slot=1 AND credential_op=?1 AND generation=?2 LIMIT 2").map_err(bad)?;
        let mut rows=select.query(params![&op[..],generation]).map_err(bad)?;
        let r=rows.next().map_err(bad)?.ok_or_else(bad_joint_record)?;
        let salt=r.get_ref(0).map_err(bad)?.as_blob().map_err(|_|bad_joint_record())?;
        let value=r.get_ref(1).map_err(bad)?.as_blob().map_err(|_|bad_joint_record())?;
        if !bool::from(self.salt.as_slice().ct_eq(salt)&self.verifier.as_ref().ct_eq(value))||rows.next().map_err(bad)?.is_some(){
            return Err(bad_joint_record());
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn synthetic(salt: [u8;16], verifier: [u8;32]) -> Self {
        Self { salt, verifier: Zeroizing::new(verifier.to_vec().into_boxed_slice()) }
    }
}

#[derive(PartialEq,Eq)]
struct MetadataIdentity{dev:u64,ino:u64,times:[i64;4]}

#[cfg(test)]
mod tests {
    use super::*;
    fn input(b: &[u8]) -> Zeroizing<Box<[u8]>> { Zeroizing::new(b.to_vec().into_boxed_slice()) }
    fn value(key: &str) -> Vec<u8> {
        include_str!("../../../tests/sqlite/web-argon2id-vector.txt").lines()
            .filter_map(|l|l.split_once('=')).find(|p|p.0==key).unwrap().1
            .as_bytes().as_chunks::<2>().0.iter().map(|p|u8::from_str_radix(std::str::from_utf8(p).unwrap(),16).unwrap()).collect()
    }

    #[test]
    fn inherited_memlock_rejects_limits_that_cannot_cover_workspace_and_worker() {
        for bytes in [0, WORKSPACE_BYTES - 1, WORKSPACE_BYTES] {
            assert!(check_lock_budget(Some(bytes)).is_err());
        }
        assert!(check_lock_budget(Some(WORKSPACE_BYTES + 1)).is_ok());
        assert!(check_lock_budget(None).is_ok());
    }

    #[test]
    fn upstream_raw_web_verifier_matches_the_other_implementations_public_example() {
        let salt: [u8;16]=value("web_salt").try_into().unwrap();
        let owner=input(b"correct horse battery staple");
        let secret=WebInput::check(&owner).unwrap();
        let made=PreparedWeb::derive(secret,salt,WebWorkspace::reserve().unwrap()).unwrap();
        assert_eq!(made.salt,salt);
        // Do not make a real issuer's secret-bearing type printable to run this fixture.
        assert!(&**made.verifier == value("web_verifier").as_slice());
    }

    #[test]
    fn byte_limits_and_verbatim_input_do_not_normalize_trim_or_truncate() {
        assert!(WebInput::check(b"").is_err()); assert!(WebInput::check(&[0xff]).is_err());
        assert!(WebInput::check(&[b'x';1025]).is_err());
        for b in [b" leading and trailing ".as_slice(), b"\0", b"e\xcc\x81", &[b'x';1024]] {
            let admitted=WebInput::check(b).unwrap(); assert!(admitted.0 == b);
        }
    }

    #[test]
    fn joint_account_secret_web_derivation_borrows_the_exact_used_prefix_not_an_unlocked_copy() {
        // Reuse the independently measured Web vector, not a real password.
        let public=b"correct horse battery staple";
        let mut owner=Zeroizing::new(vec![0;1024].into_boxed_slice());
        owner[..public.len()].copy_from_slice(public);
        let admitted=WebInput::joint(&owner,public.len()).unwrap();
        assert!(admitted.0==public&&admitted.0.as_ptr()==owner.as_ptr());
        let salt=value("web_salt").try_into().unwrap();
        let web=PreparedWeb::derive(admitted,salt,WebWorkspace::reserve().unwrap()).unwrap();
        assert!(&**web.verifier==value("web_verifier").as_slice());
        for used in [0,public.len()+1,1025]{assert!(WebInput::joint(&owner,used).is_err());}
        for invalid in [b"no\0null".as_slice(),b"no\x1bcontrols",b"no\x7ferase",&[0xff],&[b'x';1025]]{
            let candidate=input(invalid);assert!(WebInput::joint(&candidate,candidate.len()).is_err());
        }
        let source=input(" e\u{301} 💠 ".as_bytes());
        let passed=WebInput::joint(&source,source.len()).unwrap();
        assert!(passed.0==source.as_ref()&&passed.0.as_ptr()==source.as_ptr());
    }
}
