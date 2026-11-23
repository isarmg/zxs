mod deployment_check;
pub(crate) use deployment_check::check_deployment;
mod import_inventory;
pub(crate) use import_inventory::ImportCheck;
mod retirement_check;
pub(crate) use retirement_check::RetirementCheck;
// Only the authenticated lower-privilege process opens the actual SQLite ledger.
use super::{
    bad,
    host::Worker,
    manifest::{read_sealed, sealed_owner},
    transport::{self, Channel, Kind},
    worker::original_parent,
};
use crate::{
    path_rules::MAX_PATH_BYTES,
    pinned::{Kind as PathKind, Pinned, helper_elf::ForegroundElf},
    smb_profile::{
        self,
        first::{Folder, Id, MAX_FOLDERS, Mode, RecycleName},
    },
    sqlite_install::{
        approval,
        catalog::{self, Initial},
        markers::OrdinaryMark,
        originals::{SelectedSystem, named_backing},
        pair::{
            domain::{
                FreshParent, give_live_approval, request_live_approval, require_lent_ex, sealed,
            },
            observed,
            terminal::PublicConfirmation,
        },
        pair::{
            fresh_issuer::{ExplicitSubject, InitialDatabase, require_private_install},
            recycle::Recycle,
        },
        stage::random,
    },
};
use helper_linux_evidence::diagnostic_process::{OfflineSql, account_sql_descriptors};
use rustix::{
    fs::{FlockOperation, flock},
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
    fmt::Write,
    fs::File,
    io,
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::MetadataExt,
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const DOMAIN: &[u8] = b"helper exact init selection v10\0";
const FRESH: &[u8] = b"ZXS-INITIAL-SESSION-1\0";
const FRESH_SIZE: usize = FRESH.len() + 16 + 8 + 32 + 32;
const BUDGET: usize =
    DOMAIN.len() + 32 + 3 * 16 + 3 * 8 + 1 + (MAX_FOLDERS + 1) * (2 + MAX_PATH_BYTES) + FRESH_SIZE;

#[derive(Debug)]
pub(in crate::sqlite_install::pair) struct Choice {
    pub(in crate::sqlite_install::pair) path: PathBuf,
    pub(in crate::sqlite_install::pair) folder: String,
    pub(in crate::sqlite_install::pair) mode: Mode,
}
#[derive(Debug)]
pub(in crate::sqlite_install::pair) struct FirstShares {
    pub(in crate::sqlite_install::pair) host: String,
    pub(in crate::sqlite_install::pair) choices: Vec<Choice>,
    pub(in crate::sqlite_install::pair) recycle: RecycleName,
}
#[derive(Clone, Copy)]
pub(in crate::sqlite_install::pair) struct FreshScope {
    subject: ExplicitSubject,
    helper: [u8; 32],
    init_sha: [u8; 32],
}
impl FreshScope {
    pub(in crate::sqlite_install::pair) fn new(
        subject: ExplicitSubject,
        helper: [u8; 32],
        init_sha: [u8; 32],
    ) -> Self {
        Self {
            subject,
            helper,
            init_sha,
        }
    }
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
            b.extend((p.len() as u16).to_be_bytes());
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
        b.push(self.paths.len() as u8);
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
// The dedicated synthetic route admits only the earlier enrollment marker
// in selected F/R. Inspect names once on the actual held original directory;
// never follow or read any added object.
fn no_foreign_media(dir: &File) -> io::Result<()> {
    use rustix::fs::{Mode, OFlags, RawDir, openat};
    use std::mem::MaybeUninit;
    let scan = File::from(openat(
        dir,
        c".",
        OFlags::RDONLY
            | OFlags::DIRECTORY
            | OFlags::NOATIME
            | OFlags::NONBLOCK
            | OFlags::NOFOLLOW
            | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let a = dir.metadata()?;
    let b = scan.metadata()?;
    let stamp = |m: &std::fs::Metadata| {
        (
            m.dev(),
            m.ino(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
        )
    };
    if stamp(&a) != stamp(&b) || crate::pinned::Mount::on(dir)? != crate::pinned::Mount::on(&scan)?
    {
        return Err(bad_question());
    }
    let mut scratch = [MaybeUninit::uninit(); 4096];
    let mut data = RawDir::new(&scan, &mut scratch);
    let mut mark = false;
    while let Some(row) = data.next() {
        let row = row?;
        let name = row.file_name().to_bytes();
        if matches!(name, b"." | b"..") {
            continue;
        }
        if name != crate::sqlite_install::MARKER || std::mem::replace(&mut mark, true) {
            return Err(bad(
                "the original fresh data or recycle directory contains unexplained names",
            ));
        }
    }
    if !mark || stamp(&dir.metadata()?) != stamp(&a) || stamp(&scan.metadata()?) != stamp(&b) {
        return Err(bad_question());
    }
    Ok(())
}
struct OriginalFolder {
    pin: Pinned,
    dir: File,
    mark: OrdinaryMark,
    body: [u8; 101],
}
impl OriginalFolder {
    fn require(&self, host: &File, uid: u32) -> io::Result<()> {
        let m = self.dir.metadata()?;
        if !m.is_dir() || m.uid() != uid || m.mode() != 0o040_700 || m.nlink() == 0 {
            return Err(bad_question());
        }
        named_backing(
            &self.pin,
            &self.dir,
            &crate::maintenance::HostMounts::current()?,
            host,
            uid,
        )?;
        self.mark.verify(&self.dir, &self.body)?;
        no_foreign_media(&self.dir)?;
        self.mark.verify(&self.dir, &self.body)
    }
}
fn separate(a: &Pinned, b: &Pinned) -> bool {
    a.entity() != b.entity()
        && !a.entity_is_visible_ancestor_of(b)
        && !b.entity_is_visible_ancestor_of(a)
        && !a.path().starts_with(b.path())
        && !b.path().starts_with(a.path())
}
struct Originals {
    system: Pinned,
    ex: File,
    subject: ExplicitSubject,
    paths_preview: String,
    host: File,
    recycle: Recycle,
    chosen_paths: Vec<PathBuf>,
    physical: Vec<OriginalFolder>,
    database: InitialDatabase,
}
impl Originals {
    fn require(&self) -> io::Result<()> {
        let subject = self.subject.tuple();
        require(&self.system, &self.ex, subject)?;
        self.database.named(&self.ex)?;
        for f in &self.physical {
            f.require(&self.host, subject.1)?;
        }
        self.recycle.require(
            &self.host,
            subject.1,
            std::iter::once((&self.system, &self.ex))
                .chain(self.physical.iter().map(|f| (&f.pin, &f.dir))),
        )?;
        no_foreign_media(self.recycle.source().1)
    }
    fn select<T>(
        system: &Path,
        (pool, init): (Id, Id),
        choices: Vec<Choice>,
        fresh: FreshScope,
        build: impl FnOnce(Vec<Folder>) -> io::Result<T>,
    ) -> io::Result<(Self, T)> {
        let host = approval::host_admin()?;
        let tty = PublicConfirmation::select()?;
        let subject = fresh.subject.tuple();
        let system = Pinned::already_mounted(system, PathKind::StorageRoot)?;
        let ex = system.storage_directory()?;
        flock(&ex, FlockOperation::NonBlockingLockExclusive)?;
        let check = || {
            tty.check()?;
            require(&system, &ex, subject)
        };
        check()?;
        let nonce = random::<32>()?;
        let req = Question {
            nonce,
            actor: subject.0,
            pool,
            init,
            location: place(&system)?,
            system: system.path().to_path_buf(),
            paths: choices.iter().map(|c| c.path.clone()).collect(),
            fresh,
        };
        let LayoutEvidence {
            ordinary,
            recycle,
            database,
        } = read_layout(&req, &ex, check)?;
        let f = choices
            .iter()
            .zip(&ordinary)
            .map(|(c, (volume, _))| Folder {
                volume: *volume,
                name: c.folder.clone(),
                mode: c.mode,
            })
            .collect();
        let plan = build(f)?;
        // Setup cannot be issued for an absent, unconfirmed or read-only R.
        // Prove its actual original before asking for the layout approval.
        let mut physical: Vec<OriginalFolder> = Vec::new();
        for (_, c) in &ordinary {
            let pin = Pinned::already_mounted(&c.path, PathKind::StorageRoot)?;
            if !separate(&pin, &system) || physical.iter().any(|p| !separate(&pin, &p.pin)) {
                return Err(bad_question());
            }
            let dir = pin.storage_directory()?;
            let mark = OrdinaryMark::select(&dir, subject.1, &c.marker)?;
            let original = OriginalFolder {
                pin,
                dir,
                mark,
                body: c.marker,
            };
            original.require(&host, subject.1)?;
            physical.push(original);
        }
        let recycle = Recycle::select(
            &recycle,
            &host,
            subject.1,
            std::iter::once((&system, &ex)).chain(physical.iter().map(|f| (&f.pin, &f.dir))),
        )?;
        let mut paths_preview = String::from("本次低权限只读核对的原 SYSTEM：\"");
        smb_profile::escaped(&mut paths_preview, system.path().as_os_str().as_bytes());
        paths_preview.push_str("\"；普通盘核原声明与原卷标。\n");
        paths_preview.push_str("  已核独立回收盘原路径 \"");
        smb_profile::escaped(
            &mut paths_preview,
            recycle.source().0.as_os_str().as_bytes(),
        );
        paths_preview.push_str("\"；只核卷身份与固定标记。\n");
        for (c, (id, _)) in choices.iter().zip(&ordinary) {
            let hex = crate::fs_tools::lower_hex(id);
            paths_preview.push_str("  已选原路径 \"");
            smb_profile::escaped(&mut paths_preview, c.path.as_os_str().as_bytes());
            writeln!(paths_preview, "\" → 原普通卷 {hex}。").expect("writing a String");
        }
        let out = Self {
            system,
            ex,
            subject: fresh.subject,
            paths_preview,
            host,
            recycle,
            physical,
            database,
            chosen_paths: choices.into_iter().map(|c| c.path).collect(),
        };
        out.require()?;
        Ok((out, plan))
    }
}

pub(in crate::sqlite_install::pair) struct ResolvedRust {
    original: Originals,
    plan: crate::smb_profile::rust_first::RustPlan,
}
impl ResolvedRust {
    pub(in crate::sqlite_install::pair) fn plan(
        &self,
    ) -> &crate::smb_profile::rust_first::RustPlan {
        &self.plan
    }
    pub(in crate::sqlite_install::pair) fn preview(&self) -> &str {
        &self.original.paths_preview
    }
    pub(in crate::sqlite_install::pair) fn require(&self) -> io::Result<()> {
        self.original.require()
    }
    pub(in crate::sqlite_install::pair) fn fresh(
        parent: &FreshParent,
        system: &Path,
        origin: crate::smb_profile::rust_first::Origin,
        shares: FirstShares,
        scope: FreshScope,
    ) -> io::Result<Self> {
        let FirstShares {
            host,
            choices,
            recycle,
        } = shares;
        parent.before_original_sql(&origin.instance)?;
        let (original, plan) = Originals::select(
            system,
            (origin.pool, origin.init),
            choices,
            scope,
            move |folders| {
                crate::smb_profile::rust_first::RustPlan::selected(origin, host, folders, recycle)
                    .map_err(bad)
            },
        )?;
        parent.before_original_sql(&origin.instance)?;
        Ok(Self { original, plan })
    }
    pub(in crate::sqlite_install::pair) fn fresh_database(&self) -> io::Result<[u64; 3]> {
        self.original.database.named(&self.original.ex)
    }
    pub(in crate::sqlite_install::pair) fn protected_new_system(
        &self,
    ) -> io::Result<(&Pinned, &File, ExplicitSubject)> {
        self.require()?;
        Ok((
            &self.original.system,
            &self.original.ex,
            self.original.subject,
        ))
    }
    pub(in crate::sqlite_install::pair) fn require_product_parent(
        &self,
        chain: &[crate::pinned::Entity],
    ) -> io::Result<()> {
        let [_, product, parent] = chain else {
            return Err(bad_question());
        };
        let media = || {
            std::iter::once(&self.original.system)
                .chain([self.original.recycle.pinned()])
                .chain(self.original.physical.iter().map(|f| &f.pin))
        };
        for p in media() {
            if chain.contains(&p.entity()) || p.visible_chain().contains(parent) {
                return Err(bad(
                    "protected product parent aliases an original volume path",
                ));
            }
        }
        crate::sqlite_install::product_layout::require_original_media(*product, media())?;
        self.require()
    }
    pub(in crate::sqlite_install::pair) fn require_distinct_image(
        &self,
        image: &crate::sqlite_install::pair::fresh_issuer::image::Image,
    ) -> io::Result<()> {
        self.original.require()?;
        for p in std::iter::once(&self.original.system)
            .chain([self.original.recycle.pinned()])
            .chain(self.original.physical.iter().map(|f| &f.pin))
        {
            if image.path().starts_with(p.path())
                || p.path().starts_with(image.path())
                || image.includes(p.entity())
                || p.visible_chain().contains(&image.entity())
            {
                return Err(bad(
                    "the original executable overlaps enrolled SYSTEM, data or recycle",
                ));
            }
        }
        image.require()
    }
    pub(in crate::sqlite_install::pair) fn require_distinct_site(
        &self,
        site: &Path,
    ) -> io::Result<()> {
        if std::iter::once(self.original.system.path())
            .chain([self.original.recycle.source().0])
            .chain(self.original.chosen_paths.iter().map(PathBuf::as_path))
            .any(|p| p.starts_with(site) || site.starts_with(p))
        {
            return Err(bad(
                "the new Rust instance overlaps original SYSTEM, recycle or selected data",
            ));
        }
        self.original.require()
    }
}
fn require(pin: &Pinned, file: &File, subject: (Id, u32, u32)) -> io::Result<()> {
    approval::host_admin()?;
    let m = pin.metadata()?;
    if !pin.passive_mount_observation()
        || !m.is_dir()
        || m.mode() != 0o040_700
        || m.uid() != subject.1
        || m.nlink() == 0
    {
        return Err(bad(
            "different protected original SYSTEM or maintenance subject",
        ));
    }
    require_lent_ex(file, (pin.entity(), pin.require_same_named_mount()?.id))
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
fn body(i: usize, id: Id) -> [u8; 65] {
    let mut b = [0; 65];
    b[0] = i as u8;
    b[1..17].copy_from_slice(&id);
    b
}
fn query_with<T>(
    raw: &[u8],
    original: &File,
    subject: (Id, u32, u32),
    helper_sha: Option<[u8; 32]>,
    check: impl Fn() -> io::Result<()>,
    receive: impl FnOnce(&Channel, &[u8; 32], Instant) -> io::Result<T>,
) -> io::Result<T> {
    let helper = ForegroundElf::host()?;
    if let Some(expected) = helper_sha {
        helper.require_independent_digest(&expected)?;
    }
    let scope: [u8; 32] = Sha256::digest(raw).into();
    let meta = sealed(raw)?;
    let (parent, child) = transport::pair()?;
    let mut hired = Worker::spawn(helper.descriptor(), &child, &meta, OfflineSql::FirstLayout)?;
    drop(child);
    drop(meta);
    let who = UCred {
        pid: hired.pid,
        uid: Uid::from_raw(subject.1),
        gid: Gid::from_raw(subject.2),
    };
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        check()?;
        hired.live()?;
        if Instant::now() >= until {
            return Err(bad("the original read-only SYSTEM worker did not respond"));
        }
        if give_live_approval(&parent, who, &scope, || {
            check()?;
            helper.require()
        })? {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let channel = Channel::from_owned_child_session(parent, who, scope)?;
    channel.send_original(Kind::Lease, &scope, &[0; 65], original)?;
    let v = receive(&channel, &scope, until)?;
    hired.finish(&channel, until)?;
    check()?;
    Ok(v)
}
struct LayoutEvidence {
    ordinary: Vec<(Id, Initial)>,
    recycle: Initial,
    database: InitialDatabase,
}
fn read_layout(
    q: &Question,
    ex: &File,
    check: impl Fn() -> io::Result<()>,
) -> io::Result<LayoutEvidence> {
    let subject = q.fresh.subject.tuple();
    query_with(
        &q.encode()?,
        ex,
        subject,
        Some(q.fresh.helper),
        &check,
        |channel, scope, until| {
            let (b, [fd]) = channel.wait_checked::<1>(Kind::FreshDatabase, scope, until, &check)?;
            let database = InitialDatabase::received(fd, subject.1)?;
            let place = database.named(ex)?;
            if b != database_payload(place)
                || place[0] != q.location[0]
                || place[2] != q.location[2]
            {
                return Err(bad_question());
            }
            let mut ordinary = Vec::with_capacity(q.paths.len());
            for (i, path) in q.paths.iter().enumerate() {
                let (v, []) =
                    channel.wait_checked::<0>(Kind::ChosenFolder, scope, until, &check)?;
                let id: Id = v[1..17].try_into().expect("fixed reply");
                if v != body(i, id)
                    || id == [0; 16]
                    || ordinary.iter().any(|(known, _)| *known == id)
                {
                    return Err(bad_question());
                }
                let fact = receive_scoped_media(channel, scope, until, &check, i, Some(id), q)?;
                if fact.path.as_os_str() != path.as_os_str() {
                    return Err(bad_question());
                }
                ordinary.push((id, fact));
            }
            let recycle =
                receive_scoped_media(channel, scope, until, &check, q.paths.len(), None, q)?;
            if ordinary
                .iter()
                .any(|(id, _)| id.as_slice() == &recycle.marker[37..53])
            {
                return Err(bad_question());
            }
            Ok(LayoutEvidence {
                ordinary,
                recycle,
                database,
            })
        },
    )
}
fn volume_reply(i: usize, raw: &[u8]) -> [u8; 65] {
    let mut b = [0; 65];
    b[0] = i as u8;
    b[1..33].copy_from_slice(&Sha256::digest(raw));
    b
}
fn media_bytes(record: &crate::sqlite_install::catalog::Initial) -> Vec<u8> {
    let mut out = record.marker.to_vec();
    out.extend(record.path.as_os_str().as_bytes());
    out
}
// A missing `expected` can represent only the uniquely issued ordinal-one R.
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
fn receive_scoped_media(
    channel: &Channel,
    scope: &[u8; 32],
    until: Instant,
    check: impl Fn() -> io::Result<()>,
    i: usize,
    volume: Option<Id>,
    q: &Question,
) -> io::Result<Initial> {
    let (reply, [fd]) = channel.wait_checked::<1>(Kind::VolumeClaim, scope, until, check)?;
    let (_, uid, gid) = q.fresh.subject.tuple();
    let value = sealed_owner(&fd, 101 + MAX_PATH_BYTES, (uid, gid))?;
    if reply != volume_reply(i, &value) {
        return Err(bad_question());
    }
    scoped_material(&value, volume, q.pool, q.init)
}
fn send_media(channel: &Channel, scope: &[u8; 32], i: usize, claim: &Initial) -> io::Result<()> {
    let bytes = media_bytes(claim);
    let report = sealed(&bytes)?;
    channel.send_original(Kind::VolumeClaim, scope, &volume_reply(i, &bytes), &report)
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
            &body(i, volume.marker[37..53].try_into().expect("VolumeId")),
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
