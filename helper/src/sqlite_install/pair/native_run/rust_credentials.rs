//! Two confined workers for one activation. SQL holds metadata-only vault
//! witnesses and the isolated Web verifier; only the original jointly
//! deriving writer can populate the approved new 240-byte vault stage.
use super::{
    bad,
    host::{Worker, prerequisite},
    joint_material::{Reader as JointReader, Writer as JointWriter},
    objects::{Objects, object_body, read_object_body},
    transport::{self, Channel, Kind, Payload},
    worker::original_parent,
};
use crate::{
    maintenance::HostMounts,
    pinned::{Kind as PathKind, Pinned, helper_elf::ForegroundElf},
    sqlite_install::{
        approval,
        originals::SelectedSystem,
        pair::{
            domain::{
                Allocated, CredentialSite, CurrentVault, PlacedVault, RustSite, VaultStamp,
                checked_vault_stamp, deployed_worker_payload, give_live_approval,
                request_live_approval, sealed, verify_fresh_worker,
            },
            fresh_issuer::{FreshConsent, InitialDatabase, SitePayload, require_private_install},
            native_sql::{
                Original,
                rust_vault::{
                    ActiveRustMeta, RustAccountCut, RustChange, observed_binding, require_observed,
                },
            },
            rust_enroll::{
                crypto::{CredentialHelper, Cryptography},
                manifest::{Authority, Intent, Manifest, seal_bytes},
                receipt::Observed,
                secret_order::{SecretOrder, scope as secret_scope},
            },
            terminal::{HeldQuiet, PublicConfirmation, SelectedInput},
        },
    },
};
use helper_linux_evidence::diagnostic_process::{OfflineSql, account_sql_descriptors};
use rustix::{
    fs::{OFlags, fcntl_getfl},
    net::UCred,
    process::{
        DumpableBehavior, Gid, Pid, Signal, Uid, dumpable_behavior, getppid,
        parent_process_death_signal, set_dumpable_behavior, set_parent_process_death_signal,
    },
};
use std::{
    fs::File,
    io,
    os::unix::fs::MetadataExt,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

const LIMIT: Duration = Duration::from_mins(12);
fn exact_parent(fd: &File, owner: Pid) -> io::Result<ForegroundElf> {
    original_parent(fd, owner)?;
    let image = ForegroundElf::host()?;
    image.parent_is_same_image(owner)?;
    Ok(image)
}
type Subject = ([u8; 16], u32, u32);
fn subject(m: &Manifest, image: &CredentialHelper) -> io::Result<(Subject, Option<SitePayload>)> {
    let bound = match m.authority {
        Authority::Fresh(scope) => {
            verify_fresh_worker(&m.system, &m.plan, &scope)?;
            (scope.subject.tuple(), None)
        }
        Authority::Deployed(scope) => {
            let actual = deployed_worker_payload(&m.plan, scope.site)?;
            if actual.subject != scope.subject || actual.system != m.system {
                return Err(bad(
                    "different original protected deployed subject or SYSTEM",
                ));
            }
            (scope.subject.tuple(), Some(actual))
        }
    };
    let ((actor, _, _), _) = &bound;
    if m.order.actor != *actor {
        return Err(bad("wrong original Rust maintenance principal"));
    }
    image.require()?;
    Ok(bound)
}
fn isolated(
    fd: File,
    parent: Pid,
    uid: u32,
    gid: u32,
    digest: [u8; 32],
) -> io::Result<(Channel, Instant)> {
    approval::drop_to_storage(uid, gid)?;
    set_parent_process_death_signal(Some(Signal::TERM))?;
    original_parent(&fd, parent)?;
    set_dumpable_behavior(DumpableBehavior::NotDumpable)?;
    if dumpable_behavior()? != DumpableBehavior::NotDumpable {
        return Err(bad("the selected low credential worker is not isolated"));
    }
    request_live_approval(&fd, parent, &digest, || original_parent(&fd, parent))?;
    let until = Instant::now()
        .checked_add(LIMIT)
        .ok_or_else(|| bad("offline Rust worker deadline overflow"))?;
    let channel = Channel::from_owned_child_session(
        fd,
        UCred {
            pid: parent,
            uid: Uid::ROOT,
            gid: Gid::ROOT,
        },
        digest,
    )?;
    Ok((channel, until))
}
fn live_parent(owner: Pid, until: Instant) -> io::Result<()> {
    if getppid() != Some(owner)
        || parent_process_death_signal()? != Some(Signal::TERM)
        || Instant::now() >= until
    {
        return Err(bad("lost the original live Rust credential owner"));
    }
    Ok(())
}
fn body(tag: u8, id: &[u8; 16]) -> Payload {
    let mut b = [0; 65];
    b[0] = tag;
    b[1..17].copy_from_slice(id);
    b
}
fn observed(tag: u8, c: &RustChange) -> io::Result<File> {
    let result = Observed {
        state: tag,
        action: c.digest(),
        target: c.target(),
        previous: c.previous().cloned(),
    };
    sealed(&result.encode()?)
}

struct VaultWitness {
    original: File,
    mount: u64,
    stamp: VaultStamp,
}
impl VaultWitness {
    fn receive(
        channel: &Channel,
        frame: Kind,
        tag: u8,
        expected: &RustChange,
        until: Instant,
        live: impl Fn() -> io::Result<()>,
    ) -> io::Result<Self> {
        let (p, [original]) = channel.wait_checked::<1>(frame, &expected.digest(), until, live)?;
        if p[0] != tag || p[1..17] != expected.target().record {
            return Err(bad("different original shared-account vault witness"));
        }
        let u = |i: usize| u64::from_be_bytes(p[i..i + 8].try_into().expect("fixed vault shape"));
        let i = |i: usize| i64::from_be_bytes(p[i..i + 8].try_into().expect("fixed vault shape"));
        let n = |i: usize| u32::from_be_bytes(p[i..i + 4].try_into().expect("fixed vault shape"));
        let selected = Self {
            original,
            mount: u(17),
            stamp: VaultStamp {
                dev: u(25),
                ino: u(33),
                ctime_sec: i(41),
                ctime_ns: n(49),
                mtime_sec: i(53),
                mtime_ns: n(61),
            },
        };
        selected.require()?;
        Ok(selected)
    }
    fn require(&self) -> io::Result<()> {
        if !fcntl_getfl(&self.original)?.contains(OFlags::PATH)
            || self.mount == 0
            || checked_vault_stamp(&self.original, self.mount)? != self.stamp
        {
            return Err(bad(
                "the selected Rust vault witness is not the original root-owned O_PATH",
            ));
        }
        Ok(())
    }
    /// The old stage pin stays held, so a rename can change ctime but
    /// cannot counterfeit its entity through inode reuse. Require both
    /// descriptors to identify the actual latest installed stamp.
    fn now_placed(&self, placed: &Self) -> io::Result<()> {
        let a = self.stamp;
        let b = placed.stamp;
        if self.mount != placed.mount
            || (a.dev, a.ino, a.mtime_sec, a.mtime_ns) != (b.dev, b.ino, b.mtime_sec, b.mtime_ns)
            || checked_vault_stamp(&self.original, self.mount)? != b
        {
            return Err(bad(
                "the installed vault is not the same stage admitted before PENDING",
            ));
        }
        placed.require()
    }
}
pub(crate) fn account_worker() -> io::Result<()> {
    require_private_install()?;
    let [fd, meta] = account_sql_descriptors()?;
    approval::only_inherited_descriptors(&[3, 4])?;
    let parent = getppid().ok_or_else(|| bad("the offline Rust account owner is absent"))?;
    let original = exact_parent(&fd, parent)?;
    let host = approval::host_admin()?;
    let m = Manifest::read(&meta)?;
    drop(meta);
    let helper = CredentialHelper::from_worker(original, m.authority.helper(), parent)?;
    let ((_, uid, gid), deployed) = subject(&m, &helper)?;
    drop(helper); // the SQL child never needs the root executable reader
    let scope = m.scope()?;
    let (channel, until) = isolated(fd, parent, uid, gid, scope)?;
    let (zero, [lease]) =
        channel.wait_checked::<1>(Kind::Lease, &scope, until, || live_parent(parent, until))?;
    if zero != [0; 65] {
        return Err(bad("wrong original SYSTEM lease"));
    }
    let local = if deployed.is_some() {
        Some(Arc::new(HostMounts::discovery_view(false)?))
    } else {
        None
    };
    let pinned = if let Some(view) = &local {
        Pinned::require_local_root(view)?;
        Pinned::local_storage(&m.system, view)?
    } else {
        Pinned::already_mounted(&m.system, PathKind::StorageRoot)?
    };
    let e = pinned.entity();
    if [e.dev, e.ino, pinned.require_same_named_mount()?.id] != m.entity {
        return Err(bad("different original SYSTEM"));
    }
    let locked = SelectedSystem::from_origin(pinned, host, uid, gid)?.hold_lent_ex(lease)?;
    let cut = match (m.authority, deployed.as_ref()) {
        (Authority::Fresh(f), None) => {
            RustAccountCut::open_new_first(&locked, &m.plan, m.order, f.init_sha, f.database)?
        }
        (Authority::Deployed(_), Some(actual)) => {
            RustAccountCut::open_deployed_reset(&locked, &m.plan, m.order, actual)?
        }
        _ => return Err(bad("no exact original Rust scope")),
    };
    let alive = || {
        live_parent(parent, until)?;
        locked.require()
    };
    let result = match (m.intent, cut.observe()?) {
        (Intent::Fresh, Original::Absent) => Some((1, cut.propose_new()?)),
        (Intent::Lookup, Original::Absent) => None,
        (Intent::Lookup, Original::Pending(v)) => Some((2, v)),
        (Intent::Lookup, Original::Committed(v)) => Some((3, v)),
        _ => {
            return Err(bad(
                "new requires a truly absent original operation; never resumes it",
            ));
        }
    };
    // Exact protected metadata descriptors; never lend a readable SYSTEM
    // file to the high process. Both successful new paths keep these names
    // and entities through final closure (the fresh path through selector).
    let (db, stamp, marker) = cut.objects()?;
    let observed_marker = marker.require(locked.source().dir())?;
    alive()?;
    channel.send_original(Kind::DeployedDatabase, &scope, &object_body(stamp), &db)?;
    channel.send_original(
        Kind::DeployedMarker,
        &scope,
        &object_body(observed_marker),
        marker.pin(),
    )?;
    let Some((tag, change)) = result else {
        alive()?;
        channel.send_original(Kind::RustInspect, &scope, &[0; 65], &sealed(&[0])?)?;
        return Ok(());
    };
    let target = change.target();
    let digest = change.digest();
    alive()?;
    channel.send_original(
        Kind::RustInspect,
        &scope,
        &body(tag, &target.record),
        &observed(tag, &change)?,
    )?;
    if m.intent != Intent::Fresh {
        return Ok(());
    }
    let binding = observed_binding(m.nonce, digest, &target, change.previous())?;
    let (peer, [link]) = channel.wait_checked::<1>(Kind::JointSqlPeer, &digest, until, &alive)?;
    let reader = JointReader::connect(
        link,
        child_peer(peer, &target.record, uid, gid)?,
        binding,
        alive,
    )?;
    let stage = VaultWitness::receive(&channel, Kind::JointVaultDurable, 1, &change, until, alive)?;
    // The original parent sends this only after reaping the actual writer,
    // seeing its clean EOF and independently rechecking the durable stage.
    // The queued peer is identified by SCM; it is not expected still live.
    let web = reader.receive(until, alive)?;
    let staged_live = || {
        alive()?;
        stage.require()
    };
    let pending = cut.reserve_after_staging(change, m.nonce, web, until, staged_live)?;
    pending.require()?;
    staged_live()?;
    channel.send(Kind::Reserved, &digest, &body(2, &target.record))?;
    let placed = VaultWitness::receive(
        &channel,
        Kind::RustVaultPlaced,
        3,
        pending.change(),
        until,
        alive,
    )?;
    let placed_live = || {
        alive()?;
        stage.now_placed(&placed)
    };
    let active = pending.finish(placed_live)?;
    if active != target {
        return Err(bad("wrong independently read back ACTIVE result"));
    }
    let (again, stamp, current_mark) = cut.objects()?;
    if InitialDatabase::scope(&again, uid)? != InitialDatabase::scope(&db, uid)?
        || marker.require(locked.source().dir())? != observed_marker
        || current_mark.require(locked.source().dir())? != observed_marker
    {
        return Err(bad("original shared-account SYSTEM evidence changed"));
    }
    if let Some(local) = &local {
        local.require_unchanged()?;
    }
    placed_live()?;
    channel.send(Kind::SharedAccountDurable, &digest, &object_body(stamp))?;
    drop(cut);
    placed_live()?;
    channel.send(Kind::Complete, &digest, &body(3, &target.record))
}

pub(crate) fn vault_worker() -> io::Result<()> {
    require_private_install()?;
    let [fd, meta] = account_sql_descriptors()?;
    approval::only_inherited_descriptors(&[3, 4])?;
    let parent = getppid().ok_or_else(|| bad("the original Rust secret owner is absent"))?;
    let running = exact_parent(&fd, parent)?;
    let (order, original) = SecretOrder::read(&meta)?;
    drop(meta);
    let helper = CredentialHelper::from_worker(running, order.manifest.authority.helper(), parent)?;
    let ((_, uid, gid), _) = subject(&order.manifest, &helper)?;
    let pins = order
        .manifest
        .authority
        .crypto()
        .ok_or_else(|| bad("no exact original credential provider scope"))?;
    let policy = Cryptography::from_worker(pins, helper)?;
    let lib = policy.before_dropping()?;
    let tty = SelectedInput::before_drop()?;
    let (channel, until) = isolated(fd, parent, uid, gid, original)?;
    let alive = || live_parent(parent, until);
    tty.lock_secret_memory()?;
    alive()?;
    let crypto = policy.for_low_writer(&lib)?;
    let target = order.selected;
    let binding = observed_binding(
        order.manifest.nonce,
        target.action,
        &target.target,
        target.previous.as_ref(),
    )?;
    let complete = tty.reserve_joint(uid, gid, crypto, binding)?;
    let (peer, [link]) =
        channel.wait_checked::<1>(Kind::JointWriterPeer, &target.action, until, &alive)?;
    let writer = JointWriter::connect(
        link,
        child_peer(peer, &target.target.record, uid, gid)?,
        binding,
        until,
        alive,
    )?;
    channel.send(
        Kind::JointWriterReady,
        &target.action,
        &body(1, &target.target.record),
    )?;
    let (ready, [mut stage]) =
        channel.wait_checked::<1>(Kind::RustStage, &target.action, until, &alive)?;
    if ready != body(1, &target.target.record) {
        return Err(bad(
            "wrong original writable stage for the new shared password",
        ));
    }
    alive()?;
    let web = complete.seal_in_held_tty(&mut stage)?;
    drop(stage);
    alive()?;
    writer.deliver(web, alive)?;
    channel.send(
        Kind::RustVaultSealed,
        &target.action,
        &body(1, &target.target.record),
    )
}

pub(in crate::sqlite_install::pair) struct RustEnrollmentApproval<'a, 's> {
    site: CredentialSite<'s>,
    terminal: &'a PublicConfirmation,
    original: &'a Observed,
    until: Instant,
    fresh: Option<&'a dyn Fn() -> io::Result<()>>,
}
impl<'a, 's> RustEnrollmentApproval<'a, 's> {
    pub(in crate::sqlite_install::pair) fn site(&self) -> CredentialSite<'s> {
        self.site
    }
    pub(in crate::sqlite_install::pair) fn original(&self) -> &'a Observed {
        self.original
    }
    pub(in crate::sqlite_install::pair) fn require(&self) -> io::Result<()> {
        if Instant::now() >= self.until {
            return Err(bad("the original Rust maintenance approval expired"));
        }
        if let Some(check) = self.fresh {
            check()?;
        }
        self.terminal.check()?;
        self.site.require()
    }
    fn while_quiet(&self, quiet: &HeldQuiet<'_>) -> io::Result<()> {
        if Instant::now() >= self.until {
            return Err(bad("the original Rust maintenance approval expired"));
        }
        if let Some(check) = self.fresh {
            check()?;
        }
        quiet.require()?;
        self.site.require()
    }
}
/// Only the confirmed original SQL worker may make the subsequent stage
/// publishable. An observed row, filename or byte prefix alone cannot do it.
pub(in crate::sqlite_install::pair) struct DurableRustPending<'a, 'g, 's> {
    approved: &'a RustEnrollmentApproval<'g, 's>,
    sql: &'a Worker,
}
impl<'s> DurableRustPending<'_, '_, 's> {
    pub(in crate::sqlite_install::pair) fn require(
        &self,
        site: CredentialSite<'s>,
        meta: &ActiveRustMeta,
    ) -> io::Result<()> {
        if !self.approved.site.same(site) || &self.approved.original.target != meta {
            return Err(bad("not the original durable Rust operation"));
        }
        self.approved.require()?;
        self.sql.live()
    }
}

fn launch(
    helper: &ForegroundElf,
    site: CredentialSite<'_>,
    tty: &PublicConfirmation,
    scope: [u8; 32],
    meta: File,
    mode: OfflineSql,
) -> io::Result<(Worker, Channel)> {
    let (ours, child) = transport::pair()?;
    let worker = Worker::spawn(helper.descriptor(), &child, &meta, mode)?;
    drop(child);
    drop(meta);
    let (_, uid, gid) = site.subject();
    let actor = UCred {
        pid: worker.pid,
        uid: Uid::from_raw(uid),
        gid: Gid::from_raw(gid),
    };
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        worker.live()?;
        if Instant::now() >= until {
            return Err(bad(
                "the fixed original Rust credential worker did not become ready",
            ));
        }
        if give_live_approval(&ours, actor, &scope, || {
            site.require()?;
            tty.check()?;
            helper.require()
        })? {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    Ok((
        worker,
        Channel::from_owned_child_session(ours, actor, scope)?,
    ))
}
fn hex(b: &[u8]) -> String {
    crate::fs_tools::lower_hex(b)
}
fn preview(m: &Manifest, v: &Observed) -> io::Result<String> {
    let site = m.plan.origin;
    let target = &v.target;
    let old = v
        .previous
        .as_ref()
        .ok_or_else(|| bad("no exact predecessor for the reset approval"))?;
    let crypto = m
        .authority
        .crypto()
        .ok_or_else(|| bad("no exact independent provider approval"))?;
    Ok(format!(
        "仅离线变更同一 Web 与 SMB 共用账户；不更改用户名、卷或站点，不启动服务，不接管或删除未知暂存。\n\
        原 SYSTEM {:?}，主机 {}，池 {} / INIT {} / SETUP {}，站点 {} / Bind {}，配置摘要 {}。\n\
        原账 {} / 唯一用户名 {}，原凭据 {} / 代次 {} / 原正本 {}。\n\
        新操作 {} / 批准 {} / 完整请求 SHA-256 {} / 新代次 {} / 新正本 {}。\n\
        已独立核准的 Helper {}、libgcrypt {}、libgpg-error {}。随后只输入并复输同一个口令。\n",
        m.system,
        m.plan.host(),
        hex(&site.pool),
        hex(&site.init),
        hex(&site.setup),
        hex(&site.instance),
        hex(&site.bind),
        hex(&m.plan.sha()),
        hex(&target.account),
        std::str::from_utf8(&target.login).expect("checked canonical login"),
        hex(&old.op),
        old.generation,
        hex(&old.record),
        hex(&m.order.op),
        hex(&m.order.approval),
        hex(&v.action),
        target.generation,
        hex(&target.record),
        hex(&crypto.helper),
        hex(&crypto.gcrypt),
        hex(&crypto.gpgerror)
    ))
}
fn expected(m: &Manifest, v: &Observed) -> io::Result<()> {
    if matches!(m.authority,Authority::Fresh(scope) if scope.init_sha!=v.target.init_sha) {
        return Err(bad("different exact INIT consent"));
    }
    require_observed(&m.plan, &m.order, v.action, &v.target, v.previous.as_ref())
}
fn peer_body(pid: Pid, record: &[u8; 16]) -> Payload {
    let mut b = body(1, record);
    b[17..21].copy_from_slice(&pid.as_raw_pid().to_be_bytes());
    b
}
fn child_peer(b: Payload, record: &[u8; 16], uid: u32, gid: u32) -> io::Result<UCred> {
    let pid = Pid::from_raw(i32::from_be_bytes(b[17..21].try_into().expect("fixed PID")))
        .ok_or_else(|| bad("no exact lowered material peer"))?;
    if pid.as_raw_pid() < 2 || b != peer_body(pid, record) {
        return Err(bad("wrong exact lowered material peer"));
    }
    Ok(UCred {
        pid,
        uid: Uid::from_raw(uid),
        gid: Gid::from_raw(gid),
    })
}

pub(super) type TransactionResult<'s> = (Option<Observed>, Option<Completed<'s>>);
/// Own both genuinely retained, already qualified evidence families until
/// the high caller has published the fresh selector or reported the reset.
pub(super) struct Completed<'s> {
    held: Objects,
    final_db: [u8; 64],
    vault: PlacedVault<'s>,
}
impl Completed<'_> {
    pub(super) fn require(&self, original: &File) -> io::Result<()> {
        self.vault.require()?;
        self.held.require(original, &self.final_db)
    }
}
fn transact_inner<'s>(
    site: CredentialSite<'s>,
    m: Manifest,
    tty: &PublicConfirmation,
    fresh_consent: Option<(&FreshConsent<'_>, &dyn Fn() -> io::Result<()>)>,
) -> io::Result<TransactionResult<'s>> {
    require_private_install()?;
    let fresh = fresh_consent.map(|(_, live)| live);
    if let Some((consent, check)) = fresh_consent {
        consent.require_manifest(&m)?;
        check()?;
    }
    site.require()?;
    tty.check()?;
    prerequisite()?;
    let (pin, lease) = site.original_system()?;
    let original = pin.entity();
    if &m.plan != site.plan()
        || m.system.as_path() != pin.path()
        || m.order.actor != site.subject().0
        || m.entity
            != [
                original.dev,
                original.ino,
                pin.require_same_named_mount()?.id,
            ]
    {
        return Err(bad("not this exact original offline Rust site"));
    }
    let deployed = match (site, m.authority) {
        (CredentialSite::Fresh(f), Authority::Fresh(new))
            if f.fresh_payload().is_some_and(|p| p.subject == new.subject) && fresh.is_some() =>
        {
            if f.fresh_payload()
                .ok_or_else(|| bad("missing original fresh authority"))?
                .sha()?
                != new.site
            {
                return Err(bad("different original fresh installation"));
            }
            false
        }
        (CredentialSite::Deployed(d), Authority::Deployed(current))
            if d.site_sha() == current.site
                && d.payload().subject == current.subject
                && fresh.is_none() =>
        {
            true
        }
        _ => {
            return Err(bad(
                "not the exact original deployed or fresh ZXS authority",
            ));
        }
    };
    let helper = ForegroundElf::host()?;
    helper.require_independent_digest(&m.authority.helper())?;
    let scope = m.scope()?;
    let check = || {
        site.require()?;
        helper.require()?;
        tty.check()
    };
    let (mut sql, ch) = launch(
        &helper,
        site,
        tty,
        scope,
        sealed(&m.encode()?)?,
        OfflineSql::RustAccount,
    )?;
    ch.send_original(Kind::Lease, &scope, &[0; 65], lease)?;
    let initial = Instant::now() + Duration::from_secs(20);
    let (_, uid, gid) = site.subject();
    let objects = Objects::received(&ch, &scope, initial, lease, uid, &check)?;
    if let Authority::Fresh(new) = m.authority {
        objects.require_database(lease, new.database)?;
    }
    let (packet, [observed]) = ch.wait_checked::<1>(Kind::RustInspect, &scope, initial, &check)?;
    let evidence = observed.metadata()?;
    if (evidence.uid(), evidence.gid()) != (uid, gid) {
        return Err(bad("observation not from the exact lowered subject"));
    }
    let original_cut = || -> io::Result<()> {
        check()?;
        if deployed {
            pin.local_scope()
                .ok_or_else(|| bad("missing passive original SYSTEM selection"))?
                .require_unchanged()?;
        }
        objects.require(lease, &objects.initial)
    };
    if packet == [0; 65] {
        if !deployed || m.intent != Intent::Lookup || seal_bytes(&observed, 1)? != [0] {
            return Err(bad("uncertain original Rust operation absence"));
        }
        drop(observed);
        sql.finish(&ch, initial)?;
        original_cut()?;
        return Ok((None, None));
    }
    let mut chosen = Observed::read(&observed, site.plan())?;
    drop(observed);
    if packet != body(chosen.state, &chosen.target.record) {
        return Err(bad("different original sealed Rust receipt"));
    }
    expected(&m, &chosen)?;
    if m.intent == Intent::Lookup {
        if !deployed || !matches!(chosen.state, 2 | 3) || chosen.previous.is_none() {
            return Err(bad("not a historical reset observation"));
        }
        sql.finish(&ch, initial)?;
        original_cut()?;
        return Ok((Some(chosen), None));
    }
    if chosen.state != 1 || deployed != chosen.previous.is_some() {
        return Err(bad("new requires a truly new original Rust operation"));
    }
    let current = chosen
        .previous
        .as_ref()
        .map(|p| CurrentVault::select(site, p))
        .transpose()?;
    {
        let untouched = || -> io::Result<()> {
            original_cut()?;
            if let Some(v) = &current {
                v.require()?;
            }
            sql.live()
        };
        untouched()?;
        if let Some((consent, _)) = fresh_consent {
            consent.require_observed(&m, &chosen)?;
        } else if deployed {
            tty.confirm_shared_account_reset(preview(&m, &chosen)?)?;
        } else {
            return Err(bad("no original fresh combined consent"));
        }
        untouched()?;
    }
    let until = Instant::now() + Duration::from_secs(if deployed { 9 * 60 } else { 10 * 60 });
    let grant = RustEnrollmentApproval {
        site,
        terminal: tty,
        original: &chosen,
        until,
        fresh,
    };
    let digest = chosen.action;
    let record = chosen.target.record;
    let act = (|| -> io::Result<([u8; 64], PlacedVault<'_>)> {
        grant.require()?;
        sql.live()?;
        let order = SecretOrder::encode(&m, &chosen)?;
        let (mut writer, secret) = launch(
            &helper,
            site,
            tty,
            secret_scope(&order),
            sealed(&order)?,
            OfflineSql::RustVaultWriter,
        )?;
        let (to_sql, to_writer) = transport::pair()?;
        grant.require()?;
        sql.live()?;
        writer.live()?;
        ch.send_original(
            Kind::JointSqlPeer,
            &digest,
            &peer_body(writer.pid, &record),
            &to_sql,
        )?;
        secret.send_original(
            Kind::JointWriterPeer,
            &digest,
            &peer_body(sql.pid, &record),
            &to_writer,
        )?;
        // The high process has no endpoint left when it grants hidden input.
        drop(to_sql);
        drop(to_writer);
        let (ready, []) =
            secret.wait_checked::<0>(Kind::JointWriterReady, &digest, until, || {
                grant.require()?;
                writer.live()?;
                sql.live()
            })?;
        if ready != body(1, &record) {
            return Err(bad("not the actual protected shared credential input"));
        }
        // Both low peers and the locked Web KDF are ready before any secret
        // prompt or even this named, exact new non-replacing vault stage.
        original_cut()?;
        let (allocated, writable) = Allocated::reserve(&grant, current, || {
            sql.live()?;
            writer.live()
        })?;
        // No process-wide cancellation handler or other PromptSignals
        // scope is active here. The parent owns terminal restoration.
        let input = SelectedInput::before_drop()?;
        let stdin = io::stdin();
        let quiet = input.hold_for_birth(&stdin)?;
        let attempt = (|| -> io::Result<_> {
            grant.while_quiet(&quiet)?;
            secret.send_original(Kind::RustStage, &digest, &body(1, &record), &writable)?;
            drop(writable);
            let (written, []) =
                secret.wait_checked::<0>(Kind::RustVaultSealed, &digest, until, || {
                    grant.while_quiet(&quiet)?;
                    sql.live()
                })?;
            if written != body(1, &record) {
                return Err(bad(
                    "the original secret writer did not seal this exact stage",
                ));
            }
            writer.finish(&secret, until)?;
            let frozen = allocated.reopen_after_reap()?;
            grant.while_quiet(&quiet)?;
            sql.live()?;
            // From here the short original SQL reserve can make the old
            // credential unavailable. The retained DB pin gets a new time
            // stamp at final durable readback, not a newly selected inode.
            objects.require(lease, &objects.initial)?;
            let (wire, original) = frozen.wire()?;
            grant.while_quiet(&quiet)?;
            sql.live()?;
            ch.send_original(Kind::JointVaultDurable, &digest, &wire, original)?;
            let (p, []) =
                ch.wait_checked::<0>(Kind::Reserved, &digest, until, || grant.while_quiet(&quiet))?;
            if p != body(2, &record) {
                return Err(bad("the original SQL worker did not re-open this PENDING"));
            }
            Ok(frozen)
        })();
        if attempt.is_err() {
            writer.cancel();
            sql.cancel();
        }
        quiet.finish()?;
        let frozen = attempt?;
        grant.require()?;
        sql.live()?;
        let pending = DurableRustPending {
            approved: &grant,
            sql: &sql,
        };
        let placed = frozen.replace_in_pending(&pending)?;
        let (wire, original) = placed.wire()?;
        ch.send_original(Kind::RustVaultPlaced, &digest, &wire, original)?;
        let (d, []) = ch.wait_checked::<0>(Kind::SharedAccountDurable, &digest, until, || {
            grant.require()?;
            placed.require()
        })?;
        let final_db = read_object_body(d)?;
        let (done, []) = ch.wait_checked::<0>(Kind::Complete, &digest, until, || {
            grant.require()?;
            placed.require()
        })?;
        if done != body(3, &record) {
            return Err(bad("wrong original Rust credential completion"));
        }
        sql.finish(&ch, until)?;
        placed.require()?;
        grant.require()?;
        helper.require()?;
        if deployed {
            pin.local_scope()
                .ok_or_else(|| bad("missing original reset SYSTEM view"))?
                .require_unchanged()?;
        }
        objects.require(lease, &final_db)?;
        Ok((final_db, placed))
    })();
    let (final_db, vault) = if deployed {
        act.map_err(|e| reset_uncertain(&m, e))?
    } else {
        act?
    };
    chosen.state = 3;
    Ok((
        Some(chosen),
        Some(Completed {
            held: objects,
            final_db,
            vault,
        }),
    ))
}

pub(super) fn reset_uncertain(m: &Manifest, e: io::Error) -> io::Error {
    io::Error::new(
        e.kind(),
        format!(
            "共用账户重置结果未确认；仅查询同一原操作 {} / 批准 {}，勿再发起、删除或清理。{e}",
            hex(&m.order.op),
            hex(&m.order.approval)
        ),
    )
}
pub(super) fn transact_deployed<'s>(
    site: &'s super::super::domain::DeployedSite,
    m: Manifest,
    tty: &PublicConfirmation,
) -> io::Result<TransactionResult<'s>> {
    transact_inner(CredentialSite::Deployed(site), m, tty, None)
}

/// A successful return from the actual original low worker (clean exit and
/// EOF), the rather specific durable SQL readback and same original vault.
/// Raw Observed data alone is not a deployment publication capability.
#[must_use]
pub(in crate::sqlite_install::pair) struct FreshActive<'s> {
    site: &'s RustSite,
    original: Observed,
    complete: Completed<'s>,
}
impl FreshActive<'_> {
    pub(in crate::sqlite_install::pair) fn at(
        &self,
        site: &RustSite,
    ) -> io::Result<&ActiveRustMeta> {
        if !std::ptr::eq(self.site, site) {
            return Err(bad("the fresh ACTIVE proof belongs to a different site"));
        }
        let (_, original) = site.original_system()?;
        self.complete.require(original)?;
        Ok(&self.original.target)
    }
}
pub(in crate::sqlite_install::pair) fn transact_fresh<'s>(
    site: &'s RustSite,
    m: Manifest,
    consent: FreshConsent<'_>,
    live: &dyn Fn() -> io::Result<()>,
) -> io::Result<FreshActive<'s>> {
    if site.fresh_payload().is_none() || !matches!(m.authority, Authority::Fresh(_)) {
        return Err(bad("fresh original ZXS request required"));
    }
    let (Some(original), Some(complete)) = transact_inner(
        CredentialSite::Fresh(site),
        m,
        consent.terminal(),
        Some((&consent, live)),
    )?
    else {
        return Err(bad("no original fresh ZXS ACTIVE proof"));
    };
    if original.state != 3
        || original.target.generation != 1
        || original.target.op != original.target.setup
        || original.previous.is_some()
    {
        return Err(bad("no fresh generation-one completion"));
    }
    let good = FreshActive {
        site,
        original,
        complete,
    };
    good.at(site)?;
    Ok(good)
}
