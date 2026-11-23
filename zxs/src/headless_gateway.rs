//! Retained local authority for one already loaded, independently approved gateway.
use crate::hash::digest;
use std::{
    ffi::c_int,
    fs::{File, OpenOptions},
    io,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{FileExt, MetadataExt, OpenOptionsExt},
    },
};

const CONFIG: &str = "/etc/zxs-gateway/nginx.conf";
const PREFIX: &[u8] = b"ZXS-GATEWAY-1\0";
const RECORD: usize = PREFIX.len() + 4 + 8 * 5 + 32;
const MAX_CONFIG: u64 = 64 * 1024;

unsafe extern "C" {
    fn zxs_gateway_duplicate(fd: c_int) -> c_int;
    fn zxs_gateway_read_flags(directory: bool) -> c_int;
    fn zxs_gateway_pidfs(fd: c_int) -> c_int;
    fn zxs_gateway_is_live(fd: c_int) -> c_int;
    fn zxs_gateway_has_receipt_seals(fd: c_int) -> bool;
    fn zxs_gateway_is_readonly_file(fd: c_int) -> bool;
}

fn duplicate_for_handoff(slot: c_int) -> io::Result<File> {
    // SAFETY: the kernel validates the slot without dereferencing memory
    // or taking ownership of the original; new descriptors cannot alias 3–5.
    let fd = unsafe { zxs_gateway_duplicate(slot) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful duplication allocated a fresh owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn open_original(path: &str, directory: bool) -> io::Result<File> {
    // SAFETY: these are fixed system open flags with no descriptor or memory access.
    let flags = unsafe { zxs_gateway_read_flags(directory) };
    OpenOptions::new().read(true).custom_flags(flags).open(path)
}

fn bad() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "the original enrolled HTTPS gateway was not proved",
    )
}

/// Only the protected, directly controlling host Helper launcher can supply
/// a root-owned, sealed loaded-launch receipt. No argv PID, generic request
/// header, editable enrollment file or invented signature is accepted.
pub(crate) struct LaunchFds {
    pub(crate) original_pidfd: File,
    pub(crate) approved_config: File,
    pub(crate) loaded_receipt: File,
}

impl LaunchFds {
    /// Reserved native invocation ABI: 3 original PIDFD, 4 approved config,
    /// 5 sealed root launch receipt. No user arguments or environment vars.
    pub(crate) fn inherit() -> io::Result<Self> {
        let result = (|| {
            Ok(Self {
                original_pidfd: duplicate_for_handoff(3)?,
                approved_config: duplicate_for_handoff(4)?,
                loaded_receipt: duplicate_for_handoff(5)?,
            })
        })();
        result.map_err(|_: io::Error| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "no complete approved gateway descriptor handoff",
            )
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    device: u64,
    inode: u64,
    bytes: u64,
    mode: u32,
    modified: (i64, i64),
    changed: (i64, i64),
}

pub(crate) struct GatewayRun {
    pidfd: File,
    approved: File,
    config: Stamp,
    uid: u32,
}

fn config_stamp(file: &File) -> io::Result<Stamp> {
    let m = file.metadata()?;
    if !m.is_file()
        || m.uid() != 0
        || m.nlink() != 1
        || m.ino() == 0
        || m.mode() & 0o022 != 0
        || !(1..=MAX_CONFIG).contains(&m.len())
    {
        return Err(bad());
    }
    Ok(Stamp {
        device: m.dev(),
        inode: m.ino(),
        bytes: m.len(),
        mode: m.mode(),
        modified: (m.mtime(), m.mtime_nsec()),
        changed: (m.ctime(), m.ctime_nsec()),
    })
}

fn named_config() -> io::Result<File> {
    for path in ["/etc", "/etc/zxs-gateway"] {
        let info = std::fs::symlink_metadata(path)?;
        if !info.is_dir() || info.uid() != 0 || info.mode() & 0o022 != 0 {
            return Err(bad());
        }
    }
    open_original(CONFIG, false)
}

fn process_id(pidfd: &File) -> io::Result<(u64, u64)> {
    // SAFETY: this only observes the filesystem of the retained fd.
    let original_type = unsafe { zxs_gateway_pidfs(pidfd.as_raw_fd()) };
    if original_type < 0 {
        return Err(io::Error::last_os_error());
    }
    if original_type != 1 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "expected a native PIDFS process descriptor",
        ));
    }
    let id = pidfd.metadata()?;
    if id.ino() == 0 {
        return Err(bad());
    }
    Ok((id.dev(), id.ino()))
}

fn live(pidfd: &File) -> io::Result<()> {
    // SAFETY: zero-timeout observation of the retained original fd only.
    let result = unsafe { zxs_gateway_is_live(pidfd.as_raw_fd()) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    if result == 1 { Ok(()) } else { Err(bad()) }
}

fn checked_receipt<const N: usize>(file: &File) -> io::Result<[u8; N]> {
    let m = file.metadata()?;
    if !m.is_file()
        || m.uid() != 0
        || m.nlink() != 0
        || m.mode() & 0o7777 != 0o400
        || m.len() != N as u64
    {
        return Err(bad());
    }
    // SAFETY: the seal check only observes the retained descriptor.
    if !unsafe { zxs_gateway_has_receipt_seals(file.as_raw_fd()) } {
        return Err(bad());
    }
    let mut data = [0; N];
    file.read_exact_at(&mut data, 0)?;
    Ok(data)
}

fn next<const N: usize>(record: &mut &[u8]) -> io::Result<[u8; N]> {
    let (item, rest) = record.split_at_checked(N).ok_or_else(bad)?;
    *record = rest;
    item.try_into().map_err(|_| bad())
}

impl GatewayRun {
    pub(crate) fn retain(
        inputs: LaunchFds,
        enrolled_uid: u32,
        state: &ServiceEpoch,
    ) -> io::Result<Self> {
        let LaunchFds {
            original_pidfd: pidfd,
            approved_config: approved,
            loaded_receipt,
        } = inputs;
        let raw = checked_receipt::<{ RECORD + SERVICE_TAIL }>(&loaded_receipt)?;
        let mut data = raw.strip_prefix(PREFIX).ok_or_else(bad)?;
        let uid = u32::from_be_bytes(next(&mut data)?);
        let process = (
            u64::from_be_bytes(next(&mut data)?),
            u64::from_be_bytes(next(&mut data)?),
        );
        let device = u64::from_be_bytes(next(&mut data)?);
        let inode = u64::from_be_bytes(next(&mut data)?);
        let bytes = u64::from_be_bytes(next(&mut data)?);
        let sha: [u8; 32] = next(&mut data)?;
        if &ServiceEpoch::parse(&mut data)? != state
            || !data.is_empty()
            || uid != enrolled_uid
            || matches!(uid, 0 | 65534 | u32::MAX)
        {
            return Err(bad());
        }

        // Keep the exact kernel objects; never reconstruct from a numeric PID.
        // SAFETY: the status check only observes the retained config descriptor.
        if !unsafe { zxs_gateway_is_readonly_file(approved.as_raw_fd()) } {
            return Err(bad());
        }
        if process_id(&pidfd)? != process {
            return Err(bad());
        }
        live(&pidfd)?;
        let config = config_stamp(&approved)?;
        if (config.device, config.inode, config.bytes) != (device, inode, bytes)
            || config_stamp(&named_config()?)? != config
        {
            return Err(bad());
        }
        // Bounded positional read so a preexisting inherited cursor cannot
        // change which reviewed bytes are authenticated.
        let mut contents = vec![0; usize::try_from(bytes).map_err(|_| bad())?];
        approved.read_exact_at(&mut contents, 0)?;
        if digest(&contents)? != sha || config_stamp(&approved)? != config {
            return Err(bad());
        }
        let selected = Self {
            pidfd,
            approved,
            config,
            uid,
        };
        selected.require_same_original(enrolled_uid)?;
        Ok(selected)
    }

    pub(crate) fn require_same_original(&self, enrolled_uid: u32) -> io::Result<()> {
        if enrolled_uid != self.uid
            || config_stamp(&self.approved)? != self.config
            || config_stamp(&named_config()?)? != self.config
        {
            return Err(bad());
        }
        live(&self.pidfd)
    }
}

// Detached launcher prototype. Activation requires qualified exact nginx image
// and no-include standalone configuration with a dedicated worker UID.
#[cfg(feature = "host-activation")]
use std::{
    ffi::{c_char, c_long},
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, process::CommandExt},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[cfg(feature = "host-activation")]
const NGINX: &str = "/usr/sbin/nginx";
#[cfg(feature = "host-activation")]
const PID_FILE: &str = "/run/zxs-gateway.pid";
#[cfg(feature = "host-activation")]
const CWD: &str = "/etc/zxs-gateway";
#[cfg(feature = "host-activation")]
unsafe extern "C" {
    fn zxs_gateway_create_memfd(label: *const c_char) -> c_int;
    fn zxs_gateway_seal_memfd(fd: c_int) -> c_int;
    fn zxs_gateway_open_pidfd(owned_child: c_int) -> c_long;
    fn zxs_gateway_lock_config(fd: c_int) -> c_int;
    fn zxs_gateway_quit_owned_child(owned_child: c_int) -> c_int;
    fn zxs_gateway_open_service_record(state: c_int) -> c_int;
    fn zxs_gateway_prepare_nginx_exec(config: c_int) -> c_int;
    fn zxs_gateway_prepare_service_exec(
        pidfd: c_int,
        config: c_int,
        receipt: c_int,
        uid: u32,
        gid: u32,
    ) -> c_int;
}

/// Only the qualified root installer supplies these approved measured bytes
/// and UID. They are not accepted from the application socket or caller argv.
#[cfg(feature = "host-activation")]
pub(crate) struct ApprovedNginx {
    pub(crate) executable_sha: [u8; 32],
    pub(crate) configuration_sha: [u8; 32],
    pub(crate) worker_uid: u32,
}

#[cfg(feature = "host-activation")]
fn protected_file(path: &str, expected_limit: u64) -> io::Result<File> {
    let file = open_original(path, false)?;
    let m = file.metadata()?;
    if !m.is_file()
        || m.uid() != 0
        || m.nlink() != 1
        || m.mode() & 0o022 != 0
        || m.len() > expected_limit
    {
        return Err(bad());
    }
    Ok(file)
}

#[cfg(feature = "host-activation")]
fn protected_dir(path: &str) -> io::Result<()> {
    let m = std::fs::symlink_metadata(path)?;
    if !m.is_dir() || m.uid() != 0 || m.mode() & 0o022 != 0 {
        return Err(bad());
    }
    Ok(())
}

#[cfg(feature = "host-activation")]
fn sealed(label: &std::ffi::CStr, contents: &[u8]) -> io::Result<File> {
    // SAFETY: label is a terminated C string; a successful fd is new.
    let fd = unsafe { zxs_gateway_create_memfd(label.as_ptr()) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: this consumes the newly created unique descriptor.
    let mut file = unsafe { File::from_raw_fd(fd) };
    file.write_all(contents)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o400))?;
    // SAFETY: seals are applied only to this private memfd.
    if unsafe { zxs_gateway_seal_memfd(file.as_raw_fd()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(file)
}

#[cfg(feature = "host-activation")]
fn handoff(
    child: &mut Child,
    approved: File,
    stamp: Stamp,
    policy: &ApprovedNginx,
) -> io::Result<LaunchFds> {
    let child_pid = c_int::try_from(child.id()).map_err(|_| bad())?;
    // SAFETY: child is a directly spawned child owned here, not an externally
    // specified PID. No other thread or handler is permitted to reap it.
    let fd = unsafe { zxs_gateway_open_pidfd(child_pid) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful pidfd_open created this new descriptor.
    let pidfd = unsafe { File::from_raw_fd(c_int::try_from(fd).map_err(|_| bad())?) };
    let (dev, ino) = process_id(&pidfd)?;
    let expected = format!("{child_pid}\n");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        live(&pidfd)?;
        match protected_file(PID_FILE, 32) {
            Ok(file) => {
                let mut pid_bytes = Vec::with_capacity(32);
                file.take(33).read_to_end(&mut pid_bytes)?;
                if pid_bytes == expected.as_bytes() {
                    break;
                }
                if !expected.as_bytes().starts_with(&pid_bytes) {
                    return Err(bad());
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "gateway did not load its approved configuration",
            ));
        }
        thread::sleep(Duration::from_millis(20));
    }
    if config_stamp(&approved)? != stamp || config_stamp(&named_config()?)? != stamp {
        return Err(bad());
    }
    let mut bytes = Vec::with_capacity(RECORD);
    bytes.extend_from_slice(PREFIX);
    bytes.extend_from_slice(&policy.worker_uid.to_be_bytes());
    for value in [dev, ino, stamp.device, stamp.inode, stamp.bytes] {
        bytes.extend_from_slice(&value.to_be_bytes());
    }
    bytes.extend_from_slice(&policy.configuration_sha);
    let loaded_receipt = sealed(c"zxs-gateway-loaded", &bytes)?;
    live(&pidfd)?;
    Ok(LaunchFds {
        original_pidfd: pidfd,
        approved_config: approved,
        loaded_receipt,
    })
}

/// Caller retains the returned direct child. On successful handoff it must
/// either keep it under the already approved local host lifecycle or pass the
/// three descriptors to boot over private IPC. No reload or binary upgrade.
#[cfg(feature = "host-activation")]
pub(crate) fn launch_approved(policy: &ApprovedNginx) -> io::Result<(Child, LaunchFds)> {
    if crate::fs::local_uid() != 0 || matches!(policy.worker_uid, 0 | 65534 | u32::MAX) {
        return Err(bad());
    }
    for path in ["/usr", "/usr/sbin", "/run", CWD] {
        protected_dir(path)?;
    }
    let approved = named_config()?;
    // SAFETY: flock acts only on the protected config inode, serializing
    // competing invocations of this prototype until the original PID is live.
    if unsafe { zxs_gateway_lock_config(approved.as_raw_fd()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    match std::fs::symlink_metadata(PID_FILE) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "gateway pidfile already exists",
            ));
        }
    }
    let executable = protected_file(NGINX, 64 * 1024 * 1024)?;
    let mut image = Vec::new();
    (&executable)
        .take(64 * 1024 * 1024 + 1)
        .read_to_end(&mut image)?;
    if image.len() > 64 * 1024 * 1024 || digest(&image)? != policy.executable_sha {
        return Err(bad());
    }
    let stamp = config_stamp(&approved)?;
    let mut contents = vec![0; usize::try_from(stamp.bytes).map_err(|_| bad())?];
    approved.read_exact_at(&mut contents, 0)?;
    if digest(&contents)? != policy.configuration_sha || config_stamp(&approved)? != stamp {
        return Err(bad());
    }
    let input = sealed(c"zxs-gateway-config", &contents)?;
    let config_fd = input.as_raw_fd();
    let program = format!("/proc/self/fd/{}", executable.as_raw_fd());
    let config_path = format!("/proc/self/fd/{config_fd}");
    let mut nginx = Command::new(program);
    nginx
        .env_clear()
        .current_dir(CWD)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .args([
            "-p",
            "/etc/zxs-gateway/",
            "-c",
            &config_path,
            "-g",
            "daemon off;",
        ]);
    // SAFETY: mark every nonstandard inherited FD CLOEXEC, then expose
    // only the input sealed config to the approved gateway child.
    unsafe {
        nginx.pre_exec(move || {
            if zxs_gateway_prepare_nginx_exec(config_fd) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let handoff_config = approved.try_clone()?;
    let mut child = nginx.spawn()?;
    match handoff(&mut child, handoff_config, stamp, policy) {
        Ok(fds) => Ok((child, fds)),
        Err(error) => {
            // The directly spawned, exclusively owned Child is the only target.
            // nginx's master must get a chance to stop its workers and retire
            // its own pidfile; SIGKILL would leave both duties unfinished.
            if let Err(stop) = stop_failed_launch(&mut child) {
                return Err(io::Error::other(format!(
                    "gateway proof failed ({error}); original gateway shutdown was not requested ({stop})"
                )));
            }
            Err(error)
        }
    }
}

#[cfg(feature = "host-activation")]
fn stop_failed_launch(child: &mut Child) -> io::Result<()> {
    if child.try_wait()?.is_some() {
        return Ok(());
    }
    let pid = c_int::try_from(child.id()).map_err(|_| bad())?;
    // SAFETY: this is our direct unreaped child, never a value from the
    // pidfile. The one-shot launcher has no other child waiter.
    if unsafe { zxs_gateway_quit_owned_child(pid) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(feature = "host-activation")]
const SERVICE: &str = "/zxs/bin/zxs";

/// A native, already approved offline deployment selection. Do not
/// synthesize account IDs or accept these values via application input.
#[cfg(feature = "host-activation")]
pub(crate) struct ApprovedService {
    pub(crate) image_sha: [u8; 32],
    pub(crate) uid: u32,
    pub(crate) ipc_gid: u32,
}

#[derive(Eq, PartialEq)]
pub(crate) struct ServiceEpoch {
    pub(crate) owner: u32,
    pub(crate) group: u32,
    pub(crate) device: u64,
    pub(crate) inode: u64,
    pub(crate) epoch: [u8; 16],
}

const SERVICE_TAIL: usize = 2 * 4 + 2 * 8 + 16;

impl ServiceEpoch {
    #[cfg(feature = "host-activation")]
    fn seal(&self, raw: &[u8; RECORD]) -> io::Result<File> {
        let mut bytes = Vec::with_capacity(RECORD + SERVICE_TAIL);
        bytes.extend_from_slice(raw);
        bytes.extend_from_slice(&self.owner.to_be_bytes());
        bytes.extend_from_slice(&self.group.to_be_bytes());
        bytes.extend_from_slice(&self.device.to_be_bytes());
        bytes.extend_from_slice(&self.inode.to_be_bytes());
        bytes.extend_from_slice(&self.epoch);
        sealed(c"zxs-headless-selected", &bytes)
    }

    fn parse(input: &mut &[u8]) -> io::Result<Self> {
        Ok(Self {
            owner: u32::from_be_bytes(next(input)?),
            group: u32::from_be_bytes(next(input)?),
            device: u64::from_be_bytes(next(input)?),
            inode: u64::from_be_bytes(next(input)?),
            epoch: next(input)?,
        })
    }
}

#[cfg(feature = "host-activation")]
fn load_service(policy: &ApprovedService, gateway: &LaunchFds) -> io::Result<(File, File, File)> {
    use crate::{headless_auth::Descriptor, headless_start::DEFAULT_STATE};

    if crate::fs::local_uid() != 0
        || matches!(policy.uid, 0 | 65534 | u32::MAX)
        || matches!(policy.ipc_gid, 0 | 65534 | u32::MAX)
    {
        return Err(bad());
    }
    for directory in ["/zxs", "/zxs/bin", "/zxs/state"] {
        protected_dir(directory)?;
    }
    let open_directory = |path: &str| open_original(path, true);
    let state = open_directory(DEFAULT_STATE)?;
    let info = state.metadata()?;
    if info.uid() != policy.uid || info.mode() & 0o7777 != 0o700 {
        return Err(bad());
    }
    state.try_lock_shared()?;
    // SAFETY: state is an original retained directory and the static
    // NUL-terminated component has no path separators.
    let fd = unsafe { zxs_gateway_open_service_record(state.as_raw_fd()) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful openat created a unique owned descriptor.
    let descriptor = unsafe { File::from_raw_fd(fd) };
    let (_, account) = Descriptor::read_selected(&descriptor, policy.uid)?.into_parts();
    let uid = account.gateway_uid();
    if uid == policy.uid {
        return Err(bad());
    }
    let initial = checked_receipt::<RECORD>(&gateway.loaded_receipt)?;
    let data = initial.strip_prefix(PREFIX).ok_or_else(bad)?;
    let mut raw = data;
    if u32::from_be_bytes(next(&mut raw)?) != uid
        || (
            u64::from_be_bytes(next(&mut raw)?),
            u64::from_be_bytes(next(&mut raw)?),
        ) != process_id(&gateway.original_pidfd)?
    {
        return Err(bad());
    }
    live(&gateway.original_pidfd)?;
    let ipc = open_directory("/zxs/run")?.metadata()?;
    if ipc.uid() != policy.uid || ipc.gid() != policy.ipc_gid || ipc.mode() & 0o7777 != 0o710 {
        return Err(bad());
    }
    let image = protected_file(SERVICE, 128 * 1024 * 1024)?;
    let image_info = image.metadata()?;
    if image_info.mode() & 0o7111 != 0o111 {
        return Err(bad());
    }
    let mut digestor = crate::hash::Sha256::new();
    let mut reader = (&image).take(128 * 1024 * 1024 + 1);
    let mut block = [0u8; 64 * 1024];
    let mut size = 0;
    loop {
        let n = reader.read(&mut block)?;
        if n == 0 {
            break;
        }
        if size == 0 && !block[..n].starts_with(b"\x7fELF") {
            return Err(bad());
        }
        size += n;
        digestor.update(&block[..n])?;
    }
    if size == 0
        || size > 128 * 1024 * 1024
        || digestor.finish() != policy.image_sha
        || image_info.ctime() != image.metadata()?.ctime()
        || image_info.ctime_nsec() != image.metadata()?.ctime_nsec()
        || !matches!(open_directory(DEFAULT_STATE)?.metadata(), Ok(m) if m.dev() == info.dev() && m.ino() == info.ino())
    {
        return Err(bad());
    }
    let epoch = ServiceEpoch {
        owner: policy.uid,
        group: policy.ipc_gid,
        device: info.dev(),
        inode: info.ino(),
        epoch: account.epoch(),
    };
    Ok((image, state, epoch.seal(&initial)?))
}

/// Native one-shot launch from an approved actual provisioner. The returned
/// Child is ordinary OS custody; no RPC, helper daemon, restart, or reload.
#[cfg(feature = "host-activation")]
pub(crate) fn launch_service(policy: &ApprovedService, gateway: &LaunchFds) -> io::Result<Child> {
    let spare = |file: &File| duplicate_for_handoff(file.as_raw_fd());
    let (image, _original_state_lease, sealed) = load_service(policy, gateway)?;
    let image = spare(&image)?;
    let descriptors = [
        spare(&gateway.original_pidfd)?,
        spare(&gateway.approved_config)?,
        spare(&sealed)?,
    ];
    let mut command = Command::new(format!("/proc/self/fd/{}", image.as_raw_fd()));
    command
        .env_clear()
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let uid = policy.uid;
    let gid = policy.ipc_gid;
    // SAFETY: pre-exec performs only local syscalls on retained descriptors
    // and immediate values, no locks, allocation, or shared-memory mutation.
    unsafe {
        command.pre_exec(move || {
            if zxs_gateway_prepare_service_exec(
                descriptors[0].as_raw_fd(),
                descriptors[1].as_raw_fd(),
                descriptors[2].as_raw_fd(),
                uid,
                gid,
            ) != 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command.spawn()
}

#[cfg(feature = "host-activation")]
#[path = "headless_activation.rs"]
mod activation;
#[cfg(feature = "host-activation")]
pub(crate) use activation::activate;
