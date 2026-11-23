//! Exact first ordinary publications after the genuine Rust setup.
use super::{Id, bad, blob, collision, one, sql};
use crate::smb_profile::first::{Folder, FolderIds};
use rusqlite::{Transaction, params};
use sha2::{Digest, Sha256};
use std::io;

pub(super) fn volumes(
    tx: &Transaction<'_>,
    init: &Id,
    folders: &[(Folder, FolderIds)],
) -> io::Result<()> {
    let mut s = tx
        .prepare(
            "SELECT EXISTS(SELECT 1 FROM main.volume_claim v
              JOIN main.operation o ON o.op_id=v.claim_op
              WHERE v.claim_op=?1 AND v.volume_id=?2 AND v.role=2 AND o.kind=1 AND o.state=1)",
        )
        .map_err(sql)?;
    for (f, _) in folders {
        let exact: bool = s
            .query_row(params![&init[..], &f.volume[..]], |r| r.get(0))
            .map_err(sql)?;
        if !exact {
            return Err(bad(
                "first publication is limited to selected ordinary claims of this original INIT",
            ));
        }
    }
    Ok(())
}
pub(super) fn vacant(tx: &Transaction<'_>, folders: &[(Folder, FolderIds)]) -> io::Result<()> {
    let seen: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM main.root_share)
                   OR EXISTS(SELECT 1 FROM main.share_mode)",
            [],
            |r| r.get(0),
        )
        .map_err(sql)?;
    if seen {
        return Err(bad(
            "the initial publisher never overwrites or renews previously confirmed shares",
        ));
    }
    for (_, k) in folders {
        if collision(tx, &k.publish)? || collision(tx, &k.setting)? {
            return Err(bad("one original first-share operation is already in use"));
        }
    }
    Ok(())
}
#[derive(Clone, Copy)]
pub(super) struct Authority {
    pub(super) pool: Id,
    pub(super) setup: Id,
    pub(super) web: Id,
}
impl Authority {
    pub(super) fn require_first_web(self, tx: &Transaction<'_>) -> io::Result<()> {
        let new_web: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM main.active_credential w
                JOIN main.account_pair a ON a.singleton=w.slot
                JOIN main.web_password v ON v.slot=w.slot AND v.credential_op=w.credential_op
                   AND v.generation=w.generation
                WHERE w.role=0 AND w.credential_op=?1 AND w.generation=1
                  AND a.birth_op=?1 AND a.web_id=?2)",
                params![&self.setup[..], &self.web[..]],
                |r| r.get(0),
            )
            .map_err(sql)?;
        if !new_web {
            return Err(bad(
                "the original approved first Web account and verifier are not active together",
            ));
        }
        Ok(())
    }
    fn prefix(self, domain: &[u8], op: &Id) -> Sha256 {
        let mut h = Sha256::new();
        h.update(domain);
        for piece in [&self.pool, op, &self.web, &self.setup] {
            h.update(piece);
        }
        h.update(1i64.to_be_bytes());
        h
    }
    fn publish(self, f: &Folder, k: FolderIds) -> [u8; 32] {
        let mut h = self.prefix(b"ZXS-SQLITE-1/PUBLISH-ROOT\0", &k.publish);
        h.update(f.volume);
        h.update((f.name.len() as u16).to_be_bytes());
        h.update(f.name.as_bytes());
        h.finalize().into()
    }
    fn mode(self, f: &Folder, k: FolderIds) -> [u8; 32] {
        let mut h = self.prefix(b"ZXS-SQLITE-1/SHARE-MODE\0", &k.setting);
        h.update(k.share);
        h.update(k.publish);
        h.update([f.mode.code()]);
        h.finalize().into()
    }
    fn op(self, tx: &Transaction<'_>, op: &Id, kind: i64, digest: &[u8; 32]) -> io::Result<()> {
        one(tx.execute("INSERT INTO main.operation(op_id,kind,actor_id,request_sha,state) VALUES(?1,?2,?3,?4,1)",
            params![&op[..],kind,&self.web[..],&digest[..]]))
    }
    pub(super) fn insert(self, tx: &Transaction<'_>, f: &Folder, k: FolderIds) -> io::Result<()> {
        one(tx.execute("INSERT INTO main.root_share(share_id,publish_op,author_auth_op,volume_id,display_label)
                         VALUES(?1,?2,?3,?4,?5)",
            params![&k.share[..],&k.publish[..],&self.setup[..],&f.volume[..],f.name.as_bytes()]))?;
        self.op(tx, &k.publish, 3, &self.publish(f, k))?;
        one(tx.execute("INSERT INTO main.share_mode(change_op,share_id,previous_op,author_auth_op,revision,mode)
                         VALUES(?1,?2,?3,?4,1,?5)",
            params![&k.setting[..],&k.share[..],&k.publish[..],&self.setup[..],i64::from(f.mode.code())]))?;
        self.op(tx, &k.setting, 4, &self.mode(f, k))
    }
    pub(super) fn original(self, tx: &Transaction<'_>, f: &Folder, k: FolderIds) -> io::Result<()> {
        let mut s=tx.prepare("SELECT o.kind,o.state,o.actor_id,o.request_sha,r.share_id,r.author_auth_op,r.volume_id,r.display_label
                         FROM main.operation o JOIN main.root_share r ON r.publish_op=o.op_id WHERE o.op_id=?1").map_err(sql)?;
        let mut rows = s.query([&k.publish[..]]).map_err(sql)?;
        let v = rows
            .next()
            .map_err(sql)?
            .ok_or_else(|| bad("missing original first publication"))?;
        if v.get::<_, i64>(0).map_err(sql)? != 3
            || v.get::<_, i64>(1).map_err(sql)? != 1
            || blob::<16>(v, 2)? != self.web
            || blob::<32>(v, 3)? != self.publish(f, k)
            || blob::<16>(v, 4)? != k.share
            || blob::<16>(v, 5)? != self.setup
            || blob::<16>(v, 6)? != f.volume
            || v.get_ref(7).ok().and_then(|value| value.as_blob().ok()) != Some(f.name.as_bytes())
            || rows.next().map_err(sql)?.is_some()
        {
            return Err(bad(
                "different original first publication or returned share ID",
            ));
        }
        let mut s=tx.prepare("SELECT o.kind,o.state,o.actor_id,o.request_sha,m.share_id,m.previous_op,m.author_auth_op,m.revision,m.mode
                         FROM main.operation o JOIN main.share_mode m ON m.change_op=o.op_id WHERE o.op_id=?1").map_err(sql)?;
        let mut rows = s.query([&k.setting[..]]).map_err(sql)?;
        let v = rows
            .next()
            .map_err(sql)?
            .ok_or_else(|| bad("missing original first setting"))?;
        if v.get::<_, i64>(0).map_err(sql)? != 4
            || v.get::<_, i64>(1).map_err(sql)? != 1
            || blob::<16>(v, 2)? != self.web
            || blob::<32>(v, 3)? != self.mode(f, k)
            || blob::<16>(v, 4)? != k.share
            || blob::<16>(v, 5)? != k.publish
            || blob::<16>(v, 6)? != self.setup
            || v.get::<_, i64>(7).map_err(sql)? != 1
            || v.get::<_, i64>(8).map_err(sql)? != i64::from(f.mode.code())
            || rows.next().map_err(sql)?.is_some()
        {
            return Err(bad("different original initial setting"));
        }
        Ok(())
    }
}
