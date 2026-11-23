//! Qualify the deployment locator and original protected site in the retained
//! historical worker. No Helper state or credential is consulted.
use super::{
    PRODUCT, Site, approval, bad, child, contents, layout, mark, public_record::Public,
    rust_site::PublicAuthority,
};
use crate::{
    pinned::private_inode_reader,
    smb_profile::rust_first::{MAX_WIRE, RustPlan},
    sqlite_install::pair::fresh_issuer::{SitePayload, compiled_schema, image},
};
use rustix::fs::{Mode, OFlags, ResolveFlags, openat2};
use std::{
    ffi::CStr,
    fs::File,
    io,
    os::unix::fs::{FileExt, MetadataExt},
};

const LOCATOR: &CStr = c"deployment.v1";
const MAGIC: &[u8; 16] = b"ZXS-DEPLOYED-V1\0";
fn unknown() -> io::Error {
    bad("the original independently managed ZXS deployment was not confirmed")
}
fn leaf(parent: &File, name: &CStr) -> io::Result<File> {
    Ok(File::from(openat2(
        parent,
        name,
        OFlags::PATH | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
        ResolveFlags::BENEATH
            | ResolveFlags::NO_XDEV
            | ResolveFlags::NO_MAGICLINKS
            | ResolveFlags::NO_SYMLINKS,
    )?))
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SelectedSite {
    instance: [u8; 16],
    sha: [u8; 32],
}
struct Selector {
    pin: File,
    value: SelectedSite,
}

fn locate(fd: &File) -> io::Result<SelectedSite> {
    let mut v = [0; 64];
    fd.read_exact_at(&mut v, 0)?;
    if fd.read_at(&mut [0], 64)? != 0 {
        return Err(unknown());
    }
    decode_locator(&v)
}
fn decode_locator(v: &[u8]) -> io::Result<SelectedSite> {
    if v.len() != 64 || &v[..16] != MAGIC || v[16..32] == [0; 16] || v[32..] == [0; 32] {
        return Err(unknown());
    }
    Ok(SelectedSite {
        instance: v[16..32].try_into().expect("fixed site identity"),
        sha: v[32..].try_into().expect("fixed site digest"),
    })
}
fn locator(parent: &File) -> io::Result<Selector> {
    let pin = leaf(parent, LOCATOR)?;
    let m = pin.metadata()?;
    if m.mode() != 0o100_600 || m.uid() != 0 || m.gid() != 0 || m.nlink() != 1 || m.len() != 64 {
        return Err(unknown());
    }
    layout::exact_with(parent, LOCATOR, &pin, || leaf(parent, LOCATOR))?;
    let stamp = super::live::Stamp::on(&pin)?;
    let reader = private_inode_reader(&pin, 0, mark(parent)?.1)?;
    layout::bare_acl(&reader, c"system.posix_acl_access")?;
    let value = locate(&reader)?;
    if super::live::Stamp::on(&reader)? != stamp
        || super::live::Stamp::on(&leaf(parent, LOCATOR)?)? != stamp
    {
        return Err(unknown());
    }
    Ok(Selector { pin, value })
}

fn qualify_payload(bytes: &[u8], plan: &RustPlan, site_sha: [u8; 32]) -> io::Result<SitePayload> {
    let payload = SitePayload::decode(bytes)?;
    if payload.origin != plan.origin
        || payload.plan_sha != plan.sha()
        || payload.sha()? != site_sha
        || bytes != payload.encode()?
    {
        return Err(unknown());
    }
    // The parent verifies the actual approved ELF. A credential worker
    // accepts only the chosen path inside these same protected site bytes:
    // it neither selects another executable nor opens the image body.
    image::check_path(&payload.image)?;
    Ok(payload)
}

/// Qualify the original installed origin in a private worker before
/// lowering, without opening the vault or acquiring a second lock.
pub(in crate::sqlite_install::pair) fn deployed_worker_payload(
    plan: &RustPlan,
    site_sha: [u8; 32],
) -> io::Result<SitePayload> {
    compiled_schema()?;
    if site_sha == [0; 32] {
        return Err(unknown());
    }
    let expected = SelectedSite {
        instance: plan.origin.instance,
        sha: site_sha,
    };
    let site = Site::product(approval::host_admin()?, &expected.instance, false)?;
    let parent = child(&site.chain[PRODUCT.len() - 1], c".")?;
    layout::protected_control(&parent)?;
    let selector = locator(&parent)?;
    if selector.value != expected {
        return Err(unknown());
    }
    let dir = child(site.base(), c".")?;
    if mark(&dir)? != mark(site.base())? {
        return Err(unknown());
    }
    layout::bare_acl(&dir, c"system.posix_acl_default")?;
    contents(&dir, &[b"site.v1", b"rust.install", b"vault"])?;
    let auth = PublicAuthority::select(&dir)?;
    let payload = qualify_payload(auth.bytes(), plan, site_sha)?;
    layout::verify_registered_instance(&site, payload.subject.tuple().2)?;
    let installed = Public::select(&dir, c"rust.install", 1..=MAX_WIRE)?;
    if installed.bytes() != plan.wire() {
        return Err(unknown());
    }
    installed.require(&dir, c"rust.install")?;
    auth.require(&dir)?;
    let named = locator(&parent)?;
    if named.value != expected
        || super::live::Stamp::on(&named.pin)? != super::live::Stamp::on(&selector.pin)?
    {
        return Err(unknown());
    }
    site.check()?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    #[test]
    fn reattach_only_the_exact_complete_nonzero_zxs_selector() {
        let mut bytes = [0; 64];
        bytes[..16].copy_from_slice(MAGIC);
        bytes[16..32].fill(1);
        bytes[32..].fill(2);
        assert_eq!(
            decode_locator(&bytes).unwrap(),
            SelectedSite {
                instance: [1; 16],
                sha: [2; 32]
            }
        );
        assert!(decode_locator(&bytes[..63]).is_err());
        assert!(decode_locator(&[bytes.as_slice(), b"x"].concat()).is_err());
        bytes[32..].fill(0);
        assert!(decode_locator(&bytes).is_err());
    }

    #[test]
    fn reattach_and_shared_account_workers_keep_only_the_exact_image_from_the_protected_site() {
        let text = include_str!("../../../../../../tests/smb-host/rust-install-v2.txt");
        let hex = text
            .lines()
            .find_map(|s| s.strip_prefix("wire_hex="))
            .unwrap();
        let raw = hex
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
            .collect::<Vec<_>>();
        let plan = RustPlan::decode(&raw).unwrap();
        let subject =
            crate::sqlite_install::pair::ExplicitSubject::checked([71; 16], 1001, 1002).unwrap();
        let mut payload = SitePayload::new(
            &plan,
            subject,
            Path::new("/synthetic/S"),
            Path::new("/protected/selected/bin/zxs"),
            [72; 32],
        )
        .unwrap();
        let (body, approved) = (payload.encode().unwrap(), payload.sha().unwrap());
        assert!(qualify_payload(&body, &plan, approved).unwrap() == payload);
        for candidate in [
            "/zxs/bin/zxs",
            "/alternate/bin/zxs",
            "/protected/selected/bin/other",
        ] {
            payload.image = candidate.into();
            let changed = payload.encode().unwrap();
            assert!(qualify_payload(&changed, &plan, approved).is_err());
        }
        // Even an independently named site digest cannot approve a file
        // outside the fixed leaf grammar.
        assert!(
            qualify_payload(&payload.encode().unwrap(), &plan, payload.sha().unwrap()).is_err()
        );
        payload.image = "/protected/selected/bin/zxs".into();
        payload.elf_sha[0] ^= 1;
        assert!(qualify_payload(&payload.encode().unwrap(), &plan, approved).is_err());
    }
}
