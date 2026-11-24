//! Fresh headless credential binding. No listener, HTTP parser or setup writer.
use crate::{
    hash::Sha256,
    password::{Verifier, login_name},
};
use std::{
    ffi::OsStr,
    fs::File,
    io::{self, Read},
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

const MAGIC: &[u8] = b"ZXS-HTTP-ROSTER\0";
const MAX_ROLE_PATH: usize = 4095;
const MAX_DATA_ROOTS: usize = 62;
const MAX_DESCRIPTOR: usize =
    MAGIC.len() + 16 + 16 + 16 + 32 + 1 + 64 + 1 + (MAX_DATA_ROOTS + 2) * (2 + MAX_ROLE_PATH + 32);
#[path = "headless_fresh_password.rs"]
mod fresh_password;
mod session;
pub(crate) const SESSION_TTL_SECONDS: u64 = session::EXPIRES_SECONDS;
const MAX_AUTH_SCHEME_VALUE: usize = 6 + 4 * (64usize + 1 + fresh_password::MAX_BYTES).div_ceil(3);

fn invalid() -> io::Error {
    io::ErrorKind::InvalidData.into()
}

fn descriptor_digest(raw: &[u8]) -> io::Result<[u8; 32]> {
    let mut digest = Sha256::new();
    digest.update(raw)?;
    Ok(digest.finish())
}

pub(crate) struct Descriptor {
    pool: [u8; 16],
    epoch: [u8; 16],
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
    pub(crate) recycle: SelectedRoot,
    pub(crate) data: Vec<SelectedRoot>,
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
    pub(crate) fn read_selected(file: &File, owner: u32) -> io::Result<Self> {
        Self::read_checked(file, owner, Self::parse)
    }

    pub(crate) fn read_bound(file: &File, owner: u32) -> io::Result<(Self, [u8; 32])> {
        Self::read_checked(file, owner, |raw| {
            Ok((Self::parse(raw)?, descriptor_digest(raw)?))
        })
    }

    pub(crate) fn binding(file: &File, owner: u32) -> io::Result<[u8; 32]> {
        Self::read_checked(file, owner, descriptor_digest)
    }

    fn read_checked<T>(
        file: &File,
        owner: u32,
        checked: impl FnOnce(&[u8]) -> io::Result<T>,
    ) -> io::Result<T> {
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
        file.take(MAX_DESCRIPTOR as u64 + 1).read_to_end(&mut raw)?;
        if raw.len() as u64 != info.len() || stamp(&file.metadata()?) != stamp(&info) {
            return Err(invalid());
        }
        checked(&raw)
    }

    fn parse(raw: &[u8]) -> io::Result<Self> {
        if raw.len() > MAX_DESCRIPTOR {
            return Err(invalid());
        }
        let mut raw = raw.strip_prefix(MAGIC).ok_or_else(invalid)?;
        let pool = fixed(&mut raw)?;
        let epoch = fixed(&mut raw)?;
        let salt = fixed(&mut raw)?;
        let tag = fixed(&mut raw)?;
        let [login_len] = fixed(&mut raw)?;
        let login = login_name(take(&mut raw, usize::from(login_len))?)?.to_owned();
        let [data_count] = fixed(&mut raw)?;
        let data_count = usize::from(data_count);
        if !(1..=MAX_DATA_ROOTS).contains(&data_count) {
            return Err(invalid());
        }
        let system = root_field(&mut raw)?;
        let recycle = root_field(&mut raw)?;
        let data = (0..data_count)
            .map(|_| root_field(&mut raw))
            .collect::<io::Result<Vec<_>>>()?;
        let roots = [&system, &recycle]
            .into_iter()
            .chain(&data)
            .collect::<Vec<_>>();
        if pool == [0; 16]
            || epoch == [0; 16]
            || pool == epoch
            || salt == [0; 16]
            || tag == [0; 32]
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
            salt,
            tag,
            login,
            roots: SelectedRoots {
                pool,
                system,
                recycle,
                data,
            },
        })
    }

    pub(crate) fn into_parts(self) -> (SelectedRoots, Account) {
        let account = Account {
            pool: self.pool,
            epoch: self.epoch,
            login: self.login,
            verifier: Verifier::new(self.salt, self.tag),
            open: AtomicBool::new(true),
            sessions: session::Sessions::default(),
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
    open: AtomicBool,
    sessions: session::Sessions,
}

pub(crate) struct Permit<'owner> {
    account: &'owner Account,
    pool: [u8; 16],
    epoch: [u8; 16],
    expires: Option<Instant>,
}

/// A live bearer, not a Basic admission or a capability to mutate storage.
pub(crate) struct BrowsePermit<'owner> {
    original: Permit<'owner>,
    expires: Instant,
}

impl<'owner> Permit<'owner> {
    pub(crate) fn browse(self) -> Result<BrowsePermit<'owner>, AuthError> {
        let expires = self.expires.ok_or(AuthError::Invalid)?;
        Ok(BrowsePermit {
            original: self,
            expires,
        })
    }
}

impl BrowsePermit<'_> {
    pub(crate) fn require(
        &self,
        account: &Account,
        stopping: &dyn Fn() -> bool,
        now: Instant,
    ) -> Result<(), AuthError> {
        let proved = &self.original;
        if !std::ptr::eq(account, proved.account)
            || account.pool != proved.pool
            || account.epoch != proved.epoch
        {
            return Err(AuthError::Unavailable);
        }
        account.require_open(stopping)?;
        if now >= self.expires {
            return Err(AuthError::Invalid);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AuthError {
    Invalid,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LoginError {
    Auth(AuthError),
    Throttled(u64),
}

impl Account {
    /// Authenticate one direct HTTP request before parsing its body. Preserve
    /// individual Authorization field occurrences for the duplicate check.
    pub(crate) fn authenticate(
        &self,
        headers: &[(&str, &str)],
        stopping: &dyn Fn() -> bool,
    ) -> Result<Permit<'_>, AuthError> {
        let stopped = || stopping() || !self.open.load(Ordering::Acquire);
        if stopped() {
            return Err(AuthError::Unavailable);
        }
        let credentials = credentials(headers)?;
        if credentials.login() != self.login.as_str() {
            return Err(AuthError::Invalid);
        }
        let matched = self
            .verifier
            .matches(credentials.password(), &stopped)
            .map_err(|_| AuthError::Unavailable)?;
        if stopped() {
            return Err(AuthError::Unavailable);
        }
        if !matched {
            return Err(AuthError::Invalid);
        }
        Ok(self.permit(None))
    }

    pub(crate) fn authenticate_bearer(
        &self,
        headers: &[(&str, &str)],
        stopping: &dyn Fn() -> bool,
    ) -> Result<Permit<'_>, AuthError> {
        self.require_open(stopping)?;
        let expires = self.sessions.prove(
            session::Scope(self.pool, self.epoch),
            Instant::now(),
            authorization(headers)?,
        )?;
        self.require_open(stopping)?;
        if Instant::now() >= expires {
            return Err(AuthError::Invalid);
        }
        Ok(self.permit(Some(expires)))
    }

    pub(crate) fn issue_session(
        &self,
        headers: &[(&str, &str)],
        stopping: &dyn Fn() -> bool,
    ) -> Result<String, LoginError> {
        self.require_open(stopping).map_err(LoginError::Auth)?;
        // One account, one bounded rolling admission window. It applies
        // before Argon2 even to malformed, unknown, or failed credentials.
        self.sessions.allow_login(Instant::now)?;
        self.authenticate(headers, stopping)
            .map_err(LoginError::Auth)?;
        let token = self
            .sessions
            .issue(session::Scope(self.pool, self.epoch))
            .map_err(LoginError::Auth)?;
        self.require_open(stopping).map_err(LoginError::Auth)?;
        Ok(token)
    }

    fn require_open(&self, stopping: &dyn Fn() -> bool) -> Result<(), AuthError> {
        if stopping() || !self.open.load(Ordering::Acquire) {
            return Err(AuthError::Unavailable);
        }
        Ok(())
    }

    fn permit(&self, expires: Option<Instant>) -> Permit<'_> {
        Permit {
            account: self,
            pool: self.pool,
            epoch: self.epoch,
            expires,
        }
    }

    /// Stop accepting/authorizing before drain. The offline Helper cannot
    /// change credentials allowed by a live, held deployment.
    pub(crate) fn stop_admission(&self) {
        self.open.store(false, Ordering::Release);
    }
}

impl Permit<'_> {
    /// Staged upload-only path. Invalid from this *already authenticated*
    /// check means exact Bearer expiry, not an arbitrary filesystem errno.
    #[cfg(test)]
    pub(crate) fn with_upload_admission<T>(
        self,
        account: &Account,
        now: &dyn Fn() -> Instant,
        stopping: &dyn Fn() -> bool,
        one_request: impl FnOnce(
            crate::container_store::upload::Scope,
            &dyn Fn() -> Result<(), AuthError>,
        ) -> T,
    ) -> Result<T, AuthError> {
        use crate::container_store::upload::Scope;

        let expires = self.expires.ok_or(AuthError::Invalid)?;
        let check = || {
            if !std::ptr::eq(account, self.account)
                || self.pool != account.pool
                || self.epoch != account.epoch
            {
                return Err(AuthError::Unavailable);
            }
            account.require_open(stopping)?;
            if now() >= expires {
                return Err(AuthError::Invalid);
            }
            Ok(())
        };
        check()?;
        Ok(one_request(
            Scope {
                pool: self.pool,
                epoch: self.epoch,
            },
            &check,
        ))
    }

    /// HTTP adapter runs the new typed dispatch here (exactly one complete
    /// request). Check the permit before inspecting Store, and pass the
    /// borrowed predicate into selection. The predicate never takes a Store
    /// lock, reads from the socket or authorizes any other Account/epoch.
    pub(crate) fn with_admission<T>(
        self,
        account: &Account,
        one_request: impl FnOnce(&dyn Fn() -> bool) -> T,
    ) -> Result<T, AuthError> {
        let allowed = || {
            std::ptr::eq(account, self.account)
                && self.pool == account.pool
                && self.epoch == account.epoch
                && account.open.load(Ordering::Acquire)
                && self.expires.is_none_or(|expires| Instant::now() < expires)
        };
        if self
            .expires
            .is_some_and(|expires| Instant::now() >= expires)
        {
            return Err(AuthError::Invalid);
        }
        if !allowed() {
            return Err(AuthError::Unavailable);
        }
        Ok(one_request(&allowed))
    }
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
fn authorization<'a>(headers: &[(&str, &'a str)]) -> Result<&'a str, AuthError> {
    let mut values = headers
        .iter()
        .filter_map(|(k, v)| k.eq_ignore_ascii_case("authorization").then_some(*v));
    let raw = values.next().ok_or(AuthError::Invalid)?;
    if values.next().is_some() {
        return Err(AuthError::Invalid);
    }
    Ok(raw)
}

fn credentials(headers: &[(&str, &str)]) -> Result<BasicCredentials, AuthError> {
    let decoded = String::from_utf8(decode_basic(authorization(headers)?)?)
        .map_err(|_| AuthError::Invalid)?;
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
    let mut groups = encoded.as_bytes().as_chunks::<4>().0.iter().peekable();
    let mut data = Vec::with_capacity(encoded.len() / 4 * 3);
    while let Some(q) = groups.next() {
        let a = digit(q[0])?;
        let b = digit(q[1])?;
        data.push(a << 2 | b >> 4);
        match (q[2], q[3]) {
            (b'=', b'=') if groups.peek().is_none() && b.trailing_zeros() >= 4 => {}
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

#[cfg(test)]
pub(crate) use tests::{FIXTURE_BASIC, fixture_account};
