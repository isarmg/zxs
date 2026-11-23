//! Fixed, local maintenance for the sole shared Web/SMB identity in exact v20;
//! no retired SQL SMB credential issuer, migration or protocol fallback.
mod command;
mod native_sql;
mod native_run;
pub(super) use native_run::offer_init_web;
pub(crate) use native_run::{exec_first_browser,exec_copy_repair_browser};
mod web_secret;
mod smb_policy;
mod terminal;
pub(super) use terminal::PublicConfirmation;
mod observed;
mod retire_ordinary;
mod import_source;
pub(crate) use native_run::{RetirementCheck,ImportCheck,check_deployment};
mod rust_enroll;
mod fresh_issuer;
#[cfg(feature="embedded-zxs")]
pub(crate) use fresh_issuer::EmbeddedInstall;
pub(super) use fresh_issuer::{ExplicitSubject,compiled_schema,require_private_install};
pub(crate) use native_run::{rust_account_worker,rust_vault_worker,SharedAccountReset};
mod recycle;
mod domain;

pub(crate) use command::Setup;
pub(crate) use native_run::exec_layout_sql_child;
pub(super) use native_sql::WriteGate as NativeWriteGate;
pub(super) use domain::{InitialLayout,require_lent_ex};
use super::{bad,request::Id};

// A nonsecret repair confirmation, not an installer role or root approval.
pub(crate) fn confirm_copy_repair(format:crate::volume_images::repair::RepairFormat,preview:String,web:Option<u64>,
    require:impl Fn()->std::io::Result<()>)->std::io::Result<()>{
    use crate::volume_images::repair::RepairFormat;
    let tty=terminal::PublicConfirmation::select()?;
    if let Some(bytes)=web{native_run::offer_copy_repair_web(&tty,format,bytes,&require)?;}
    require()?;
    match format{
        RepairFormat::Ext4=>tty.confirm_ext_repair(preview)?,
        RepairFormat::Exfat=>tty.confirm_exfat_repair(preview)?,
        RepairFormat::Fat32=>tty.confirm_fat32_repair(preview)?,
    }
    require()?;tty.check()
}

fn login(s: &[u8]) -> bool {
    let basic = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    (1..=64).contains(&s.len()) && basic(s[0])
        && s.iter().all(|&b| basic(b) || matches!(b, b'.' | b'_' | b'-'))
}
