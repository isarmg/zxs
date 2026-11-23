//! A bounded, unprivileged browser request to display the original console
//! confirmation. This never signs an INIT or accepts a path/data/secret from
//! HTTP. Only the authenticated exact process can return a request.
use super::{
    bad,
    manifest::read_sealed,
    transport::{Channel, Kind},
    worker::original_parent,
};
use crate::{
    disk_web::{self, InstallPhase},
    pinned::helper_elf::ForegroundElf,
    sqlite_install::{
        approval,
        pair::{ExplicitSubject, domain::request_live_approval},
    },
};
use helper_linux_evidence::diagnostic_process::account_sql_descriptors;
use rustix::net::UCred;
use rustix::process::{
    DumpableBehavior, Gid, Signal, Uid, dumpable_behavior, getppid, set_dumpable_behavior,
    set_parent_process_death_signal,
};
use rustix::rand::{GetRandomFlags, getrandom};
use sha2::{Digest, Sha256};
use std::{
    io,
    net::{Ipv4Addr, TcpListener},
    time::{Duration, Instant},
};

const TAG: &[u8] = b"ZXS-HELPER/FIRST-BROWSER-2\0";
const FIXED: usize = TAG.len() + 32 + 32 + 16 + 8 + 1 + 1;
const PANEL: Duration = Duration::from_mins(8);
fn no() -> io::Error {
    bad("the original first-install browser request is unconfirmed; no installation was started")
}
pub(crate) fn worker() -> io::Result<()> {
    let [fd, meta] = account_sql_descriptors()?;
    approval::only_inherited_descriptors(&[3, 4])?;
    let parent = getppid().ok_or_else(no)?;
    original_parent(&fd, parent)?;
    approval::host_admin()?;
    ForegroundElf::host()?.parent_is_same_image(parent)?;
    let raw = read_sealed(&meta, FIXED)?;
    drop(meta);
    let p = raw
        .strip_prefix(TAG)
        .filter(|r| r.len() == 90)
        .ok_or_else(no)?;
    let desired = ExplicitSubject::checked(
        p[64..80].try_into().map_err(|_| no())?,
        u32::from_be_bytes(p[80..84].try_into().map_err(|_| no())?),
        u32::from_be_bytes(p[84..88].try_into().map_err(|_| no())?),
    )?;
    let (_, uid, gid) = desired.tuple();
    let phase = InstallPhase::from_wire(p[88]).ok_or_else(no)?;
    if p[..32] == [0; 32] || p[32..64] == [0; 32] || !(1..=62).contains(&p[89]) {
        return Err(no());
    }
    approval::drop_to_storage(uid, gid)?;
    set_parent_process_death_signal(Some(Signal::TERM))?;
    original_parent(&fd, parent)?;
    set_dumpable_behavior(DumpableBehavior::NotDumpable)?;
    if dumpable_behavior()? != DumpableBehavior::NotDumpable {
        return Err(no());
    }
    let scope = Sha256::digest(&raw).into();
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
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    helper_linux_evidence::panel_confinement::first_install_panel(&listener)?;
    let port = listener.local_addr()?.port();
    let mut code = [0u8; 32];
    if getrandom(&mut code[..], GetRandomFlags::NONBLOCK)? != code.len() || code == [0; 32] {
        return Err(no());
    }
    let mut message = [0u8; 65];
    message[..2].copy_from_slice(&port.to_be_bytes());
    message[2..34].copy_from_slice(&code);
    channel.send(Kind::FirstBrowserReady, &scope, &message)?;
    let same = || {
        if getppid() != Some(parent) {
            return Err(no());
        }
        Ok(())
    };
    if !disk_web::serve_first(listener, phase, p[89], code, Instant::now() + PANEL, same)? {
        return Err(no());
    }
    same()?;
    channel.send(Kind::FirstBrowserRequest, &scope, &[0; 65])
}
