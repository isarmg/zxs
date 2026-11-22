//! The approved first-install plan contains names and addresses, not paths,
//! credentials, service readiness, or authority to initialise another pool.
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, net::{IpAddr, Ipv4Addr, Ipv6Addr}};

pub(crate) type Id = [u8; 16];
pub(crate) const MAX_ENDPOINTS: usize = 16;
pub(crate) const MAX_FOLDERS: usize = 62;
const MAX_NAME: usize = 255;
pub(crate) const MAX_WIRE: usize = 16 + 5 * 16 + 1 + MAX_ENDPOINTS * 18
    + 1 + MAX_FOLDERS * (16 + 1 + 2 + MAX_NAME) + 2 + MAX_NAME;
const _: () = assert!(MAX_WIRE == 17_631);
type Result<T> = std::result::Result<T, &'static str>;
const BAD: &str = "not one canonical first SMB plan for this original installation";

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct Endpoint { ip: IpAddr, prefix: u8 }
impl Endpoint {
    fn checked(ip: IpAddr, prefix: u8) -> Result<Self> {
        match ip {
            IpAddr::V4(a) if a.octets()[0] != 0 && !a.is_loopback() && !a.is_multicast() && !a.is_broadcast()
                && (1..=32).contains(&prefix) => {
                if prefix < 31 {
                    let raw = u32::from(a);
                    let host = u32::MAX >> prefix;
                    if raw & host == 0 || raw & host == host { return Err(BAD); }
                }
            }
            IpAddr::V6(a) if !a.is_unspecified() && !a.is_loopback() && !a.is_multicast() && !a.is_unicast_link_local()
                && a.to_ipv4().is_none() && (1..=128).contains(&prefix) => {}
            _ => return Err(BAD),
        }
        Ok(Self { ip, prefix })
    }
    pub(crate) fn named(text: &str) -> Result<Self> {
        let (address, bits) = text.split_once('/').ok_or(BAD)?;
        let ip: IpAddr = address.parse().map_err(|_| BAD)?;
        let prefix: u8 = bits.parse().map_err(|_| BAD)?;
        if ip.to_string() != address || prefix.to_string() != bits { return Err(BAD); }
        Self::checked(ip, prefix) // no wildcard, loopback, ambiguous zone, or RFC1918-only rule
    }
    pub(crate) fn address(self) -> IpAddr { self.ip }
    pub(crate) fn prefix(self) -> u8 { self.prefix }
    fn emit(self, out: &mut Vec<u8>) {
        match self.ip {
            IpAddr::V4(a) => { out.extend([4, self.prefix]); out.extend(a.octets()); }
            IpAddr::V6(a) => { out.extend([6, self.prefix]); out.extend(a.octets()); }
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Mode { ReadOnly, ReadWrite }
impl Mode {
    pub(crate) fn code(self) -> u8 { match self { Self::ReadOnly => 1, Self::ReadWrite => 2 } }
    fn parse(value: u8) -> Result<Self> {
        match value { 1 => Ok(Self::ReadOnly), 2 => Ok(Self::ReadWrite), _ => Err(BAD) }
    }
}
fn name(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_NAME && s != "." && s != ".." && !s.ends_with([' ', '.'])
        && !s.chars().any(|c| c.is_control() || matches!(c, '"' | '\\' | '/' | ':' | '|' | '<' | '>' | '*' | '?'))
}
/// One explicitly named logical route. It is neither a volume selector nor
/// an independent `ShareId`, operation, or login.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecycleName(String);
impl RecycleName {
    pub(crate) fn new(label: String) -> Result<Self> {
        if name(&label) { Ok(Self(label)) } else { Err(BAD) }
    }
    pub(crate) fn as_str(&self) -> &str { &self.0 }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Folder { pub(crate) volume: Id, pub(crate) name: String, pub(crate) mode: Mode }
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FolderIds { pub(crate) share: Id, pub(crate) publish: Id, pub(crate) setting: Id }
fn derive(domain:&[u8],sha: &[u8; 32], kind: u8, volume: &Id) -> Id {
    let mut h = Sha256::new();
    h.update(domain);
    h.update(sha);
    h.update([kind]);
    h.update(volume); // stable identity of the explicitly selected ordinary volume
    h.finalize()[..16].try_into().expect("16 bytes")
}
fn send_name(out: &mut Vec<u8>, s: &str) {
    out.extend((s.len() as u16).to_be_bytes());
    out.extend_from_slice(s.as_bytes());
}

/// The common suffix is only syntax and chosen names/IDs; it is not an
/// installation authority or a credential-family selector.
pub(super) fn sort_selection(ids:[Id;5],endpoints:&mut[Endpoint],chosen:&mut[Folder],recycle:&RecycleName)->Result<BTreeSet<Id>>{
    let mut done = BTreeSet::new();
    if !ids.into_iter().all(|p| p != [0; 16] && done.insert(p))
        || !(1..=MAX_ENDPOINTS).contains(&endpoints.len()) || !(1..=MAX_FOLDERS).contains(&chosen.len()) { return Err(BAD); }
    endpoints.sort_unstable();
    if endpoints.windows(2).any(|p| p[0].address() == p[1].address()) { return Err(BAD); }
    chosen.sort_unstable_by_key(|f| f.volume);
    {
        let mut names = BTreeSet::from([recycle.as_str()]);
        for f in chosen.iter() {
            if f.volume == [0; 16] || !done.insert(f.volume) || !name(&f.name) || !names.insert(f.name.as_str()) {
                return Err(BAD);
            }
        }
    } // Case remains exact; these are real ZXS folder names.
    Ok(done)
}
pub(super) fn emit_layout(wire:&mut Vec<u8>,endpoints:&[Endpoint],chosen:&[Folder],recycle:&RecycleName){
    wire.push(endpoints.len() as u8);
    for &e in endpoints { e.emit(wire); }
    wire.push(chosen.len() as u8);
    for f in chosen {
        wire.extend(f.volume);
        wire.push(f.mode.code());
        send_name(wire, &f.name);
    }
    // Only the exact directory name, never a physical R path or ID.
    send_name(wire, recycle.as_str());
}

pub(super) fn folder_ids(domain:&[u8],sha:&[u8;32],chosen:Vec<Folder>,mut done:BTreeSet<Id>)->Result<Vec<(Folder,FolderIds)>>{
    let mut next = |kind, vol: &Id| {
        let id = derive(domain, sha, kind, vol);
        if id == [0; 16] || !done.insert(id) { Err(BAD) } else { Ok(id) }
    };
    let mut folders = Vec::with_capacity(chosen.len());
    for f in chosen {
        let v = f.volume;
        folders.push((f, FolderIds { share: next(1, &v)?, publish: next(2, &v)?, setting: next(3, &v)? }));
    }
    Ok(folders)
}

pub(super) fn take<'a>(p: &mut &'a [u8], n: usize) -> Result<&'a [u8]> {
    let (a, b) = p.split_at_checked(n).ok_or(BAD)?;
    *p = b;
    Ok(a)
}
pub(super) fn fixed<const N: usize>(p: &mut &[u8]) -> Result<[u8; N]> { Ok(take(p, N)?.try_into().expect("exact length")) }
fn label(p: &mut &[u8]) -> Result<String> {
    let n = u16::from_be_bytes(fixed(p)?) as usize;
    if n == 0 || n > MAX_NAME { return Err(BAD); }
    Ok(std::str::from_utf8(take(p, n)?).map_err(|_| BAD)?.to_owned())
}

pub(super) fn decode_layout(p:&mut &[u8])->Result<(Vec<Endpoint>,Vec<Folder>,RecycleName)>{
    let [ep] = fixed(p)?;
    if !(1..=MAX_ENDPOINTS).contains(&(ep as usize)) { return Err(BAD); }
    let mut endpoints = Vec::with_capacity(ep as usize);
    for _ in 0..ep {
        let [family, prefix] = fixed(p)?;
        let ip = match family {
            4 => IpAddr::V4(Ipv4Addr::from(fixed::<4>(p)?)),
            6 => IpAddr::V6(Ipv6Addr::from(fixed::<16>(p)?)),
            _ => return Err(BAD),
        };
        endpoints.push(Endpoint::checked(ip, prefix)?);
    }
    let [n] = fixed(p)?;
    if !(1..=MAX_FOLDERS).contains(&(n as usize)) { return Err(BAD); }
    let mut folders = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let v = fixed(p)?;
        let [mode] = fixed(p)?;
        folders.push(Folder { volume: v, mode: Mode::parse(mode)?, name: label(p)? });
    }
    let recycle = RecycleName::new(label(p)?)?;
    Ok((endpoints,folders,recycle))
}
