//! A read-only, one-session owner for the ZXS deployment locator and the
//! authoritative existing site. No Helper state or credential is consulted.
use super::{
    PRODUCT, Site, approval, bad, child, contents, layout, lease, mark, product_pin,
    public_record::Public,
    rust_site::{PublicAuthority, VaultStamp, checked_vault_stamp},
    shared_lease, state_pins,
};
use crate::{
    pinned::{Kind, Pinned, private_inode_reader},
    smb_profile::rust_first::{MAX_WIRE, RustPlan},
    sqlite_install::{
        pair::fresh_issuer::{SitePayload, compiled_schema, image},
        product_layout,
    },
};
use rustix::fs::{FlockOperation, Mode, OFlags, ResolveFlags, flock, openat2};
use sha2::{Digest, Sha256};
use std::{
    ffi::CStr,
    fs::File,
    io,
    os::unix::fs::{FileExt, MetadataExt},
    path::Path,
};

const LOCATOR: &CStr = c"deployment.v1";
const VAULT: &CStr = c"vault";
use crate::sqlite_install::pair::native_sql::rust_vault::VAULT_NAME as RECORD;
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

#[must_use]
pub(in crate::sqlite_install::pair) struct DeployedSite {
    parent: File,
    selector: Selector,
    original: Site,
    site_ex: File,
    system: Pinned,
    system_ex: File,
    authority: PublicAuthority,
    plan_file: Public,
    payload: SitePayload,
    plan: RustPlan,
    verified_image: image::Image,
}
impl DeployedSite {
    pub(in crate::sqlite_install::pair) fn select() -> io::Result<Self> {
        Self::select_with_system(|path| Pinned::already_mounted(path, Kind::StorageRoot))
    }
    pub(in crate::sqlite_install::pair) fn select_for_account_reset() -> io::Result<Self> {
        Self::select_with_system(|path| {
            let scope = std::sync::Arc::new(crate::maintenance::HostMounts::discovery_view(false)?);
            Pinned::require_local_root(&scope)?;
            Pinned::local_storage(path, &scope)
        })
    }
    fn select_with_system(find: impl FnOnce(&Path) -> io::Result<Pinned>) -> io::Result<Self> {
        compiled_schema()?;
        approval::no_inherited_descriptors()?;
        approval::same_terminal(&io::stdin(), &io::stderr())?;
        let host = approval::host_admin()?;
        let [top, base] = state_pins(&host)?;
        let parent = child(&base, c".")?;
        if mark(&base)? != mark(&parent)? {
            return Err(unknown());
        }
        layout::protected_control(&parent)?;
        flock(&parent, FlockOperation::NonBlockingLockShared)?;
        let selector = locator(&parent)?;
        let original = Site::product(host, &selector.value.instance, false)?;
        if mark(&original.chain[0])? != mark(&top)?
            || mark(&original.chain[PRODUCT.len() - 1])? != mark(&parent)?
        {
            return Err(unknown());
        }
        let site_ex = child(original.base(), c".")?;
        let authority = PublicAuthority::select(&site_ex)?;
        let payload = SitePayload::decode(authority.bytes())?;
        if payload.origin.instance != selector.value.instance
            || <[u8; 32]>::from(Sha256::digest(authority.bytes())) != selector.value.sha
        {
            return Err(unknown());
        }
        layout::verify_registered_instance(&original, payload.subject.tuple().2)?;
        let plan_file = Public::select(&site_ex, c"rust.install", 1..=MAX_WIRE)?;
        let plan = RustPlan::decode(plan_file.bytes()).map_err(bad)?;
        if plan.origin != payload.origin || plan.sha() != payload.plan_sha {
            return Err(unknown());
        }
        let product = mark(&original.chain[0])?.0;
        let verified_image = image::Image::select(&payload.image, &payload.elf_sha, product)?;
        let system = find(&payload.system)?;
        product_layout::require_original_media(product, [&system])?;
        let system_ex = system.storage_directory()?;
        // Same kernel order as the only original cold supervisor. An online
        // participant holds S SH, in which case this check cannot qualify.
        flock(&system_ex, FlockOperation::NonBlockingLockExclusive)?;
        flock(&site_ex, FlockOperation::NonBlockingLockExclusive)?;
        let result = Self {
            parent,
            selector,
            original,
            site_ex,
            system,
            system_ex,
            authority,
            plan_file,
            payload,
            plan,
            verified_image,
        };
        result.require()?;
        Ok(result)
    }
    pub(in crate::sqlite_install::pair) fn require(&self) -> io::Result<()> {
        compiled_schema()?;
        approval::same_terminal(&io::stdin(), &io::stderr())?;
        let (_, uid, gid) = self.payload.subject.tuple();
        layout::protected_control(&self.parent)?;
        layout::verify_registered_instance(&self.original, gid)?;
        product_layout::require_original_media(mark(&self.original.chain[0])?.0, [&self.system])?;
        shared_lease(&self.parent, mark(&self.original.chain[PRODUCT.len() - 1])?)?;
        let s = self.system.require_same_named_mount()?;
        if !self.system.passive_mount_observation()
            || self.system.metadata()?.uid() != uid
            || self.system.metadata()?.mode() != 0o040_700
            || self.payload.system.as_path() != self.system.path()
        {
            return Err(unknown());
        }
        lease(&self.system_ex, (self.system.entity(), s.id))?;
        lease(&self.site_ex, mark(self.original.base())?)?;
        layout::bare_acl(&self.site_ex, c"system.posix_acl_default")?;
        // Both names are retained even if a future product pair uses one
        // physical backing; never let an origin be inside the other role.
        if self
            .system
            .visible_chain()
            .contains(&mark(self.original.base())?.0)
            || mark(&self.original.host)?.0 == self.system.entity()
        {
            return Err(unknown());
        }
        for path in &self.original.chain {
            if mark(path)?.0 == self.system.entity() {
                return Err(unknown());
            }
        }
        if self.verified_image.includes(self.system.entity())
            || self
                .system
                .visible_chain()
                .contains(&self.verified_image.entity())
            || self.verified_image.includes(mark(&self.parent)?.0)
        {
            return Err(unknown());
        }
        let located = locator(&self.parent)?;
        if located.value != self.selector.value
            || super::live::Stamp::on(&located.pin)? != super::live::Stamp::on(&self.selector.pin)?
        {
            return Err(unknown());
        }
        contents(&self.site_ex, &[b"rust.install", b"site.v1", b"vault"])?;
        self.authority.require(&self.site_ex)?;
        self.plan_file.require(&self.site_ex, c"rust.install")?;
        self.verified_image.require()
    }
    pub(in crate::sqlite_install::pair) fn original(&self) -> (&Pinned, &File) {
        (&self.system, &self.system_ex)
    }
    pub(in crate::sqlite_install::pair) fn payload(&self) -> &SitePayload {
        &self.payload
    }
    pub(in crate::sqlite_install::pair) fn plan(&self) -> &RustPlan {
        &self.plan
    }
    pub(in crate::sqlite_install::pair) fn site_sha(&self) -> [u8; 32] {
        self.selector.value.sha
    }
    pub(in crate::sqlite_install::pair) fn require_vault_prefix(
        &self,
        wanted: &[u8; 224],
    ) -> io::Result<()> {
        self.observe_vault(wanted).map(drop)
    }
    pub(super) fn offline_vault_directory(&self) -> io::Result<File> {
        self.require()?;
        let pin = || product_pin(&self.site_ex, VAULT);
        let vault = pin()?;
        let m = vault.metadata()?;
        if m.mode() != 0o040_700 || m.uid() != 0 || m.gid() != 0 {
            return Err(unknown());
        }
        let dir = child(&vault, c".")?;
        if mark(&dir)? != mark(&vault)? || mark(&dir)?.1 != mark(&self.site_ex)?.1 {
            return Err(unknown());
        }
        layout::exact_with(&self.site_ex, VAULT, &vault, pin)?;
        layout::bare_acl(&dir, c"system.posix_acl_access")?;
        layout::bare_acl(&dir, c"system.posix_acl_default")?;
        self.require()?;
        Ok(dir)
    }
    pub(in crate::sqlite_install::pair) fn observe_vault(
        &self,
        wanted: &[u8; 224],
    ) -> io::Result<ObservedVault<'_>> {
        let dir = self.offline_vault_directory()?;
        contents(&dir, &[RECORD.to_bytes()])?;
        let original = leaf(&dir, RECORD)?;
        let mount = mark(&dir)?.1;
        let stamp = checked_vault_stamp(&original, mount)?;
        let read = private_inode_reader(&original, 0, mount)?;
        layout::bare_acl(&read, c"system.posix_acl_access")?;
        let mut prefix = [0; 224];
        read.read_exact_at(&mut prefix, 0)?; // never the 16-byte credential
        if &prefix != wanted
            || checked_vault_stamp(&read, mount)? != stamp
            || checked_vault_stamp(&leaf(&dir, RECORD)?, mount)? != stamp
            || mark(&product_pin(&self.site_ex, VAULT)?)? != mark(&dir)?
        {
            return Err(unknown());
        }
        // Retain only the original non-readable pin, never the prefix reader.
        drop(read);
        let witness = ObservedVault {
            site: self,
            dir,
            original,
            stamp,
            mount,
        };
        witness.require()?;
        Ok(witness)
    }
}
pub(in crate::sqlite_install::pair) struct ObservedVault<'a> {
    site: &'a DeployedSite,
    dir: File,
    original: File,
    stamp: VaultStamp,
    mount: u64,
}
impl ObservedVault<'_> {
    pub(in crate::sqlite_install::pair) fn require(&self) -> io::Result<()> {
        self.site.require()?;
        let parent = mark(&self.dir)?;
        if parent.1 != self.mount || mark(&product_pin(&self.site.site_ex, VAULT)?)? != parent {
            return Err(unknown());
        }
        contents(&self.dir, &[RECORD.to_bytes()])?;
        layout::bare_acl(&self.dir, c"system.posix_acl_access")?;
        layout::bare_acl(&self.dir, c"system.posix_acl_default")?;
        if checked_vault_stamp(&self.original, self.mount)? != self.stamp
            || checked_vault_stamp(&leaf(&self.dir, RECORD)?, self.mount)? != self.stamp
        {
            return Err(unknown());
        }
        self.site.require()
    }
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
