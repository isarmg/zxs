//! Nonsecret SETUP_PAIR request and read-only historical receipt only. A
//! structurally valid SID or an SQL match is not a Samba domain proof, a
//! maintenance approval, a password check, or a privilege to create credentials.
use super::{bad, catalog::Canonical, request::Id};
use rusqlite::{params, Transaction};
use sha2::{Digest, Sha256};
use std::io;

const PREFIX: &[u8] = b"ZXS-SQLITE-1/SETUP-PAIR\0";

/// All caller-provided identifiers describe a lookup. They confer no authority.
pub(super) struct PairRequest {
    pool: Id, init: Id, op: Id, actor: Id, approval: Id,
    web: Vec<u8>, smb: Vec<u8>, sid: [u8; 28],
}

fn login(s: &[u8]) -> bool {
    let basic = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    (1..=64).contains(&s.len()) && basic(s[0])
        && s.iter().all(|&b| basic(b) || matches!(b, b'.' | b'_' | b'-'))
}

impl PairRequest {
    pub(super) fn new(pool: Id, init: Id, op: Id, actor: Id, approval: Id,
                      web: &[u8], smb: &[u8], sid: [u8; 28]) -> io::Result<Self> {
        if [pool, init, op, actor, approval].contains(&[0; 16]) || init == op {
            return Err(bad("missing or conflicting pool/setup identifiers"));
        }
        if !login(web) || !login(smb) || web == smb
            || [b"guest".as_slice(), b"anonymous", b"nobody", b"root"].contains(&smb) {
            return Err(bad("expected two distinct, valid protocol-specific login names"));
        }
        if sid[..12] != [1, 5, 0, 0, 0, 0, 0, 5, 21, 0, 0, 0]
            || sid[12..24] == [0; 12] || sid[24..] != 1000u32.to_le_bytes() {
            return Err(bad("not a standalone protocol user SID of the required shape"));
        }
        Ok(Self { pool, init, op, actor, approval, web: web.to_vec(), smb: smb.to_vec(), sid })
    }
    pub(super) fn sha(&self) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(PREFIX);
        for id in [&self.pool, &self.init, &self.op, &self.actor, &self.approval] {
            h.update(id);
        }
        for login in [&self.web, &self.smb] {
            h.update([login.len() as u8]);
            h.update(login);
        }
        h.update(self.sid);
        h.finalize().into()
    }

    /// Query the supplied snapshot; opens no path, accepts no passwords and
    /// changes nothing. The caller must separately bind the original S, actual
    /// connection and current authorization before exposing a result. Absent is
    /// never permission to create, resume or reset either protocol account.
    pub(super) fn original(&self, tx: &Transaction<'_>, template: &Canonical) -> io::Result<History> {
        let sql = |_| bad("original fixed-pair history is unavailable");
        template.require(tx)?;
        let init: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM main.pool_birth b JOIN main.operation i ON i.op_id=b.init_op
                WHERE b.singleton=1 AND b.pool_id=?1 AND b.init_op=?2 AND i.kind=1 AND i.state=1)",
            params![&self.pool[..], &self.init[..]], |r| r.get(0)).map_err(sql)?;
        if !init { return Err(bad("this exact original pool initialization is not committed")); }
        let sha = self.sha();
        let mut st = tx.prepare(include_str!("original-first.sql")).map_err(sql)?;
        let mut rows = st.query(params![&self.pool[..], &self.op[..], &self.actor[..], &sha[..],
            &self.init[..], &self.approval[..], &self.web[..], &self.smb[..], &self.sid[..]]).map_err(sql)?;
        if let Some(row) = rows.next().map_err(sql)? {
            let id = |i| -> io::Result<Id> {
                row.get_ref(i).map_err(sql)?.as_blob().map_err(|_| bad("invalid stored account ID"))?
                    .try_into().map_err(|_| bad("invalid stored account ID"))
            };
            let (web, smb) = (id(0)?, id(1)?);
            if web == [0; 16] || smb == [0; 16] || web == smb || rows.next().map_err(sql)?.is_some() {
                return Err(bad("ambiguous or invalid fixed account IDs"));
            }
            return Ok(History::Committed { web, smb });
        }
        // Do not confuse a competing setup, a reused operation ID, or partial
        // credential history with a pristine result. No secret columns selected.
        let occupied: bool = tx.query_row("SELECT
            EXISTS(SELECT 1 FROM main.operation WHERE op_id=?1 OR kind=2)
            OR EXISTS(SELECT 1 FROM main.account_pair)
            OR EXISTS(SELECT 1 FROM main.web_password)
            OR EXISTS(SELECT 1 FROM main.smb_password)", [&self.op[..]], |r| r.get(0)).map_err(sql)?;
        if occupied { return Err(bad("there is different or incomplete setup history; no action taken")); }
        Ok(History::Absent)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum History { Absent, Committed { web: Id, smb: Id } }

#[cfg(test)]
#[path = "../../tests/sqlite/pair_history.rs"]
mod tests;
