//! Private offline workers invoked by the original local maintenance tool.
use crate::{
    container_store::{
        Body, Receipt,
        import_one::{ImportIntent, SourceWitness},
    },
    hash::Sha256,
    headless_offline,
};
use std::{
    cell::Cell,
    fs::File,
    io::{self, Read},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::MetadataExt,
    },
};

const WIRE: usize = 138;
const HEADER: usize = 73;
const PATH_BYTES: usize = 4095;
const MAX_ENVELOPE: usize = 72 + 8 + 16 + 16 + 32 + 3 * (2 + PATH_BYTES) + 104 + 2 + 255;
type Payload = [u8; 65];

pub(crate) mod batch;
pub(crate) mod verify;

#[path = "headless_init_data_worker.rs"]
pub(crate) mod init_data;

unsafe extern "C" {
    fn zxs_import_take_inherited(fd: i32) -> i32;
    fn zxs_import_socket(fd: i32, parent: i32) -> i32;
    fn zxs_import_parent() -> i32;
    fn zxs_import_sealed(fd: i32) -> i32;
    fn zxs_import_reader(fd: i32) -> i32;
    fn zxs_mount_id(fd: i32, out: *mut u64) -> i32;
    fn zxs_import_send(fd: i32, data: *const u8, size: usize) -> isize;
    fn zxs_import_receive(
        fd: i32,
        data: *mut u8,
        size: usize,
        parent: i32,
        body_fd: *mut i32,
    ) -> isize;
}

fn inherited_order() -> io::Result<(File, i32, Vec<u8>)> {
    inherited_order_with_limit(MAX_ENVELOPE)
}

fn inherited_order_with_limit(max_bytes: usize) -> io::Result<(File, i32, Vec<u8>)> {
    let socket = inherited(3)?;
    let order = inherited(4)?;
    // SAFETY: getppid takes no arguments; socket lives across the descriptor check.
    let parent = unsafe { zxs_import_parent() };
    if parent <= 1 || unsafe { zxs_import_socket(socket.as_raw_fd(), parent) } != 0 {
        return Err(refused());
    }
    Ok((socket, parent, retained_order(order, max_bytes)?))
}

pub(crate) fn run() -> io::Result<()> {
    let (socket, parent, order_bytes) = inherited_order()?;
    let (fence, selection) = headless_offline::retain()?;
    let (scope, digest, intent) = order_intent(&order_bytes, fence.binding)?;
    let channel = Channel {
        socket,
        parent,
        scope,
        digest,
        kind: 45,
    };
    let (writer, state) = selection.bind_import(|state| fence.require(state))?;
    let (_, node) = intent.identity()?;
    let require = || fence.require(&state);
    if let Some(receipt) = writer.receipt(&intent, require).map_err(|_| refused())? {
        return channel.send(receipt_step(5, &receipt, node));
    }

    channel.send(step(1, 0))?;
    let (answer, source) = channel.receive()?;
    let mount = u64::from_be_bytes(answer[1..9].try_into().map_err(|_| refused())?);
    if mount == 0 || answer != step(2, mount) {
        return Err(refused());
    }
    let source = source.ok_or_else(refused)?;
    let proof = Cell::new(0_u64);
    let authorize = || {
        require()?;
        source_matches(&source, &intent, mount)?;
        let nonce = proof.get().checked_add(1).ok_or_else(refused)?;
        proof.set(nonce);
        channel.send(step(3, nonce))?;
        let (answer, descriptor) = channel.receive()?;
        if descriptor.is_some() || answer != step(4, nonce) {
            return Err(refused());
        }
        require()?;
        source_matches(&source, &intent, mount)
    };
    let receipt = writer
        .save(&intent, &source, authorize)
        .map_err(|_| refused())?;
    require()?;
    source_matches(&source, &intent, mount)?;
    channel.send(receipt_step(6, &receipt, node))
}

fn refused() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "offline import handoff refused",
    )
}

fn inherited(fd: i32) -> io::Result<File> {
    // SAFETY: only reserved descriptors 3 and 4 enter here, once each and before
    // filesystem opens. The native check proves the descriptor is currently open
    // and marks it close-on-exec before ownership.
    unsafe {
        if zxs_import_take_inherited(fd) == 0 {
            return Ok(File::from_raw_fd(fd));
        }
    }
    Err(refused())
}

fn retained_order(order: File, max_bytes: usize) -> io::Result<Vec<u8>> {
    let stamp = order.metadata()?;
    // SAFETY: order remains owned throughout this descriptor-only native check.
    if !stamp.is_file()
        || stamp.nlink() != 0
        || unsafe { zxs_import_sealed(order.as_raw_fd()) } != 0
    {
        return Err(refused());
    }
    let limit = u64::try_from(max_bytes).map_err(|_| refused())?;
    if stamp.len() > limit {
        return Err(refused());
    }
    let mut raw = Vec::new();
    order.take(limit + 1).read_to_end(&mut raw)?;
    if u64::try_from(raw.len()).map_err(|_| refused())? != stamp.len() {
        return Err(refused());
    }
    Ok(raw)
}

fn order_intent(
    mut bytes: &[u8],
    binding: [u8; 32],
) -> io::Result<([u8; 32], [u8; 32], ImportIntent)> {
    if fixed::<8>(&mut bytes)? != *b"ZXIOC1\0\0" || fixed::<32>(&mut bytes)? != binding {
        return Err(refused());
    }
    let scope = fixed(&mut bytes)?;
    if scope == [0; 32] {
        return Err(refused());
    }
    let mut hash = Sha256::new();
    hash.update(bytes)?;
    Ok((scope, hash.finish(), parse_ticket(bytes)?))
}

fn parse_ticket(mut bytes: &[u8]) -> io::Result<ImportIntent> {
    if fixed::<8>(&mut bytes)? != *b"ZXIMP1\0\0" {
        return Err(refused());
    }
    let seed = fixed(&mut bytes)?;
    let pool = fixed(&mut bytes)?;
    let data_token = fixed(&mut bytes)?;
    let data_path = variable(&mut bytes, PATH_BYTES)?;
    let source_root = variable(&mut bytes, PATH_BYTES)?;
    let source_relative = variable(&mut bytes, PATH_BYTES)?;
    let source = SourceWitness {
        device: u64::from_be_bytes(fixed(&mut bytes)?),
        inode: u64::from_be_bytes(fixed(&mut bytes)?),
        uid: u32::from_be_bytes(fixed(&mut bytes)?),
        mode: u32::from_be_bytes(fixed(&mut bytes)?),
        links: u64::from_be_bytes(fixed(&mut bytes)?),
        modified: pair(&mut bytes)?,
        changed: pair(&mut bytes)?,
    };
    let body = Body {
        length: u64::from_be_bytes(fixed(&mut bytes)?),
        sha256: fixed(&mut bytes)?,
    };
    let name = String::from_utf8(variable(&mut bytes, 255)?).map_err(|_| refused())?;
    if !bytes.is_empty() {
        return Err(refused());
    }
    let intent = ImportIntent {
        seed,
        pool,
        data_token,
        data_path,
        source_root,
        source_relative,
        source,
        name,
        body,
    };
    intent.identity()?;
    Ok(intent)
}

fn fixed<const N: usize>(bytes: &mut &[u8]) -> io::Result<[u8; N]> {
    let (part, tail) = bytes.split_at_checked(N).ok_or_else(refused)?;
    *bytes = tail;
    part.try_into().map_err(|_| refused())
}

fn variable(bytes: &mut &[u8], limit: usize) -> io::Result<Vec<u8>> {
    let width = usize::from(u16::from_be_bytes(fixed(bytes)?));
    if width > limit {
        return Err(refused());
    }
    let (part, tail) = bytes.split_at_checked(width).ok_or_else(refused)?;
    *bytes = tail;
    Ok(part.to_vec())
}

fn pair(bytes: &mut &[u8]) -> io::Result<(i64, i64)> {
    Ok((
        i64::from_be_bytes(fixed(bytes)?),
        i64::from_be_bytes(fixed(bytes)?),
    ))
}

fn source_matches(file: &File, intent: &ImportIntent, mount: u64) -> io::Result<()> {
    let fd = file.as_raw_fd();
    let mut actual_mount = 0_u64;
    // SAFETY: file owns fd here, and actual_mount is a writable u64 through the call.
    let safe_fd =
        unsafe { zxs_import_reader(fd) == 0 && zxs_mount_id(fd, &raw mut actual_mount) == 0 };
    let stamp = file.metadata()?;
    let expected = &intent.source;
    if !safe_fd
        || actual_mount != mount
        || !stamp.is_file()
        || stamp.dev() != expected.device
        || stamp.ino() != expected.inode
        || stamp.uid() != expected.uid
        || stamp.mode() != expected.mode
        || stamp.nlink() != expected.links
        || stamp.size() != intent.body.length
        || (stamp.mtime(), stamp.mtime_nsec()) != expected.modified
        || (stamp.ctime(), stamp.ctime_nsec()) != expected.changed
    {
        return Err(refused());
    }
    Ok(())
}

struct Channel {
    socket: File,
    parent: i32,
    scope: [u8; 32],
    digest: [u8; 32],
    kind: u8,
}

impl Channel {
    fn wire(&self, payload: Payload) -> [u8; WIRE] {
        let mut out = [0; WIRE];
        out[..8].copy_from_slice(b"ZXSAMW1\0");
        out[8] = self.kind;
        out[9..41].copy_from_slice(&self.scope);
        out[41..HEADER].copy_from_slice(&self.digest);
        out[HEADER..].copy_from_slice(&payload);
        out
    }

    fn send(&self, payload: Payload) -> io::Result<()> {
        let raw = self.wire(payload);
        // SAFETY: socket and raw remain valid for the duration of send.
        let sent = unsafe { zxs_import_send(self.socket.as_raw_fd(), raw.as_ptr(), raw.len()) };
        if usize::try_from(sent).ok() != Some(WIRE) {
            return Err(refused());
        }
        Ok(())
    }

    fn receive(&self) -> io::Result<(Payload, Option<File>)> {
        let mut raw = [0; WIRE];
        let mut fd = -1;
        // SAFETY: all storage is valid and writable for the stated widths; native code
        // gives ownership only of the single authenticated and close-on-exec descriptor.
        let got = unsafe {
            zxs_import_receive(
                self.socket.as_raw_fd(),
                raw.as_mut_ptr(),
                raw.len(),
                self.parent,
                &raw mut fd,
            )
        };
        // SAFETY: a nonnegative fd is fresh, as established by receive.
        let owned = (fd >= 0).then(|| unsafe { File::from_raw_fd(fd) });
        if usize::try_from(got).ok() != Some(WIRE) || raw[..HEADER] != self.wire([0; 65])[..HEADER]
        {
            return Err(refused());
        }
        Ok((raw[HEADER..].try_into().map_err(|_| refused())?, owned))
    }
}

fn step(kind: u8, word: u64) -> Payload {
    let mut out = [0; 65];
    out[0] = kind;
    out[1..9].copy_from_slice(&word.to_be_bytes());
    out
}

fn receipt_step(kind: u8, receipt: &Receipt, node: [u8; 16]) -> Payload {
    let mut out = step(kind, receipt.sequence);
    out[9..25].copy_from_slice(&receipt.mutation.container);
    out[25..41].copy_from_slice(&receipt.mutation.request);
    out[41..57].copy_from_slice(&node);
    out
}
