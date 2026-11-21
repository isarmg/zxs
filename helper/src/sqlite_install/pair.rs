//! Fixed, local maintenance for the sole Web and SMB identities in v9;
//! no retired SQL SMB credential issuer, migration or protocol fallback.
mod command;
mod native_epoch;
mod native_sql;
mod native_run;
pub(super) use native_run::offer_init_web;
pub(crate) use native_run::{exec_first_browser,exec_ext_repair_browser};
mod web_secret;
mod smb_policy;
mod terminal;
pub(super) use terminal::PublicConfirmation;
mod observed;
mod online;
mod recycle;
mod inactive_web_unit;
mod web_systemd;
mod web_systemd_state;
mod web_unit_process;
mod web_unit_cgroup;
mod control_web_unit;
pub(crate) use control_web_unit::Control as WebUnitControl;
mod install_web_unit;
pub(crate) use install_web_unit::InstallWebUnit;
pub(crate) use online::{StartSmb,Online};
mod domain;

pub(crate) use command::Setup;
pub(crate) use domain::exec_child;
pub(crate) use native_run::{exec_sql_child,exec_layout_sql_child};
pub(super) use native_sql::WriteGate as NativeWriteGate;
pub(super) use domain::{require_lent_ex,require_lent_sh};
use super::{bad,request::Id};

// A nonsecret repair confirmation, not an installer role or root approval.
pub(crate) fn confirm_ext_repair(preview:String,web:Option<u64>,require:impl Fn()->std::io::Result<()>)->std::io::Result<()>{
    let tty=terminal::PublicConfirmation::select()?;
    if let Some(bytes)=web{native_run::offer_ext_repair_web(&tty,bytes,&require)?;}
    require()?;tty.confirm_ext_repair(preview)?;require()?;tty.check()
}

fn login(s: &[u8]) -> bool {
    let basic = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    (1..=64).contains(&s.len()) && basic(s[0])
        && s.iter().all(|&b| basic(b) || matches!(b, b'.' | b'_' | b'-'))
}

fn distinct_roles(web:&[u8],smb:&[u8])->bool{
    web!=smb&&![b"guest".as_slice(),b"anonymous",b"nobody",b"root"].contains(&smb)
}
