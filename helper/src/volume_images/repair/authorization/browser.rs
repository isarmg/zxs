//! The optional fixed working-copy page is a request to display the existing
//! terminal challenge. The child receives neither source, destination, checker
//! nor terminal. Nothing here can act on a disk or grant the parent authority.
use super::{RepairFormat, SHAPE, TAG, no, raw_scope};
use crate::local_worker::{
    Worker, sealed, sealed_owner,
    transport::{self, Channel, Kind},
};
use crate::{
    disk_web,
    foreground::{PublicConfirmation, only_inherited_descriptors},
    fresh_random::operation as random,
    maintenance,
    pinned::helper_elf::ForegroundElf,
};
use helper_linux_evidence::{
    diagnostic_process::{OfflineSql, account_sql_descriptors},
    prompt_signals::PromptSignals,
};
use rustix::{
    net::{UCred, sockopt},
    process::{
        DumpableBehavior, Pid, Signal, dumpable_behavior, getgid, getppid, getuid,
        parent_process_death_signal, set_dumpable_behavior,
    },
};
use sha2::{Digest, Sha256};
use std::{
    io,
    net::{Ipv4Addr, TcpListener},
    time::{Duration, Instant},
};

const WINDOW: Duration = Duration::from_mins(8);

pub(super) fn offer(
    tty: &PublicConfirmation,
    format: RepairFormat,
    bytes: u64,
    require: impl Fn() -> io::Result<()>,
) -> io::Result<()> {
    tty.check()?;
    require()?;
    let (uid, gid) = (getuid(), getgid());
    maintenance::diagnostic_worker(uid.as_raw(), gid.as_raw())?;
    let mut scope = TAG.to_vec();
    scope.extend(random::<32>()?);
    scope.push(format.scope_code());
    scope.extend(bytes.to_be_bytes());
    raw_scope(&scope)?;
    let digest = Sha256::digest(&scope).into();
    let envelope = sealed(&scope)?;
    let helper = ForegroundElf::local()?;
    let signals = PromptSignals::begin()?;
    let check = || {
        signals.check()?;
        tty.check()
    };
    check()?;
    let (left, right) = transport::pair()?;
    let mut child = Worker::spawn(
        helper.descriptor(),
        &right,
        &envelope,
        OfflineSql::CopyRepairBrowser,
    )?;
    drop(right);
    drop(envelope);
    let channel = Channel::from_owned_child_session(
        left,
        UCred {
            pid: child.pid,
            uid,
            gid,
        },
        digest,
    )?;
    let wait = Instant::now() + Duration::from_secs(10);
    let (ready, []) =
        channel.wait_checked::<0>(Kind::CopyRepairBrowserReady, &digest, wait, &check)?;
    let port = u16::from_be_bytes(ready[..2].try_into().expect("bounded fixed port"));
    if port == 0 || ready[2..34] == [0; 32] || ready[34..] != [0; 31] {
        return Err(no());
    }
    child.live()?;
    helper.require()?;
    require()?;
    let code = crate::fs_tools::lower_hex(&ready[2..34]);
    tty.report(&format!("仅本机 {} 镜像工作副本请求：http://127.0.0.1:{port}/\n一次性配对码（2 分钟有效，请勿分享）：{code}\n网页只把请求交回这个终端，不读取或修复镜像。\n",format.panel_label()))?;
    let end = wait + WINDOW;
    let (request, []) =
        channel.wait_checked::<0>(Kind::CopyRepairBrowserRequest, &digest, end, &check)?;
    if request != [0; 65] {
        return Err(no());
    }
    // Closing HTTP, EOF on the original result endpoint and an actual
    // successful child exit all precede the new foreground challenge.
    child.finish(&channel, end)?;
    check()?;
    signals.finish()?;
    drop(channel);
    drop(child);
    helper.require()?;
    require()
}

pub(super) fn worker() -> io::Result<()> {
    let [fd, meta] = account_sql_descriptors()?;
    only_inherited_descriptors(&[3, 4])?;
    let (uid, gid) = (getuid(), getgid());
    maintenance::diagnostic_worker(uid.as_raw(), gid.as_raw())?;
    let same = |parent: Pid| {
        if getppid() == Some(parent) {
            Ok(())
        } else {
            Err(no())
        }
    };
    let parent = getppid().ok_or_else(no)?;
    let who = sockopt::socket_peercred(&fd)?;
    if who.pid != parent
        || who.uid != uid
        || who.gid != gid
        || parent_process_death_signal()? != Some(Signal::TERM)
    {
        return Err(no());
    }
    ForegroundElf::local()?.parent_is_same_image(parent)?;
    set_dumpable_behavior(DumpableBehavior::NotDumpable)?;
    if dumpable_behavior()? != DumpableBehavior::NotDumpable {
        return Err(no());
    }
    let raw = sealed_owner(&meta, SHAPE, (uid.as_raw(), gid.as_raw()))?;
    drop(meta);
    let (format, size) = raw_scope(&raw)?;
    let digest = Sha256::digest(&raw).into();
    let channel = Channel::from_owned_child_session(fd, who, digest)?;
    same(parent)?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    helper_linux_evidence::panel_confinement::first_install_panel(&listener)?;
    let port = listener.local_addr()?.port();
    let code = random::<32>()?;
    let mut ready = [0; 65];
    ready[..2].copy_from_slice(&port.to_be_bytes());
    ready[2..34].copy_from_slice(&code);
    same(parent)?;
    channel.send(Kind::CopyRepairBrowserReady, &digest, &ready)?;
    if !disk_web::serve_repair(
        listener,
        format,
        size,
        code,
        Instant::now() + WINDOW,
        || same(parent),
    )? {
        return Err(no());
    }
    same(parent)?;
    channel.send(Kind::CopyRepairBrowserRequest, &digest, &[0; 65])
}
