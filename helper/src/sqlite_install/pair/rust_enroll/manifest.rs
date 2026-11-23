//! Exact nonsecret order from the original root account owner. Decode is
//! not approval. The child still proves ancestry, parent and the lent S.
use super::bad;
use crate::sqlite_install::pair::fresh_issuer::{
    CredentialScope, CryptoPins, ExplicitSubject, compiled_schema,
};
use crate::{
    path_rules::Kind,
    smb_profile::rust_first::{MAX_WIRE, RustPlan},
    sqlite_install::pair::native_sql::rust_vault::{
        Desired, RustChange, RustOrder, SetupSpec, shared_login,
    },
};
use rustix::fs::{SealFlags, fcntl_get_seals};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io,
    os::unix::fs::{FileExt, MetadataExt},
    path::PathBuf,
};

const TAG: &[u8; 18] = b"ZXS-RUST-ENROLL-2\0";
const DEPLOYED_TAG: &[u8] = b"ZXS-DEPLOYED-SHARED-RESET-2\0";
const LIMIT: usize = TAG.len()
    + 2
    + 32
    + 3 * 16
    + 3 * 8
    + 2
    + 4095
    + 2
    + MAX_WIRE
    + 65
    + 16
    + 16
    + 28
    + CredentialScope::WIDTH;
const _: () = assert!(DeployedScope::WIDTH <= CredentialScope::WIDTH);
fn no() -> io::Error {
    bad("wrong sealed original Rust account request")
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::sqlite_install::pair) enum Intent {
    Lookup,
    Fresh,
}
#[derive(Clone, Copy, Eq, PartialEq)]
pub(in crate::sqlite_install::pair) struct DeployedScope {
    pub subject: ExplicitSubject,
    pub site: [u8; 32],
    pub helper: [u8; 32],
    pub providers: Option<([u8; 32], [u8; 32])>,
}
impl DeployedScope {
    const WIDTH: usize = DEPLOYED_TAG.len() + 24 + 128;
    fn check(self) -> io::Result<()> {
        compiled_schema()?;
        if self.site == [0; 32] || self.helper == [0; 32] {
            return Err(no());
        }
        if let Some(p) = self.crypto() {
            p.check()?;
        }
        Ok(())
    }
    pub(super) fn crypto(self) -> Option<CryptoPins> {
        self.providers.map(|(gcrypt, gpgerror)| CryptoPins {
            helper: self.helper,
            gcrypt,
            gpgerror,
        })
    }
    fn encode(self, e: &mut Vec<u8>) -> io::Result<()> {
        self.check()?;
        let (actor, uid, gid) = self.subject.tuple();
        e.extend(DEPLOYED_TAG);
        e.extend(actor);
        e.extend(uid.to_be_bytes());
        e.extend(gid.to_be_bytes());
        let (gcrypt, gpgerror) = self.providers.unwrap_or(([0; 32], [0; 32]));
        for n in [self.site, self.helper, gcrypt, gpgerror] {
            e.extend(n);
        }
        Ok(())
    }
    fn decode(raw: &[u8]) -> io::Result<Self> {
        if raw.len() != Self::WIDTH {
            return Err(no());
        }
        let mut b = Cursor(raw.strip_prefix(DEPLOYED_TAG).ok_or_else(no)?);
        let subject = ExplicitSubject::checked(
            b.fixed()?,
            u32::from_be_bytes(b.fixed()?),
            u32::from_be_bytes(b.fixed()?),
        )?;
        let site = b.fixed()?;
        let helper = b.fixed()?;
        let gcrypt = b.fixed()?;
        let gpgerror = b.fixed()?;
        let providers = if (gcrypt, gpgerror) == ([0; 32], [0; 32]) {
            None
        } else {
            Some((gcrypt, gpgerror))
        };
        let original = Self {
            subject,
            site,
            helper,
            providers,
        };
        original.check()?;
        Ok(original)
    }
}
#[derive(Clone, Copy, Eq, PartialEq)]
pub(in crate::sqlite_install::pair) enum Authority {
    Fresh(CredentialScope),
    Deployed(DeployedScope),
}
impl Authority {
    pub(in crate::sqlite_install::pair) fn helper(self) -> [u8; 32] {
        match self {
            Self::Fresh(f) => f.crypto.helper,
            Self::Deployed(d) => d.helper,
        }
    }
    pub(in crate::sqlite_install::pair) fn crypto(self) -> Option<CryptoPins> {
        match self {
            Self::Fresh(f) => Some(f.crypto),
            Self::Deployed(d) => d.crypto(),
        }
    }
}
pub(in crate::sqlite_install::pair) struct Manifest {
    pub(in crate::sqlite_install::pair) intent: Intent,
    pub(in crate::sqlite_install::pair) nonce: [u8; 32],
    pub(in crate::sqlite_install::pair) system: PathBuf,
    pub(in crate::sqlite_install::pair) entity: [u64; 3],
    pub(in crate::sqlite_install::pair) plan: RustPlan,
    pub(in crate::sqlite_install::pair) order: RustOrder,
    pub(in crate::sqlite_install::pair) authority: Authority,
}
impl Manifest {
    fn require(&self) -> io::Result<()> {
        compiled_schema()?;
        let o = self.plan.origin;
        let w = &self.order;
        let path = self.system.to_str().ok_or_else(no)?.as_bytes();
        Kind::StorageRoot.check_path(path).map_err(bad)?;
        if path.len() > 4095
            || self.entity[1] == 0
            || self.entity[2] == 0
            || self.nonce == [0; 32]
            || [w.op, w.actor, w.approval].contains(&[0; 16])
            || w.op == o.init
        {
            return Err(no());
        }
        match (self.authority, &w.role) {
            (Authority::Fresh(new), Desired::Setup(_))
                if self.intent == Intent::Fresh
                    && new.subject.tuple().0 == w.actor
                    && w.op == o.setup =>
            {
                RustChange::for_fresh(&self.plan, w, new.init_sha).map(drop)
            }
            (
                Authority::Deployed(s),
                Desired::Reset {
                    previous,
                    generation,
                },
            ) if s.subject.tuple().0 == w.actor
                && (self.intent == Intent::Fresh) == s.providers.is_some()
                && ![o.pool, o.init, o.instance, o.bind, w.op, [0; 16]].contains(previous)
                && ![o.pool, o.instance, o.setup, o.bind, w.approval].contains(&w.op)
                && (1..i64::MAX).contains(generation)
                && (*previous == o.setup) == (*generation == 1) =>
            {
                s.check()
            }
            _ => Err(no()),
        }
    }
    pub(in crate::sqlite_install::pair) fn encode(&self) -> io::Result<Vec<u8>> {
        self.require()?;
        let path = self.system.to_str().ok_or_else(no)?.as_bytes();
        let mut e = Vec::with_capacity(LIMIT);
        e.extend_from_slice(TAG);
        e.push(match self.intent {
            Intent::Lookup => 0,
            Intent::Fresh => 1,
        });
        e.extend(self.nonce);
        for id in [self.order.actor, self.order.approval, self.order.op] {
            e.extend(id);
        }
        for n in self.entity {
            e.extend(n.to_be_bytes());
        }
        e.extend((path.len() as u16).to_be_bytes());
        e.extend(path);
        let raw = self.plan.wire();
        e.extend((raw.len() as u16).to_be_bytes());
        e.extend(raw);
        match &self.order.role {
            Desired::Setup(spec) => {
                e.push(1);
                e.extend(spec.account);
                e.extend(spec.record);
                e.extend(spec.sid);
                e.push(spec.login.len() as u8);
                e.extend(&spec.login);
            }
            Desired::Reset {
                previous,
                generation,
            } => {
                e.push(2);
                e.extend(previous);
                e.extend(generation.to_be_bytes());
            }
        }
        match self.authority {
            Authority::Fresh(new) => new.encode(&mut e)?,
            Authority::Deployed(s) => s.encode(&mut e)?,
        }
        if e.len() > LIMIT {
            return Err(no());
        }
        Ok(e)
    }
    pub(in crate::sqlite_install::pair) fn scope(&self) -> io::Result<[u8; 32]> {
        let mut h = Sha256::new();
        h.update(b"ZXS-HELPER/SHARED-ACCOUNT-SCOPE-2\0");
        h.update(self.encode()?);
        Ok(h.finalize().into())
    }
    pub(in crate::sqlite_install::pair) fn read(file: &File) -> io::Result<Self> {
        decode(&seal_bytes(file, LIMIT)?)
    }
}

pub(in crate::sqlite_install::pair) fn seal_bytes(
    file: &File,
    limit: usize,
) -> io::Result<Vec<u8>> {
    let m = file.metadata()?;
    let seals = fcntl_get_seals(file)?;
    let needed = SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL;
    let n = usize::try_from(m.len())
        .ok()
        .filter(|n| (1..=limit).contains(n))
        .ok_or_else(no)?;
    if !m.is_file() || m.nlink() != 0 || !seals.contains(needed) {
        return Err(no());
    }
    let mut out = vec![0; n];
    file.read_exact_at(&mut out, 0)?;
    if file.read_at(&mut [0; 1], n as u64)? != 0 {
        return Err(no());
    }
    Ok(out)
}
pub(in crate::sqlite_install::pair) fn decode(raw: &[u8]) -> io::Result<Manifest> {
    if raw.len() > LIMIT {
        return Err(no());
    }
    let mut b = Cursor(raw.strip_prefix(TAG).ok_or_else(no)?);
    let intent = match b.byte()? {
        0 => Intent::Lookup,
        1 => Intent::Fresh,
        _ => return Err(no()),
    };
    let nonce = b.fixed()?;
    let actor = b.fixed()?;
    let approval = b.fixed()?;
    let op = b.fixed()?;
    let entity = [
        u64::from_be_bytes(b.fixed()?),
        u64::from_be_bytes(b.fixed()?),
        u64::from_be_bytes(b.fixed()?),
    ];
    let system = PathBuf::from(std::str::from_utf8(b.counted(4095)?).map_err(|_| no())?);
    let plan = RustPlan::decode(b.counted(MAX_WIRE)?).map_err(bad)?;
    let role = match b.byte()? {
        1 => Desired::Setup(SetupSpec {
            account: b.fixed()?,
            record: b.fixed()?,
            sid: b.fixed()?,
            login: b.name()?,
        }),
        2 => Desired::Reset {
            previous: b.fixed::<16>()?,
            generation: i64::from_be_bytes(b.fixed()?),
        },
        _ => return Err(no()),
    };
    let authority = if b.0.starts_with(DEPLOYED_TAG) {
        Authority::Deployed(DeployedScope::decode(b.0)?)
    } else {
        Authority::Fresh(CredentialScope::decode(b.0)?)
    };
    let m = Manifest {
        intent,
        nonce,
        system,
        entity,
        plan,
        order: RustOrder {
            actor,
            approval,
            op,
            role,
        },
        authority,
    };
    if m.encode()?.as_slice() != raw {
        return Err(no());
    }
    Ok(m)
}
struct Cursor<'a>(&'a [u8]);
impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        let (a, b) = self.0.split_at_checked(n).ok_or_else(no)?;
        self.0 = b;
        Ok(a)
    }
    fn fixed<const N: usize>(&mut self) -> io::Result<[u8; N]> {
        self.take(N)?.try_into().map_err(|_| no())
    }
    fn byte(&mut self) -> io::Result<u8> {
        let [n] = self.fixed()?;
        Ok(n)
    }
    fn counted(&mut self, limit: usize) -> io::Result<&'a [u8]> {
        let n = usize::from(u16::from_be_bytes(self.fixed()?));
        if !(1..=limit).contains(&n) {
            return Err(no());
        }
        self.take(n)
    }
    fn name(&mut self) -> io::Result<Vec<u8>> {
        let n = usize::from(self.byte()?);
        let b = self.take(n)?;
        if !shared_login(b) {
            return Err(no());
        }
        Ok(b.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(intent: Intent) -> Manifest {
        let fixture = include_str!("../../../../tests/smb-host/rust-install-v2.txt");
        let raw = fixture
            .lines()
            .find_map(|l| l.strip_prefix("wire_hex="))
            .unwrap();
        let bytes = (0..raw.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&raw[i..i + 2], 16).unwrap())
            .collect::<Vec<_>>();
        let plan = RustPlan::decode(&bytes).unwrap();
        Manifest {
            intent,
            nonce: [101; 32],
            system: "/synthetic/S".into(),
            entity: [0, 12, 13],
            order: RustOrder {
                actor: [102; 16],
                approval: [103; 16],
                op: [104; 16],
                role: Desired::Reset {
                    previous: plan.origin.setup,
                    generation: 1,
                },
            },
            authority: Authority::Deployed(DeployedScope {
                subject: ExplicitSubject::checked([102; 16], 1001, 1002).unwrap(),
                site: [105; 32],
                helper: [106; 32],
                providers: (intent == Intent::Fresh).then_some(([107; 32], [108; 32])),
            }),
            plan,
        }
    }
    #[test]
    fn shared_reset_receipt_and_new_are_distinct_immutable_worker_scopes() {
        let q = sample(Intent::Fresh);
        let raw = q.encode().unwrap();
        let decoded = decode(&raw).unwrap();
        assert_eq!(decoded.encode().unwrap(), raw);
        assert_eq!(decoded.scope().unwrap(), q.scope().unwrap());
        assert_ne!(sample(Intent::Lookup).scope().unwrap(), q.scope().unwrap());
        for n in [0, 1, TAG.len(), raw.len() - 1] {
            assert!(decode(&raw[..n]).is_err());
        }
        let mut tail = raw.clone();
        tail.push(0);
        assert!(decode(&tail).is_err());
        let mut cross = raw.clone();
        cross[TAG.len()] = 0;
        assert!(decode(&cross).is_err()); // receipt cannot carry writer providers
        let mut retired = raw.clone();
        retired[TAG.len()] = 2;
        assert!(decode(&retired).is_err());
        let mut blank = raw;
        let n = blank.len();
        blank[n - 64..n - 32].fill(0);
        assert!(decode(&blank).is_err());
        let mut forbidden = sample(Intent::Fresh);
        forbidden.order.role = Desired::Reset {
            previous: [12; 16],
            generation: 1,
        };
        assert!(forbidden.encode().is_err()); // only the true first setup can be generation one
        assert!(
            decode(&sample(Intent::Lookup).encode().unwrap())
                .unwrap()
                .authority
                .crypto()
                .is_none()
        );
    }
}
