//! SQL side of the paired replacement. No SMB result is produced here.
use super::{Connection, retirement};
use super::super::work_record;
use crate::{OperationId, fs::invalid, system::reader::file_location::Place};
use std::io;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Pair {
    pub(super) replacement: OperationId,
    pub(super) save: OperationId,
    pub(super) recycle: OperationId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum State { Pending, Selected, Voided }

impl Pair {
    /// Only after both exact originals were inserted in this same transaction.
    pub(super) fn reserve(self, db: &Connection) -> io::Result<()> {
        let q = db.prepare(c"INSERT INTO main.file_replace(replace_op,save_op,recycle_op,state)
                            VALUES(?1,?2,?3,0)")?;
        q.bind_bytes(1, &self.replacement.0)?;
        q.bind_bytes(2, &self.save.0)?;
        q.bind_bytes(3, &self.recycle.0)?;
        q.done()
    }

    pub(super) fn state(self, db: &Connection) -> io::Result<Option<State>> {
        let q = db.prepare(c"SELECT p.state,s.state,r.state,
            EXISTS(SELECT 1 FROM main.file_transfer_source x WHERE x.transfer_op=p.recycle_op),
            EXISTS(SELECT 1 FROM main.file_transfer_void x WHERE x.transfer_op=p.recycle_op)
            FROM main.file_replace p JOIN main.operation s ON s.op_id=p.save_op AND s.kind=10
                   JOIN main.operation r ON r.op_id=p.recycle_op AND r.kind=12
            WHERE p.replace_op=?1 AND p.save_op=?2 AND p.recycle_op=?3")?;
        q.bind_bytes(1, &self.replacement.0)?;
        q.bind_bytes(2, &self.save.0)?;
        q.bind_bytes(3, &self.recycle.0)?;
        if !work_record::start_one(&q)? { return Ok(None); }
        let shape = (q.byte(0)?,q.byte(1)?,q.byte(2)?,q.byte(3)?,q.byte(4)?);
        q.done()?;
        match shape {
            (0, 0, 0, 0, 0) => Ok(Some(State::Pending)),
            (1, 1, 1, 1, 0) => Ok(Some(State::Selected)),
            (2, 0, 0, 0, 1) => Ok(Some(State::Voided)),
            _ => Err(invalid("inconsistent original replacement receipt")),
        }
    }

    /// Find an unfinished original only if both currently acquired FILEs
    /// still match both of the exact position coordinates recorded for it.
    pub(super) fn pending_for(db:&Connection,source:&Place,target:&Place)->io::Result<Option<Self>>{
        let q=db.prepare(c"SELECT p.replace_op,p.save_op,p.recycle_op
            FROM main.file_replace p JOIN main.file_transfer m ON m.transfer_op=p.recycle_op AND m.direction=1
            JOIN main.work_edit w ON w.edit_op=p.save_op
            WHERE p.state=0 AND m.file_id=?1 AND m.source_op=?2 AND m.source_gen=?3
              AND m.source_id=?4 AND m.source_parent=?5 AND m.source_name=?6
              AND w.file_id=?7 AND w.position_op=?8 LIMIT 2")?;
        q.bind_bytes(1,&source.file.0)?;q.bind_bytes(2,&source.op.0)?;q.bind_integer(3,source.generation)?;
        q.bind_bytes(4,&source.volume.0)?;q.bind_bytes(5,&source.parent.bytes())?;
        q.bind_bytes(6,source.name.as_bytes())?;q.bind_bytes(7,&target.file.0)?;q.bind_bytes(8,&target.op.0)?;
        if !work_record::start_one(&q)?{return Ok(None);}
        let p=Self{replacement:OperationId(q.array(0)?),save:OperationId(q.array(1)?),recycle:OperationId(q.array(2)?)};
        q.done()?;
        if p.state(db)?!=Some(State::Pending){return Err(invalid("abandoned replacement original is not pending"));}
        Ok(Some(p))
    }

    /// Before ordinary recovery can write candidate bytes, reject an
    /// original operation reserved by the joined replacement path.
    pub(super) fn require_unpaired(db: &Connection, op: OperationId) -> io::Result<()> {
        let q = db.prepare(c"SELECT 1 FROM main.file_replace
            WHERE save_op=?1 OR recycle_op=?1")?;
        q.bind_bytes(1, &op.0)?;
        if work_record::start_one(&q)? {
            return Err(io::Error::new(io::ErrorKind::WouldBlock,
                "the original operation belongs to a paired file replacement"));
        }
        Ok(())
    }

    fn finish(self, db: &Connection, state: i64) -> io::Result<()> {
        let q = db.prepare(c"UPDATE main.file_replace SET state=?4
            WHERE replace_op=?1 AND save_op=?2 AND recycle_op=?3 AND state=0 RETURNING replace_op")?;
        q.bind_bytes(1, &self.replacement.0)?;
        q.bind_bytes(2, &self.save.0)?;
        q.bind_bytes(3, &self.recycle.0)?;
        q.bind_integer(4, state)?;
        if !work_record::start_one(&q)? || q.array::<16>(0)? != self.replacement.0 {
            return Err(invalid("missing original pending replacement"));
        }
        q.done()
    }

    /// Call only inside the outer transaction after rechecking the exact
    /// held writers, synchronized target work and synchronized recycle copy.
    pub(super) fn confirm(self, db: &Connection, old: Option<&[u8]>, new: &[u8]) -> io::Result<()> {
        self.finish(db, 1)?;
        retirement::record(db, self.recycle, old, new)?;
        if self.state(db)? != Some(State::Selected) {
            return Err(invalid("missing selected original replacement"));
        }
        Ok(())
    }

    /// Call only with current maintenance authority and the same original
    /// unselected media witnesses required by the ordinary transfer releaser.
    pub(super) fn release(self, db: &Connection, setting: OperationId, auth: OperationId, reason: u8)
        -> io::Result<()> {
        self.finish(db, 2)?;
        let q = db.prepare(c"INSERT INTO main.file_transfer_void(transfer_op,setting_op,auth_op,reason)
                            VALUES(?1,?2,?3,?4)")?;
        q.bind_bytes(1, &self.recycle.0)?;
        q.bind_bytes(2, &setting.0)?;
        q.bind_bytes(3, &auth.0)?;
        q.bind_integer(4, i64::from(reason))?;
        q.done()?;
        if self.state(db)? != Some(State::Voided) {
            return Err(invalid("missing released original replacement"));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../tests/unit/attached/system/smb_rust_replace_pair/tests.rs"]
mod tests;
