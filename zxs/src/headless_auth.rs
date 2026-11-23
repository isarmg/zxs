//! Fresh headless credential binding. No listener, HTTP parser or setup writer.
use crate::password::{Verifier, login_name};
use std::{
    ffi::OsStr,
    fs::File,
    io::{self, Read},
    mem::{MaybeUninit, size_of},
    os::{
        fd::AsFd,
        unix::{ffi::OsStrExt, fs::MetadataExt, net::UnixStream},
    },
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

const MAGIC: &[u8] = b"ZXS-HEADLESS-1\0";
const MAX_ROLE_PATH: usize = 4095;
const MAX_DESCRIPTOR: usize =
    MAGIC.len() + 16 + 16 + 4 + 16 + 32 + 1 + 64 + 3 * (2 + MAX_ROLE_PATH + 32);
#[path = "headless_fresh_password.rs"]
mod fresh_password;
const MAX_AUTH_SCHEME_VALUE: usize = 6 + 4 * (64usize + 1 + fresh_password::MAX_BYTES).div_ceil(3);

fn invalid() -> io::Error {
    io::ErrorKind::InvalidData.into()
}

pub(crate) struct Descriptor {
    pool: [u8; 16],
    epoch: [u8; 16],
    terminator_uid: u32,
    salt: [u8; 16],
    tag: [u8; 32],
    login: String,
    roots: SelectedRoots,
}

/// Lexically exact selections only. Startup must acquire real held media
/// separately; none of these strings is an authorization or a Store handle.
pub(crate) struct SelectedRoots {
    pub(crate) pool: [u8; 16],
    pub(crate) system: SelectedRoot,
    pub(crate) data: SelectedRoot,
    pub(crate) recycle: SelectedRoot,
}
pub(crate) struct SelectedRoot {
    path: PathBuf,
    pub(crate) token: [u8; 32],
}
impl SelectedRoot {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Descriptor {
    /// The startup owner must first hold the original deployment lease and
    /// open the exact Helper-selected, no-follow regular file beneath that
    /// protected directory. This function does not discover root identity or
    /// trust a user-selected pathname. Keep the lease through service drain.
    pub(crate) fn read_selected(file: File, owner: u32) -> io::Result<Self> {
        let info = file.metadata()?;
        if !info.is_file()
            || info.nlink() != 1
            || info.mode() & 0o7777 != 0o600
            || info.uid() != owner
            || info.len() > MAX_DESCRIPTOR as u64
        {
            return Err(invalid());
        }
        let stamp = |info: &std::fs::Metadata| {
            (
                info.dev(),
                info.ino(),
                info.uid(),
                info.nlink(),
                info.mode(),
                info.len(),
                info.mtime(),
                info.mtime_nsec(),
                info.ctime(),
                info.ctime_nsec(),
            )
        };
        let mut raw = Vec::with_capacity(MAX_DESCRIPTOR);
        (&file)
            .take(MAX_DESCRIPTOR as u64 + 1)
            .read_to_end(&mut raw)?;
        if raw.len() as u64 != info.len() || stamp(&file.metadata()?) != stamp(&info) {
            return Err(invalid());
        }
        Self::parse(&raw)
    }

    fn parse(raw: &[u8]) -> io::Result<Self> {
        if raw.len() > MAX_DESCRIPTOR {
            return Err(invalid());
        }
        let mut raw = raw.strip_prefix(MAGIC).ok_or_else(invalid)?;
        let pool = fixed(&mut raw)?;
        let epoch = fixed(&mut raw)?;
        let terminator_uid = u32::from_be_bytes(fixed(&mut raw)?);
        let salt = fixed(&mut raw)?;
        let tag = fixed(&mut raw)?;
        let [login_len] = fixed(&mut raw)?;
        let login = login_name(take(&mut raw, usize::from(login_len))?)?.to_owned();
        let system = root_field(&mut raw)?;
        let data = root_field(&mut raw)?;
        let recycle = root_field(&mut raw)?;
        let roots = [&system, &data, &recycle];
        if pool == [0; 16]
            || epoch == [0; 16]
            || pool == epoch
            || salt == [0; 16]
            || tag == [0; 32]
            || matches!(terminator_uid, 0 | 65534 | u32::MAX)
            || !raw.is_empty()
            || roots.iter().enumerate().any(|(i, root)| {
                roots[..i].iter().any(|old| {
                    old.token == root.token
                        || old.path.starts_with(&root.path)
                        || root.path.starts_with(&old.path)
                })
            })
        {
            return Err(invalid());
        }
        Ok(Self {
            pool,
            epoch,
            terminator_uid,
            salt,
            tag,
            login,
            roots: SelectedRoots {
                pool,
                system,
                data,
                recycle,
            },
        })
    }

    pub(crate) fn into_parts(self) -> (SelectedRoots, Account) {
        let account = Account {
            pool: self.pool,
            epoch: self.epoch,
            login: self.login,
            verifier: Verifier::new(self.salt, self.tag),
            terminator_uid: self.terminator_uid,
            open: AtomicBool::new(true),
        };
        (self.roots, account)
    }
}

fn take<'a>(raw: &mut &'a [u8], len: usize) -> io::Result<&'a [u8]> {
    let (start, rest) = raw.split_at_checked(len).ok_or_else(invalid)?;
    *raw = rest;
    Ok(start)
}

fn root_field(raw: &mut &[u8]) -> io::Result<SelectedRoot> {
    let len = usize::from(u16::from_be_bytes(fixed(raw)?));
    let path = take(raw, len)?;
    let absolute = path.strip_prefix(b"/").ok_or_else(invalid)?;
    if len > MAX_ROLE_PATH
        || path.contains(&0)
        || absolute
            .split(|b| *b == b'/')
            .any(|part| matches!(part, b"" | b"." | b".."))
    {
        return Err(invalid());
    }
    let token = fixed(raw)?;
    if token == [0; 32] {
        return Err(invalid());
    }
    Ok(SelectedRoot {
        path: PathBuf::from(OsStr::from_bytes(path)),
        token,
    })
}

fn fixed<const N: usize>(raw: &mut &[u8]) -> io::Result<[u8; N]> {
    take(raw, N)?.try_into().map_err(|_| invalid())
}

pub(crate) struct Account {
    pool: [u8; 16],
    epoch: [u8; 16],
    login: String,
    verifier: Verifier,
    terminator_uid: u32,
    open: AtomicBool,
}

pub(crate) struct Peer<'owner> {
    account: &'owner Account,
    _stream: &'owner UnixStream,
}

pub(crate) struct Permit<'owner> {
    account: &'owner Account,
    pool: [u8; 16],
    epoch: [u8; 16],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AuthError {
    Invalid,
    Unavailable,
}

impl Account {
    pub(crate) fn gateway_uid(&self) -> u32 {
        self.terminator_uid
    }
    pub(crate) fn epoch(&self) -> [u8; 16] {
        self.epoch
    }

    /// Private Unix accepted socket only. The terminator forwards the
    /// original Authorization header and never supplies an account ID.
    pub(crate) fn admit_peer<'owner>(
        &'owner self,
        stream: &'owner UnixStream,
    ) -> io::Result<Peer<'owner>> {
        if !self.open.load(Ordering::Acquire) || peer_uid(stream)? != self.terminator_uid {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        Ok(Peer {
            account: self,
            _stream: stream,
        })
    }

    /// Stop accepting/authorizing before drain. The offline Helper cannot
    /// change credentials allowed by a live, held deployment.
    pub(crate) fn stop_admission(&self) {
        self.open.store(false, Ordering::Release);
    }

    /// HTTP adapter runs the new typed dispatch here (exactly one complete
    /// request). Check the permit before inspecting Store, and pass the
    /// borrowed predicate into selection. The predicate never takes a Store
    /// lock, reads from the socket or authorizes any other Account/epoch.
    pub(crate) fn with_admission<T>(
        &self,
        permit: Permit<'_>,
        one_request: impl FnOnce(&dyn Fn() -> bool) -> T,
    ) -> Result<T, AuthError> {
        let allowed = || {
            std::ptr::eq(self, permit.account)
                && permit.pool == self.pool
                && permit.epoch == self.epoch
                && self.open.load(Ordering::Acquire)
        };
        if !allowed() {
            return Err(AuthError::Unavailable);
        }
        Ok(one_request(&allowed))
    }
}

impl<'owner> Peer<'owner> {
    /// Call before parsing body or entering typed dispatch, using original
    /// headers (duplicate occurrences must not be combined or discarded).
    pub(crate) fn authenticate(
        &self,
        headers: &[(&str, &str)],
    ) -> Result<Permit<'owner>, AuthError> {
        let account = self.account;
        if !account.open.load(Ordering::Acquire) {
            return Err(AuthError::Unavailable);
        }
        let credentials = credentials(headers)?;
        if credentials.login() != account.login.as_str() {
            return Err(AuthError::Invalid);
        }
        if !account
            .verifier
            .matches(credentials.password())
            .map_err(|_| AuthError::Unavailable)?
        {
            return Err(AuthError::Invalid);
        }
        if !account.open.load(Ordering::Acquire) {
            return Err(AuthError::Unavailable);
        }
        Ok(Permit {
            account,
            pool: account.pool,
            epoch: account.epoch,
        })
    }
}

fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    use std::os::fd::AsRawFd;
    let mut cred = MaybeUninit::<libc::ucred>::uninit();
    let mut len = libc::socklen_t::try_from(size_of::<libc::ucred>()).map_err(|_| invalid())?;
    // SAFETY: the borrowed socket stays open; both output buffers are
    // writable and sized for the fixed Linux SO_PEERCRED result.
    let result = unsafe {
        libc::getsockopt(
            stream.as_fd().as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            cred.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    if usize::try_from(len).ok() != Some(size_of::<libc::ucred>()) {
        return Err(invalid());
    }
    // SAFETY: successful SO_PEERCRED initialized exactly the requested struct.
    let cred = unsafe { cred.assume_init() };
    if cred.pid <= 0 {
        return Err(invalid());
    }
    Ok(cred.uid)
}

struct BasicCredentials {
    decoded: String,
    separator: usize,
}
impl BasicCredentials {
    fn login(&self) -> &str {
        &self.decoded[..self.separator]
    }
    fn password(&self) -> &str {
        &self.decoded[self.separator + 1..]
    }
}
fn credentials(headers: &[(&str, &str)]) -> Result<BasicCredentials, AuthError> {
    let mut values = headers
        .iter()
        .filter_map(|(k, v)| k.eq_ignore_ascii_case("authorization").then_some(*v));
    let raw = values.next().ok_or(AuthError::Invalid)?;
    if values.next().is_some() {
        return Err(AuthError::Invalid);
    }
    let decoded = String::from_utf8(decode_basic(raw)?).map_err(|_| AuthError::Invalid)?;
    let separator = decoded.find(':').ok_or(AuthError::Invalid)?;
    let bound = BasicCredentials { decoded, separator };
    if login_name(bound.login().as_bytes()).is_err()
        || !fresh_password::admissible(bound.password())
    {
        return Err(AuthError::Invalid);
    }
    Ok(bound)
}

fn decode_basic(value: &str) -> Result<Vec<u8>, AuthError> {
    fn digit(b: u8) -> Result<u8, AuthError> {
        match b {
            b'A'..=b'Z' => Ok(b - b'A'),
            b'a'..=b'z' => Ok(b - b'a' + 26),
            b'0'..=b'9' => Ok(b - b'0' + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => Err(AuthError::Invalid),
        }
    }
    let invalid = AuthError::Invalid;
    let (scheme, encoded) = value.split_once(' ').ok_or(invalid)?;
    if !scheme.eq_ignore_ascii_case("Basic")
        || value.len() > MAX_AUTH_SCHEME_VALUE
        || encoded.is_empty()
        || encoded.len() % 4 != 0
    {
        return Err(invalid);
    }
    let mut groups = encoded.as_bytes().chunks_exact(4).peekable();
    let mut data = Vec::with_capacity(encoded.len() / 4 * 3);
    while let Some(q) = groups.next() {
        let a = digit(q[0])?;
        let b = digit(q[1])?;
        data.push(a << 2 | b >> 4);
        match (q[2], q[3]) {
            (b'=', b'=') if groups.peek().is_none() && b & 15 == 0 => {}
            (b'=', _) => return Err(invalid),
            (c, b'=') if groups.peek().is_none() => {
                let c = digit(c)?;
                if c & 3 != 0 {
                    return Err(invalid);
                }
                data.push(b << 4 | c >> 2);
            }
            (c, d) => {
                let c = digit(c)?;
                data.push(b << 4 | c >> 2);
                data.push(c << 6 | digit(d)?);
            }
        }
    }
    Ok(data)
}

#[cfg(test)]
#[path = "../tests/unit/attached/headless_auth/tests.rs"]
mod tests;
