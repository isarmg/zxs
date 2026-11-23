//! Private one-shot proof for a fresh standalone ZXS deployment. It is not
//! an installer policy, online route, user credential, or persisted cache.
mod db;
mod wire;
use super::bad;
pub(super) use db::InitialDatabase;
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::ffi::OsStr;
use std::{ffi::OsString, io};
pub(super) use wire::{CredentialScope, CryptoPins, MAX_SITE, SitePayload};

const SCHEMA_SHA: &str = "4d54de32c0004f7b5c0ed6af44a556d8add56517518cf7d8b0a5fe8bb158a0f4";
pub(in crate::sqlite_install) fn compiled_schema() -> io::Result<()> {
    let wanted = crate::fs_tools::hex(SCHEMA_SHA).expect("frozen v21 DDL hash");
    let actual: [u8; 32] = Sha256::digest(crate::sqlite_install::stage::SCHEMA.as_bytes()).into();
    if crate::sqlite_install::catalog::SCHEMA_VERSION != 21 || actual != wanted {
        return Err(bad(
            "the private fresh ZXS issuer is not qualified for this exact v21 format",
        ));
    }
    Ok(())
}
pub(in crate::sqlite_install) fn require_private_install() -> io::Result<()> {
    if !cfg!(feature = "private-fresh-install") {
        return Err(bad("private fresh ZXS issuance is disabled"));
    }
    compiled_schema()
}
type Id = [u8; 16];
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::sqlite_install) struct ExplicitSubject {
    actor: Id,
    uid: u32,
    gid: u32,
}
impl ExplicitSubject {
    pub(in crate::sqlite_install) fn checked(actor: Id, uid: u32, gid: u32) -> io::Result<Self> {
        if actor == [0; 16]
            || [uid, gid]
                .iter()
                .any(|n| matches!(*n, 0 | 65534 | u32::MAX))
        {
            return Err(bad(
                "expected the approved nonroot storage subject and original INIT actor",
            ));
        }
        Ok(Self { actor, uid, gid })
    }
    pub(in crate::sqlite_install) fn parse(
        it: &mut impl Iterator<Item = OsString>,
    ) -> Result<Self, &'static str> {
        use super::command::{key, value};
        fn who(it: &mut impl Iterator<Item = OsString>, key: &str) -> Result<u32, &'static str> {
            let os = value(it, key)?;
            let s = os
                .to_str()
                .ok_or("expected a canonical numeric storage credential")?;
            s.parse::<u32>()
                .ok()
                .filter(|v| !matches!(*v, 0 | 65534 | u32::MAX) && v.to_string() == s)
                .ok_or("expected a canonical nonroot numeric storage UID/GID")
        }
        let actor = key(it, "--init-actor")?;
        let uid = who(it, "--storage-uid")?;
        let gid = who(it, "--storage-gid")?;
        Self::checked(actor, uid, gid).map_err(|_| "wrong original explicit storage subject")
    }
    pub(in crate::sqlite_install) fn tuple(self) -> (Id, u32, u32) {
        (self.actor, self.uid, self.gid)
    }
}

pub(super) mod image;
#[cfg(test)]
use super::command::{key, value};
#[cfg(test)]
use super::terminal::PublicConfirmation;
#[cfg(test)]
use super::{
    command::{name, sha},
    native_run::Choice,
    native_sql::rust_vault::shared_login,
};
#[cfg(test)]
use crate::{
    smb_profile::{
        first::{MAX_FOLDERS, Mode, RecycleName},
        rust_first::{self, Origin},
    },
    sqlite_install::product_layout,
};
#[cfg(test)]
use helper_linux_evidence::prompt_signals::PromptSignals;
#[cfg(feature = "embedded-zxs")]
pub(crate) use image::EmbeddedInstall;
#[cfg(test)]
use std::path::PathBuf;

// Before and after password prompts only. Do not leave process-wide
// handlers or a blocked signal mask across the next independent TTY guard.
#[cfg(test)]
fn filesystem_phase<T>(
    live: &dyn Fn() -> io::Result<()>,
    phase: impl FnOnce(&dyn Fn() -> io::Result<()>) -> io::Result<T>,
) -> io::Result<T> {
    let signals = PromptSignals::begin()?;
    let guarded = || {
        signals.check()?;
        live()
    };
    guarded()?;
    let value = phase(&guarded);
    let last = guarded();
    signals.finish()?;
    last?;
    value
}

/// Historical parser fixture. It does not open or mutate an installation.
#[cfg(test)]
#[derive(Debug)]
pub(super) struct Selection {
    system: PathBuf,
    image_path: PathBuf,
    choices: Vec<Choice>,
}
#[cfg(test)]
impl Selection {
    pub(super) fn parse(mut it: impl Iterator<Item = OsString>) -> Result<Self, &'static str> {
        ExplicitSubject::parse(&mut it)?;
        let mut it = it.peekable();
        let system = if it.peek().is_some_and(|s| s == "--system") {
            PathBuf::from(value(&mut it, "--system")?)
        } else {
            PathBuf::from(product_layout::SYSTEM)
        };
        product_layout::media(&system)?;
        if system.to_str().is_none_or(|s| s.len() > 4095) {
            return Err("SYSTEM must be an exact protected UTF-8 path");
        }
        let origin = Origin {
            pool: key(&mut it, "--pool")?,
            init: key(&mut it, "--init")?,
            instance: key(&mut it, "--instance")?,
            bind: key(&mut it, "--bind")?,
            setup: key(&mut it, "--setup-op")?,
        };
        let ids = [
            origin.pool,
            origin.init,
            origin.instance,
            origin.bind,
            origin.setup,
        ];
        if ids.iter().enumerate().any(|(i, n)| ids[..i].contains(n)) {
            return Err("fresh installation identities must be distinct");
        }
        sha(&mut it, "--init-sha256")?;
        key(&mut it, "--approval")?;
        let image_path = if it.peek().is_some_and(|s| s == "--zxs-image") {
            PathBuf::from(value(&mut it, "--zxs-image")?)
        } else {
            PathBuf::from(product_layout::IMAGE)
        };
        image::check_path(&image_path).map_err(
            |_| "choose the protected canonical absolute path to the fixed zxs executable",
        )?;
        if system.starts_with(&image_path) || image_path.starts_with(&system) {
            return Err("SYSTEM must not contain the executable");
        }
        sha(&mut it, "--zxs-sha256")?;
        sha(&mut it, "--helper-sha256")?;
        sha(&mut it, "--gcrypt-sha256")?;
        sha(&mut it, "--gpgerror-sha256")?;
        value(&mut it, "--host")?
            .into_string()
            .ok()
            .filter(|v| rust_first::valid_host(v))
            .ok_or("expected the canonical lower ASCII installation hostname")?;
        let login = name(&mut it, "--login")?;
        if !shared_login(&login) {
            return Err("expected one canonical non-guest login for both Web and SMB");
        }
        RecycleName::new(
            value(&mut it, "--recycle-name")?
                .into_string()
                .map_err(|_| "expected a valid logical recycle name")?,
        )?;
        let mut folders: Vec<Choice> = Vec::new();
        while let Some(flag) = it.next() {
            if (flag == "--share" || flag == "--share-label") && folders.len() < MAX_FOLDERS {
                let selected = it
                    .next()
                    .ok_or("missing actual original data root or explicit DATA label")?;
                let p = if flag == "--share-label" {
                    product_layout::data_label(selected)?
                } else {
                    PathBuf::from(selected)
                };
                product_layout::media(&p)?;
                if system.starts_with(&p)
                    || p.starts_with(&system)
                    || image_path.starts_with(&p)
                    || p.starts_with(&image_path)
                    || folders
                        .iter()
                        .any(|f| p.starts_with(&f.path) || f.path.starts_with(&p))
                {
                    return Err("SYSTEM, ordinary media and the selected image must not overlap");
                }
                it.next()
                    .and_then(|s| s.into_string().ok())
                    .ok_or("missing exact UTF-8 folder name")?;
                match it.next().as_deref().and_then(OsStr::to_str) {
                    Some("ro") => Mode::ReadOnly,
                    Some("rw") => Mode::ReadWrite,
                    _ => return Err("select the explicit folder mode ro or rw"),
                };
                folders.push(Choice { path: p });
            } else {
                return Err(
                    "only explicit --share/--share-label entries; no recovery, password or second executable",
                );
            }
        }
        if folders.is_empty() {
            return Err("select at least one original ordinary folder");
        }
        Ok(Self {
            system,
            image_path,
            choices: folders,
        })
    }
}
fn hex(bytes: &[u8]) -> String {
    crate::fs_tools::lower_hex(bytes)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn arg(s: &str) -> impl Iterator<Item = OsString> + '_ {
        s.split_ascii_whitespace().map(OsString::from)
    }
    #[test]
    fn fresh_deployment_issue_parser_cannot_select_an_old_action_or_credential_path() {
        let form = format!(
            "--init-actor {} --storage-uid 1001 --storage-gid 1002 --system /lab/S --pool {} --init {} \
            --instance {} --bind {} --setup-op {} --init-sha256 {} --approval {} --zxs-image /fixture/bin/zxs --zxs-sha256 {} --helper-sha256 {} --gcrypt-sha256 {} \
            --gpgerror-sha256 {} --host fresh.test --login first.zxs --recycle-name Recycle \
            --share /lab/F Work rw",
            hex(&[31; 16]),
            hex(&[1; 16]),
            hex(&[2; 16]),
            hex(&[3; 16]),
            hex(&[4; 16]),
            hex(&[5; 16]),
            hex(&[45; 32]),
            hex(&[6; 16]),
            hex(&[41; 32]),
            hex(&[42; 32]),
            hex(&[43; 32]),
            hex(&[44; 32])
        );
        let selected = Selection::parse(arg(&form)).unwrap();
        assert_eq!(selected.image_path, PathBuf::from("/fixture/bin/zxs"));
        assert_eq!(selected.system, PathBuf::from("/lab/S"));
        let defaults = form
            .replace("--system /lab/S ", "")
            .replace("--zxs-image /fixture/bin/zxs ", "")
            .replace("--share /lab/F", "--share-label 工具");
        let selected = Selection::parse(arg(&defaults)).unwrap();
        assert_eq!(selected.system, PathBuf::from(product_layout::SYSTEM));
        assert_eq!(selected.image_path, PathBuf::from(product_layout::IMAGE));
        assert_eq!(
            selected.choices[0].path,
            PathBuf::from("/zxs/disks/data/工具")
        );
        for bad in [
            format!("{form} --resume"),
            format!("{form} --password bogus"),
            format!("{form} --site /other"),
            form.replace("first.zxs", "guest"),
            form.replace(
                "--login first.zxs",
                "--web-login first.web --smb-login first.smb",
            ),
            form.replace("/lab/F", "/lab/S/F"),
            form.replace("1001", "01001"),
            form.replace(&hex(&[3; 16]), &hex(&[2; 16])),
            format!("{form} --listen 192.0.2.17/24"),
            form.replace("--share /lab/F Work rw", ""),
            defaults.replace(&format!("--zxs-sha256 {} ", hex(&[41; 32])), ""),
            defaults.replace("--share-label 工具", "--share-label .."),
            form.replace("/fixture/bin/zxs", "/lab/S/zxs"),
            form.replace("/fixture/bin/zxs", "/zxs/state/zxs"),
            form.replace("/fixture/bin/zxs", "/lab/F/zxs"),
            form.replace("/fixture/bin/zxs", "/fixture/bin/other"),
            form.replace("/fixture/bin/zxs", "zxs"),
            form.replace(&hex(&[41; 32]), &hex(&[0; 32])),
        ] {
            assert!(Selection::parse(arg(&bad)).is_err());
        }
        let public = format!("issue-fresh {form}");
        let through_web = format!("web issue-fresh {form}");
        assert_eq!(
            super::super::command::Setup::parse(arg(&through_web)).is_ok(),
            cfg!(feature = "private-fresh-install")
        );
        for bad in [
            "web web issue-fresh",
            "web reset-account",
            "web",
            "web issue-fresh --password hidden",
        ] {
            assert!(super::super::command::Setup::parse(arg(bad)).is_err());
        }
        let retired = format!("issue-fresh-zxs-v18 {form}");
        assert!(super::super::command::Setup::parse(arg(&retired)).is_err());
        assert_eq!(
            super::super::command::Setup::parse(arg(&public)).is_ok(),
            cfg!(feature = "private-fresh-install")
        );
    }
}

#[cfg(test)]
#[path = "../../../tests/sqlite/fresh_terminal.rs"]
mod terminal_tests;
