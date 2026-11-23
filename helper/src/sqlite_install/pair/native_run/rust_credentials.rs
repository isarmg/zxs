//! Two confined workers for one activation. SQL holds metadata-only vault
//! witnesses and the isolated Web verifier; only the original jointly
//! deriving writer can populate the approved new 240-byte vault stage.
use super::{
    bad,
    joint_material::{Reader as JointReader, Writer as JointWriter},
    objects::object_body,
    transport::{Channel, Kind, Payload},
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
                VaultStamp, checked_vault_stamp, deployed_worker_payload, request_live_approval,
                sealed, verify_fresh_worker,
            },
            fresh_issuer::{InitialDatabase, SitePayload, require_private_install},
            native_sql::{
                Original,
                rust_vault::{RustAccountCut, RustChange, observed_binding},
            },
            rust_enroll::{
                crypto::{CredentialHelper, Cryptography},
                manifest::{Authority, Intent, Manifest},
                receipt::Observed,
                secret_order::SecretOrder,
            },
            terminal::SelectedInput,
        },
    },
};
use helper_linux_evidence::diagnostic_process::account_sql_descriptors;
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
    sync::Arc,
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
        &binding,
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
    let complete = tty.reserve_joint(uid, gid, crypto, &binding)?;
    let (peer, [link]) =
        channel.wait_checked::<1>(Kind::JointWriterPeer, &target.action, until, &alive)?;
    let writer = JointWriter::connect(
        link,
        child_peer(peer, &target.target.record, uid, gid)?,
        &binding,
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
