// This ignored executable case can only observe the fixed disposable fixture.
use super::cross_f_observe::{FixtureRequest, Phase, Retirement, observe};
use std::{
    fs::File,
    io::{self, ErrorKind},
    os::fd::{AsRawFd, FromRawFd},
    os::unix::fs::{FileExt, MetadataExt},
};

const FIXTURE_FD: i32 = 3;
const FIXTURE_ID: u32 = 23100;
const MAX_RECORD: usize = 512;

fn refuse() -> io::Error { io::Error::new(ErrorKind::PermissionDenied, "not an original guest history observation") }

fn descriptor() -> io::Result<File> {
    // SAFETY: these ID queries do not access memory. This exact ignored
    // process owns FD 3; the named fixture runner exclusively supplies it.
    let (uid, euid, gid, egid, flags, seals, nnp) = unsafe {
        (libc::getuid(), libc::geteuid(), libc::getgid(), libc::getegid(),
            libc::fcntl(FIXTURE_FD, libc::F_GETFL), libc::fcntl(FIXTURE_FD, libc::F_GET_SEALS),
            libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0 as libc::c_ulong, 0 as libc::c_ulong, 0 as libc::c_ulong, 0 as libc::c_ulong))
    };
    let wanted = libc::F_SEAL_WRITE | libc::F_SEAL_GROW | libc::F_SEAL_SHRINK | libc::F_SEAL_SEAL;
    if [uid, euid, gid, egid] != [FIXTURE_ID; 4] || nnp != 1
        || flags < 0 || flags & (libc::O_PATH | libc::O_ACCMODE) != libc::O_RDONLY
        || seals < 0 || seals & wanted != wanted {
        return Err(refuse());
    }
    // SAFETY: the verified inherited original is not stdio and has no other
    // Rust owner in this one exact libtest case.
    let held = unsafe { File::from_raw_fd(FIXTURE_FD) };
    let m = held.metadata()?;
    if !m.is_file() || m.nlink() != 0 || m.uid() != 0 || m.gid() != 0 || m.mode() & 0o7777 != 0o400
        || !matches!(usize::try_from(m.size()), Ok(1..=MAX_RECORD)) {
        return Err(refuse());
    }
    // SAFETY: query and set close-on-exec on this same contained test FD. No
    // subprocess is ever spawned, and the inherited access mode is unchanged.
    if unsafe { libc::fcntl(held.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(held)
}

fn fixture() -> io::Result<()> {
    let held = descriptor()?;
    let len = usize::try_from(held.metadata()?.size()).map_err(|_| refuse())?;
    let mut raw = [0; MAX_RECORD];
    let record = raw.get_mut(..len).ok_or_else(refuse)?;
    held.read_exact_at(record, 0)?;
    let text = std::str::from_utf8(record).map_err(|_| refuse())?;
    let Some(text) = text.strip_suffix('\n') else { return Err(refuse()) };
    let parts: Vec<&str> = text.split('\n').collect();
    let ["SMB-MOVE-HISTORY-1", nonce, pool, from, to, file, a, b, movement] = parts.as_slice() else {
        return Err(refuse());
    };
    // This type supplies exact nonzero 32-digit lower-case hex validation.
    let challenge: crate::OperationId = nonce.parse()?;
    let observed = observe(&FixtureRequest {
        pool: pool.parse()?, login: "movecheck", from: from.parse()?, to: to.parse()?,
        file: file.parse()?, nodes: [a.parse()?, b.parse()?],
        phase: if *movement == "-" { Phase::Source } else { Phase::Target { move_op: movement.parse()? } },
    })?;
    let retirement = match observed.retirement {
        Retirement::NotMoved => "source",
        Retirement::Pending => "pending",
        Retirement::Retired => "retired",
    };
    let [first, second] = observed.versions;
    let hex = |digest: [u8; 32]| digest.iter().map(|b| format!("{b:02x}")).collect::<String>();
    println!("\nSMB-MOVE-HISTORY {challenge} {} {} {} {} {retirement} {} {} {} {} {} {} {} {} {} {}",
        observed.file, observed.volume, observed.position, observed.generation,
        first.node, first.save, first.revision, first.bytes, hex(first.sha256),
        second.node, second.save, second.revision, second.bytes, hex(second.sha256));
    Ok(())
}

#[test]
#[ignore = "only for the independently approved, sealed disposable Linux MOVE fixture"]
fn guest_move_history_record() {
    assert!(fixture().is_ok(), "fixed read-only historical fixture did not confirm");
}
