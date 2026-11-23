//! The public fields have no password or reusable grant. Detached receiver
//! matches the actual ROOT sender, current original setup, inherited lease
//! and sealed scope before using them. This is not another catalog.
use super::{ExplicitSubject, bad, require_private_install};
use crate::{
    path_rules::Kind,
    smb_profile::rust_first::{Origin, RustPlan},
    sqlite_install::product_layout,
};
use sha2::{Digest, Sha256};
use std::{
    io,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

const SITE: &[u8; 16] = b"ZXS-RUST-SITE-1\0";
const DEPLOYED: &[u8; 16] = b"ZXS-DEPLOYED-V1\0";
const MAX_PATH: usize = 4095;
pub(in crate::sqlite_install::pair) const MAX_SITE: usize =
    16 + 16 * 6 + 8 + 64 + 2 * (2 + MAX_PATH);
pub(in crate::sqlite_install::pair) const DEPLOYMENT_BYTES: usize = 64;
const ORDER: &[u8] = b"ZXS-FRESH-CREDENTIAL-1\0";
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::sqlite_install::pair) struct CryptoPins {
    pub(in crate::sqlite_install::pair) helper: [u8; 32],
    pub(in crate::sqlite_install::pair) gcrypt: [u8; 32],
    pub(in crate::sqlite_install::pair) gpgerror: [u8; 32],
}
impl CryptoPins {
    pub(in crate::sqlite_install::pair) fn check(self) -> io::Result<()> {
        if [self.helper, self.gcrypt, self.gpgerror].contains(&[0; 32]) {
            return Err(no());
        }
        Ok(())
    }
}
pub(in crate::sqlite_install::pair) fn no() -> io::Error {
    bad("not the exact fresh standalone ZXS authority")
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(in crate::sqlite_install::pair) struct CredentialScope {
    pub(in crate::sqlite_install::pair) subject: ExplicitSubject,
    pub(in crate::sqlite_install::pair) crypto: CryptoPins,
    pub(in crate::sqlite_install::pair) site: [u8; 32],
    pub(in crate::sqlite_install::pair) init_sha: [u8; 32],
    pub(in crate::sqlite_install::pair) database: [u64; 3],
}
impl CredentialScope {
    pub(in crate::sqlite_install::pair) const WIDTH: usize =
        ORDER.len() + 16 + 8 + 96 + 32 + 32 + 24;
    pub(in crate::sqlite_install::pair) fn encode(self, out: &mut Vec<u8>) -> io::Result<()> {
        require_private_install()?;
        self.check()?;
        let (actor, uid, gid) = self.subject.tuple();
        out.extend_from_slice(ORDER);
        out.extend_from_slice(&actor);
        out.extend_from_slice(&uid.to_be_bytes());
        out.extend_from_slice(&gid.to_be_bytes());
        for hash in [
            self.crypto.helper,
            self.crypto.gcrypt,
            self.crypto.gpgerror,
            self.site,
            self.init_sha,
        ] {
            out.extend_from_slice(&hash);
        }
        for n in self.database {
            out.extend_from_slice(&n.to_be_bytes());
        }
        Ok(())
    }
    fn check(self) -> io::Result<()> {
        self.crypto.check()?;
        if self.site == [0; 32] || self.init_sha == [0; 32] || self.database[1..].contains(&0) {
            return Err(no());
        }
        Ok(())
    }
    pub(in crate::sqlite_install::pair) fn decode(raw: &[u8]) -> io::Result<Self> {
        require_private_install()?;
        if raw.len() != Self::WIDTH {
            return Err(no());
        }
        let mut c = Cursor(raw.strip_prefix(ORDER).ok_or_else(no)?);
        let subject = ExplicitSubject::checked(
            c.a()?,
            u32::from_be_bytes(c.a()?),
            u32::from_be_bytes(c.a()?),
        )?;
        let crypto = CryptoPins {
            helper: c.a()?,
            gcrypt: c.a()?,
            gpgerror: c.a()?,
        };
        let site = c.a()?;
        let init_sha = c.a()?;
        let database = [
            u64::from_be_bytes(c.a()?),
            u64::from_be_bytes(c.a()?),
            u64::from_be_bytes(c.a()?),
        ];
        let s = Self {
            subject,
            crypto,
            site,
            init_sha,
            database,
        };
        s.check()?;
        Ok(s)
    }
}
#[derive(Clone, Eq, PartialEq)]
pub(in crate::sqlite_install::pair) struct SitePayload {
    pub(in crate::sqlite_install::pair) origin: Origin,
    pub(in crate::sqlite_install::pair) subject: ExplicitSubject,
    pub(in crate::sqlite_install::pair) plan_sha: [u8; 32],
    pub(in crate::sqlite_install::pair) elf_sha: [u8; 32],
    pub(in crate::sqlite_install::pair) system: PathBuf,
    pub(in crate::sqlite_install::pair) image: PathBuf,
}
fn path(raw: &[u8]) -> io::Result<PathBuf> {
    Kind::StorageRoot.check_path(raw).map_err(bad)?;
    let s = std::str::from_utf8(raw).map_err(|_| no())?;
    if !(1..=MAX_PATH).contains(&raw.len())
        || s.chars().any(char::is_control)
        || s.split('/').any(|c| c.len() > 255)
    {
        return Err(no());
    }
    Ok(PathBuf::from(s))
}
impl SitePayload {
    pub(in crate::sqlite_install::pair) fn new(
        plan: &RustPlan,
        subject: ExplicitSubject,
        system: &Path,
        image: &Path,
        elf_sha: [u8; 32],
    ) -> io::Result<Self> {
        let s = Self {
            origin: plan.origin,
            subject,
            plan_sha: plan.sha(),
            elf_sha,
            system: system.to_owned(),
            image: image.to_owned(),
        };
        s.check()?;
        Ok(s)
    }
    fn check(&self) -> io::Result<()> {
        let o = self.origin;
        let ids = [o.pool, o.init, o.instance, o.bind, o.setup];
        if ids.contains(&[0; 16])
            || ids.iter().enumerate().any(|(i, v)| ids[..i].contains(v))
            || self.plan_sha == [0; 32]
            || self.elf_sha == [0; 32]
            || self.image.starts_with(&self.system)
            || self.system.starts_with(&self.image)
        {
            return Err(no());
        }
        for p in [&self.system, &self.image] {
            let _ = path(p.as_os_str().as_bytes())?;
        }
        product_layout::media(&self.system).map_err(bad)?;
        if !product_layout::image_separate(&self.image) {
            return Err(no());
        }
        Ok(())
    }
    pub(in crate::sqlite_install::pair) fn encode(&self) -> io::Result<Vec<u8>> {
        self.check()?;
        let o = self.origin;
        let (actor, uid, gid) = self.subject.tuple();
        let mut b =
            Vec::with_capacity(188 + self.system.as_os_str().len() + self.image.as_os_str().len());
        b.extend_from_slice(SITE);
        for id in [o.pool, o.init, o.instance, o.bind, o.setup, actor] {
            b.extend_from_slice(&id);
        }
        for n in [uid, gid] {
            b.extend_from_slice(&n.to_be_bytes());
        }
        for d in [self.plan_sha, self.elf_sha] {
            b.extend_from_slice(&d);
        }
        for p in [&self.system, &self.image] {
            let s = p.as_os_str().as_bytes();
            b.extend_from_slice(
                &u16::try_from(s.len())
                    .map_err(|_| bad("site path exceeds wire limit"))?
                    .to_be_bytes(),
            );
            b.extend_from_slice(s);
        }
        Ok(b)
    }
    pub(in crate::sqlite_install::pair) fn decode(raw: &[u8]) -> io::Result<Self> {
        let mut c = Cursor(
            raw.strip_prefix(SITE)
                .filter(|_| raw.len() <= MAX_SITE)
                .ok_or_else(no)?,
        );
        let origin = Origin {
            pool: c.a()?,
            init: c.a()?,
            instance: c.a()?,
            bind: c.a()?,
            setup: c.a()?,
        };
        let subject = ExplicitSubject::checked(
            c.a()?,
            u32::from_be_bytes(c.a()?),
            u32::from_be_bytes(c.a()?),
        )?;
        let plan_sha = c.a()?;
        let elf_sha = c.a()?;
        let system = c.path()?;
        let image = c.path()?;
        if !c.0.is_empty() {
            return Err(no());
        }
        let s = Self {
            origin,
            subject,
            plan_sha,
            elf_sha,
            system,
            image,
        };
        s.check()?;
        Ok(s)
    }
    pub(in crate::sqlite_install::pair) fn sha(&self) -> io::Result<[u8; 32]> {
        Ok(Sha256::digest(self.encode()?).into())
    }
}
pub(in crate::sqlite_install::pair) fn activation(
    site: &SitePayload,
) -> io::Result<[u8; DEPLOYMENT_BYTES]> {
    let mut bytes = [0; DEPLOYMENT_BYTES];
    bytes[..16].copy_from_slice(DEPLOYED);
    bytes[16..32].copy_from_slice(&site.origin.instance);
    bytes[32..].copy_from_slice(&site.sha()?);
    Ok(bytes)
}
struct Cursor<'a>(&'a [u8]);
impl<'a> Cursor<'a> {
    fn take(&mut self, k: usize) -> io::Result<&'a [u8]> {
        let (a, b) = self.0.split_at_checked(k).ok_or_else(no)?;
        self.0 = b;
        Ok(a)
    }
    fn a<const N: usize>(&mut self) -> io::Result<[u8; N]> {
        self.take(N)?.try_into().map_err(|_| no())
    }
    fn path(&mut self) -> io::Result<PathBuf> {
        let n = u16::from_be_bytes(self.a()?);
        path(self.take(usize::from(n))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn plan() -> RustPlan {
        // Same immutable 140-byte public sample as the ZXS consumer's
        // launch/site/approval.rs (not the 172-byte first-share vector).
        let hex = "5a58532d525553542d534d422d49320011111111111111111111111111111111121212121212121212121212121212121616161616161616161616161616161617171717171717171717171717171717131313131313131313131313131313130b666978747572652e6c616e0133333333333333333333333333333333020004446f63730006e59b9ee694b6";
        let raw = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect::<Vec<_>>();
        RustPlan::decode(&raw).unwrap()
    }
    #[test]
    fn layout_v18_persisted_site_uses_selected_effective_paths_and_rejects_reserved_state() {
        let plan = plan();
        let subject = ExplicitSubject::checked([81; 16], 1001, 1002).unwrap();
        let original = SitePayload::new(
            &plan,
            subject,
            Path::new(product_layout::SYSTEM),
            Path::new(product_layout::IMAGE),
            [82; 32],
        )
        .unwrap();
        let wire = original.encode().unwrap();
        assert!(SitePayload::decode(&wire).unwrap() == original);
        let mut changed = wire.clone();
        let old = product_layout::SYSTEM.as_bytes();
        let path = b"/zxs/state/system";
        let at = changed.windows(old.len()).position(|n| n == old).unwrap();
        changed[at..at + old.len()].copy_from_slice(path);
        assert!(SitePayload::decode(&changed).is_err()); // still exact width, not a framing-only failure
        assert!(
            SitePayload::new(
                &plan,
                subject,
                Path::new(product_layout::SYSTEM),
                Path::new("/zxs/log/zxs"),
                [82; 32]
            )
            .is_err()
        );
    }

    #[test]
    fn fresh_v18_site_and_activation_match_the_independently_measured_zxs_layout() {
        let r = SitePayload::new(
            &plan(),
            ExplicitSubject::checked([31; 16], 1001, 1002).unwrap(),
            Path::new("/fresh S/SYSTEM"),
            Path::new("/opt/zxs/bin/zxs"),
            [41; 32],
        )
        .unwrap();
        let bytes = r.encode().unwrap();
        assert_eq!(bytes.len(), 219);
        assert_eq!(
            r.sha().unwrap(),
            crate::fs_tools::hex(
                "43165db572d843db3e6fb72ce943a88688e4c04a0161f88a6d540141ec38d23d"
            )
            .unwrap()
        );
        assert!(SitePayload::decode(&bytes).unwrap() == r);
        for len in 0..bytes.len() {
            assert!(SitePayload::decode(&bytes[..len]).is_err());
        }
        let p = activation(&r).unwrap();
        assert_eq!(&p[..16], DEPLOYED);
        assert_eq!(&p[16..32], &r.origin.instance);
        assert_eq!(&p[32..], &r.sha().unwrap());
        let mut modified = bytes;
        modified.push(0);
        assert!(SitePayload::decode(&modified).is_err());
    }
}
