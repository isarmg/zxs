//! Fixed, local maintenance for the sole Web and SMB identities in v9;
//! no retired SQL SMB credential issuer, migration or protocol fallback.
mod command;
mod native_epoch;
mod native_sql;
mod native_run;
mod web_secret;
mod smb_policy;
mod terminal;
mod observed;
mod online;
mod recycle;
pub(crate) use online::{StartSmb,Online};
mod domain;

pub(crate) use command::Setup;
pub(crate) use domain::exec_child;
pub(crate) use native_run::{exec_sql_child,exec_layout_sql_child};
pub(super) use native_sql::WriteGate as NativeWriteGate;
pub(super) use domain::{require_lent_ex,require_lent_sh};
use super::{bad,request::Id};

fn login(s: &[u8]) -> bool {
    let basic = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    (1..=64).contains(&s.len()) && basic(s[0])
        && s.iter().all(|&b| basic(b) || matches!(b, b'.' | b'_' | b'-'))
}

fn distinct_roles(web:&[u8],smb:&[u8])->bool{
    web!=smb&&![b"guest".as_slice(),b"anonymous",b"nobody",b"root"].contains(&smb)
}
