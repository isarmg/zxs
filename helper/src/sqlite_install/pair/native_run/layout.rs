mod deployment_check;
mod import_inventory;
mod retirement_check;
// Only the authenticated lower-privilege process opens the actual SQLite ledger.
use super::{
    bad,
    manifest::read_sealed,
    transport::{Channel, Kind},
    worker::original_parent,
};
use crate::{
    path_rules::MAX_PATH_BYTES,
    pinned::{Kind as PathKind, Pinned, helper_elf::ForegroundElf},
    smb_profile::first::{Id, MAX_FOLDERS},
    sqlite_install::{
        approval,
        catalog::{self, Initial},
        originals::SelectedSystem,
        pair::fresh_issuer::{ExplicitSubject, InitialDatabase, require_private_install},
        pair::{
            domain::{request_live_approval, sealed},
            observed,
        },
    },
};
use helper_linux_evidence::diagnostic_process::account_sql_descriptors;
use rustix::{
    net::UCred,
    process::{
        DumpableBehavior, Gid, Signal, Uid, dumpable_behavior, getppid, set_dumpable_behavior,
        set_parent_process_death_signal,
    },
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    ffi::OsString,
    io,
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const DOMAIN: &[u8] = b"helper exact init selection v10\0";
const FRESH: &[u8] = b"ZXS-INITIAL-SESSION-1\0";
const FRESH_SIZE: usize = FRESH.len() + 16 + 8 + 32 + 32;
const BUDGET: usize =
    DOMAIN.len() + 32 + 3 * 16 + 3 * 8 + 1 + (MAX_FOLDERS + 1) * (2 + MAX_PATH_BYTES) + FRESH_SIZE;

#[cfg(test)]
#[derive(Debug)]
pub(in crate::sqlite_install::pair) struct Choice {
    pub(in crate::sqlite_install::pair) path: PathBuf,
}
#[derive(Clone, Copy)]
pub(in crate::sqlite_install::pair) struct FreshScope {
    subject: ExplicitSubject,
    helper: [u8; 32],
    init_sha: [u8; 32],
}
impl FreshScope {
    fn encode(self, out: &mut Vec<u8>) -> io::Result<()> {
        require_private_install()?;
        if self.helper == [0; 32] || self.init_sha == [0; 32] {
            return Err(bad_question());
        }
        let (actor, uid, gid) = self.subject.tuple();
        out.extend_from_slice(FRESH);
        out.extend(actor);
        out.extend(uid.to_be_bytes());
        out.extend(gid.to_be_bytes());
        out.extend(self.helper);
        out.extend(self.init_sha);
        Ok(())
    }
    fn decode(bytes: &[u8]) -> io::Result<Self> {
        require_private_install()?;
        let r = bytes
            .strip_prefix(FRESH)
            .filter(|r| r.len() == 88)
            .ok_or_else(bad_question)?;
        let subject = ExplicitSubject::checked(
            r[..16].try_into().map_err(|_| bad_question())?,
            u32::from_be_bytes(r[16..20].try_into().map_err(|_| bad_question())?),
            u32::from_be_bytes(r[20..24].try_into().map_err(|_| bad_question())?),
        )?;
        let helper = r[24..56].try_into().map_err(|_| bad_question())?;
        let init_sha = r[56..].try_into().map_err(|_| bad_question())?;
        if helper == [0; 32] || init_sha == [0; 32] {
            return Err(bad_question());
        }
        Ok(Self {
            subject,
            helper,
            init_sha,
        })
    }
}
struct Question {
    nonce: [u8; 32],
    actor: Id,
    pool: Id,
    init: Id,
    location: [u64; 3],
    system: PathBuf,
    paths: Vec<PathBuf>,
    fresh: FreshScope,
}
fn bad_question() -> io::Error {
    bad("wrong scoped first-layout original INIT selection")
}
impl Question {
    fn encode(&self) -> io::Result<Vec<u8>> {
        fn path(b: &mut Vec<u8>, p: &Path) -> io::Result<()> {
            let p = p.as_os_str().as_bytes();
            crate::path_rules::Kind::StorageRoot
                .check_path(p)
                .map_err(bad)?;
            b.extend(
                u16::try_from(p.len())
                    .map_err(|_| bad_question())?
                    .to_be_bytes(),
            );
            b.extend(p);
            Ok(())
        }
        if self.nonce == [0; 32]
            || [self.actor, self.pool, self.init].contains(&[0; 16])
            || self.system.to_str().is_none()
            || self.location[1] == 0
            || self.location[2] == 0
            || !(1..=MAX_FOLDERS).contains(&self.paths.len())
        {
            return Err(bad_question());
        }
        let mut b = DOMAIN.to_vec();
        b.extend(self.nonce);
        b.extend(self.actor);
        b.extend(self.pool);
        b.extend(self.init);
        for n in self.location {
            b.extend(n.to_be_bytes());
        }
        path(&mut b, &self.system)?;
        b.push(u8::try_from(self.paths.len()).map_err(|_| bad_question())?);
        let mut seen = BTreeSet::new();
        for p in &self.paths {
            if self.system == *p || !seen.insert(p.as_os_str().as_bytes()) {
                return Err(bad_question());
            }
            path(&mut b, p)?;
        }
        if self.fresh.subject.tuple().0 != self.actor {
            return Err(bad_question());
        }
        self.fresh.encode(&mut b)?;
        Ok(b)
    }
    fn decode(bytes: &[u8]) -> io::Result<Self> {
        fn take<'a>(b: &mut &'a [u8], n: usize) -> io::Result<&'a [u8]> {
            let (v, rest) = b.split_at_checked(n).ok_or_else(bad_question)?;
            *b = rest;
            Ok(v)
        }
        fn field<const N: usize>(b: &mut &[u8]) -> io::Result<[u8; N]> {
            Ok(take(b, N)?.try_into().expect("fixed length"))
        }
        fn path(b: &mut &[u8]) -> io::Result<PathBuf> {
            let n = u16::from_be_bytes(field(b)?) as usize;
            if n == 0 || n > MAX_PATH_BYTES {
                return Err(bad_question());
            }
            let raw = take(b, n)?;
            crate::path_rules::Kind::StorageRoot
                .check_path(raw)
                .map_err(bad)?;
            Ok(PathBuf::from(OsString::from_vec(raw.to_vec())))
        }
        if bytes.len() > BUDGET {
            return Err(bad_question());
        }
        let mut b = bytes.strip_prefix(DOMAIN).ok_or_else(bad_question)?;
        let nonce = field(&mut b)?;
        let actor = field(&mut b)?;
        let pool = field(&mut b)?;
        let init = field(&mut b)?;
        let location = [
            u64::from_be_bytes(field(&mut b)?),
            u64::from_be_bytes(field(&mut b)?),
            u64::from_be_bytes(field(&mut b)?),
        ];
        let system = path(&mut b)?;
        let [count] = field(&mut b)?;
        if !(1..=MAX_FOLDERS).contains(&(count as usize)) {
            return Err(bad_question());
        }
        let paths = (0..count)
            .map(|_| path(&mut b))
            .collect::<io::Result<Vec<_>>>()?;
        let fresh = FreshScope::decode(b)?;
        let q = Self {
            nonce,
            actor,
            pool,
            init,
            location,
            system,
            paths,
            fresh,
        };
        if q.encode()?.as_slice() != bytes {
            return Err(bad_question());
        }
        Ok(q)
    }
}
fn place(pin: &Pinned) -> io::Result<[u64; 3]> {
    let e = pin.entity();
    Ok([e.dev, e.ino, pin.require_same_named_mount()?.id])
}
fn database_payload(place: [u64; 3]) -> [u8; 65] {
    let mut bytes = [0; 65];
    bytes[0] = 1;
    for (i, n) in place.into_iter().enumerate() {
        let o = 1 + 8 * i;
        bytes[o..o + 8].copy_from_slice(&n.to_be_bytes());
    }
    bytes
}
fn body(i: usize, id: Id) -> io::Result<[u8; 65]> {
    let mut b = [0; 65];
    b[0] = u8::try_from(i).map_err(|_| bad_question())?;
    b[1..17].copy_from_slice(&id);
    Ok(b)
}
fn volume_reply(i: usize, raw: &[u8]) -> io::Result<[u8; 65]> {
    let mut b = [0; 65];
    b[0] = u8::try_from(i).map_err(|_| bad_question())?;
    b[1..33].copy_from_slice(&Sha256::digest(raw));
    Ok(b)
}
fn media_bytes(record: &crate::sqlite_install::catalog::Initial) -> Vec<u8> {
    let mut out = record.marker.to_vec();
    out.extend(record.path.as_os_str().as_bytes());
    out
}
// A missing `expected` can represent only the uniquely issued ordinal-one R.
#[cfg(test)]
fn scoped_material(raw: &[u8], expected: Option<Id>, pool: Id, init: Id) -> io::Result<Initial> {
    let (prefix, path) = raw.split_at_checked(101).ok_or_else(bad_question)?;
    let marker: [u8; 101] = prefix.try_into().expect("control marker width");
    crate::path_rules::Kind::StorageRoot
        .check_path(path)
        .map_err(bad)?;
    let role = if expected.is_some() { 2 } else { 1 };
    if &marker[..20] != b"ZXS-SQLITE-1/VOLUME\0"
        || marker[20] != role
        || marker[21..37] != pool
        || marker[37..53] == [0; 16]
        || marker[37..53] == pool
        || expected.is_some_and(|id| marker[37..53] != id)
        || marker[53..69] != init
        || marker[69..] == [0; 32]
    {
        return Err(bad_question());
    }
    Ok(Initial {
        path: PathBuf::from(OsString::from_vec(path.to_vec())),
        marker,
    })
}
fn send_media(channel: &Channel, scope: &[u8; 32], i: usize, claim: &Initial) -> io::Result<()> {
    let bytes = media_bytes(claim);
    let report = sealed(&bytes)?;
    channel.send_original(Kind::VolumeClaim, scope, &volume_reply(i, &bytes)?, &report)
}
pub(crate) fn worker() -> io::Result<()> {
    let [fd, meta] = account_sql_descriptors()?;
    approval::only_inherited_descriptors(&[3, 4])?;
    let parent = getppid().ok_or_else(bad_question)?;
    original_parent(&fd, parent)?;
    let helper = ForegroundElf::host()?;
    helper.parent_is_same_image(parent)?;
    let raw = read_sealed(&meta, BUDGET)?;
    drop(meta);
    enum QuestionKind {
        Fresh(Question),
        Retirement(retirement_check::Question),
        Deployment(Box<deployment_check::Question>),
        Import(import_inventory::Question),
    }
    let q = if raw.starts_with(retirement_check::DOMAIN) {
        QuestionKind::Retirement(retirement_check::Question::decode(&raw)?)
    } else if raw.starts_with(deployment_check::DOMAIN) {
        QuestionKind::Deployment(Box::new(deployment_check::Question::decode(&raw)?))
    } else if raw.starts_with(import_inventory::DOMAIN) {
        QuestionKind::Import(import_inventory::Question::decode(&raw)?)
    } else {
        let q = Question::decode(&raw)?;
        helper.require_independent_digest(&q.fresh.helper)?;
        QuestionKind::Fresh(q)
    };
    let local = matches!(&q, QuestionKind::Import(_))
        .then(import_inventory::mount_scope)
        .transpose()?;
    let host = approval::host_admin()?;
    drop(helper);
    let (subject, location, path) = match &q {
        QuestionKind::Fresh(q) => (q.fresh.subject, q.location, q.system.as_path()),
        QuestionKind::Retirement(q) => q.original_system(),
        QuestionKind::Deployment(q) => q.original_system(),
        QuestionKind::Import(q) => q.original_system(),
    };
    let (actor, uid, gid) = subject.tuple();
    approval::drop_to_storage(uid, gid)?;
    set_parent_process_death_signal(Some(Signal::TERM))?;
    original_parent(&fd, parent)?;
    set_dumpable_behavior(DumpableBehavior::NotDumpable)?;
    if dumpable_behavior()? != DumpableBehavior::NotDumpable {
        return Err(bad_question());
    }
    let scope: [u8; 32] = Sha256::digest(&raw).into();
    request_live_approval(&fd, parent, &scope, || original_parent(&fd, parent))?;
    let channel = Channel::from_owned_child_session(
        fd,
        UCred {
            pid: parent,
            uid: Uid::ROOT,
            gid: Gid::ROOT,
        },
        scope,
    )?;
    let (empty, [lease]) = channel.wait::<1>(
        Kind::Lease,
        &scope,
        Instant::now() + Duration::from_secs(20),
    )?;
    if empty != [0; 65] {
        return Err(bad_question());
    }
    let pin = match &local {
        Some(scope) => Pinned::local_storage(path, scope)?,
        None => Pinned::already_mounted(path, PathKind::StorageRoot)?,
    };
    if place(&pin)? != location {
        return Err(bad_question());
    }
    let system = SelectedSystem::from_origin(pin, host, uid, gid)?.hold_lent_ex(lease)?;
    let q = match q {
        QuestionKind::Fresh(q) => q,
        QuestionKind::Retirement(q) => return q.inspect(&system, &channel, &scope, parent),
        QuestionKind::Deployment(q) => return q.inspect(&system, &channel, &scope, parent),
        QuestionKind::Import(q) => return q.inspect(&system, &channel, &scope, parent),
    };
    let (catalogue, db) =
        observed::isolated_first_claims(&q.pool, &q.init, &actor, &q.fresh.init_sha, &system)?;
    let place = InitialDatabase::scope(&db, uid)?;
    if place[0] != q.location[0] || place[2] != q.location[2] || getppid() != Some(parent) {
        return Err(bad_question());
    }
    system.require()?;
    channel.send_original(Kind::FreshDatabase, &scope, &database_payload(place), &db)?;
    let chosen = q
        .paths
        .iter()
        .map(|p| {
            catalogue
                .iter()
                .find(|c| {
                    c.marker[20] == 2 && c.path.as_os_str().as_bytes() == p.as_os_str().as_bytes()
                })
                .ok_or_else(|| bad("selected path is not a true ordinary F of this original INIT"))
        })
        .collect::<io::Result<Vec<&Initial>>>()?;
    if getppid() != Some(parent) {
        return Err(bad_question());
    }
    system.require()?;
    let recycle = catalog::initial_recycle(&catalogue)?;
    for (i, volume) in chosen.into_iter().enumerate() {
        channel.send(
            Kind::ChosenFolder,
            &scope,
            &body(i, volume.marker[37..53].try_into().expect("VolumeId"))?,
        )?;
        send_media(&channel, &scope, i, volume)?;
    }
    send_media(&channel, &scope, q.paths.len(), recycle)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_material_binds_a_marker_to_the_exact_approved_volume() {
        let (pool, init) = ([1; 16], [2; 16]);
        let chosen = [0x21; 16];
        let proof = crate::sqlite_install::catalog::Initial {
            path: PathBuf::from(OsString::from_vec(b"/mnt/original-\xfe".to_vec())),
            marker: crate::sqlite_install::catalog::claimed_marker(
                2,
                &pool,
                &chosen,
                &init,
                &[0x31; 32],
            ),
        };
        let raw = media_bytes(&proof);
        assert_eq!(
            scoped_material(&raw, Some(chosen), pool, init)
                .unwrap()
                .path,
            proof.path
        );
        assert!(scoped_material(&raw, Some([0x22; 16]), pool, init).is_err());
        assert!(scoped_material(&raw, None, pool, init).is_err());
        let mut recycle = raw.clone();
        recycle[20] = 1;
        assert!(scoped_material(&recycle, Some(chosen), pool, init).is_err());
        assert_eq!(
            scoped_material(&recycle, None, pool, init).unwrap().path,
            proof.path
        );
        let mut wrong = raw.clone();
        wrong[53] = 0;
        assert!(scoped_material(&wrong, Some(chosen), pool, init).is_err());
        let mut wrong = raw;
        wrong.extend(b"/../other");
        assert!(scoped_material(&wrong, Some(chosen), pool, init).is_err());
    }
    fn fresh_scope() -> FreshScope {
        FreshScope {
            subject: ExplicitSubject::checked([1; 16], 1001, 1002).unwrap(),
            helper: [41; 32],
            init_sha: [42; 32],
        }
    }
    #[test]
    fn fresh_v18_question_roundtrips_the_distinct_exact_init_and_helper_digests() {
        let q = Question {
            nonce: [7; 32],
            actor: [1; 16],
            pool: [2; 16],
            init: [3; 16],
            location: [9, 10, 15],
            system: "/fixture/S".into(),
            paths: vec!["/fixture/F".into()],
            fresh: fresh_scope(),
        };
        if !cfg!(feature = "private-fresh-install") {
            assert!(q.encode().is_err());
            return;
        }
        let raw = q.encode().unwrap();
        let accepted = crate::fs_tools::hex(
            "a4ccc67c9bd77c3f81bb195af060e5e8820424ec69d3aad103968e556b2c14cd",
        )
        .unwrap();
        assert_eq!(<[u8; 32]>::from(Sha256::digest(&raw)), accepted);
        let real = Question::decode(&raw).unwrap().fresh;
        assert_eq!((real.helper, real.init_sha), ([41; 32], [42; 32]));
        assert!(Question::decode(&raw[..raw.len() - 1]).is_err());
        assert!(Question::decode(&raw[..raw.len() - FRESH_SIZE]).is_err());
        let mut tail = raw;
        tail.push(0);
        assert!(Question::decode(&tail).is_err());
    }
    #[test]
    fn exact_raw_paths_are_distinct_and_not_normalized() {
        let mut q = Question {
            nonce: [7; 32],
            actor: [1; 16],
            pool: [2; 16],
            init: [3; 16],
            location: [0, 9, 15],
            system: "/system".into(),
            paths: vec![
                PathBuf::from(OsString::from_vec(b"/mnt/data-\xff".to_vec())),
                "/mnt/Data".into(),
            ],
            fresh: fresh_scope(),
        };
        if !cfg!(feature = "private-fresh-install") {
            assert!(q.encode().is_err());
            return;
        }
        let raw = q.encode().unwrap();
        assert_eq!(Question::decode(&raw).unwrap().paths, q.paths);
        let mut extra = raw.clone();
        extra.push(0);
        assert!(Question::decode(&extra).is_err());
        assert!(Question::decode(&raw[..raw.len() - 1]).is_err());
        q.paths.push(q.paths[0].clone());
        assert!(q.encode().is_err());
        q.paths.pop();
        q.paths[0] = q.system.clone();
        assert!(q.encode().is_err());
    }
}
