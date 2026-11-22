//! File labels are logical SYSTEM metadata keyed by the immutable FileId.
use super::{Catalog, Connection, WebAdmin, navigation};
use crate::{DirectoryId, FileId, OperationId, identity::ShareId, label_text::{Label, MAX}};
use std::io;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Selected {
    pub(crate) share: ShareId,
    pub(crate) parent: Option<DirectoryId>,
    pub(crate) file: FileId,
    pub(crate) position: OperationId,
    pub(crate) scope: [u8; 32],
}

pub(crate) struct Labels {
    pub(crate) names: Vec<Label>,
    pub(crate) next: Option<Label>,
}

fn list(db: &Connection, file: FileId, after: Option<&Label>) -> io::Result<Labels> {
    let mut q = db.prepare(c"SELECT label FROM main.file_label WHERE file_id=?1 AND label>?2 ORDER BY label LIMIT ?3")?;
    q.bind_bytes(1, &file.0)?;
    q.bind_bytes(2, after.map_or(&[], Label::bytes))?;
    q.bind_integer(3, (navigation::PAGE + 1) as i64)?;
    let mut names = Vec::with_capacity(navigation::PAGE + 1);
    loop {
        match q.step() {
            100 => {
                let s = Label::from_bytes(q.row()?.blob(0, MAX)?)
                    .filter(|label| names.last().or(after).is_none_or(|last: &Label| last.bytes() < label.bytes()))
                    .ok_or_else(|| crate::fs::invalid("invalid stored file label"))?;
                names.push(s);
            }
            101 => break,
            rc => {
                super::check(rc)?;
                return Err(crate::fs::invalid("invalid file label row"));
            }
        }
    }
    let next = if names.len() > navigation::PAGE {
        names.pop();
        names.last().cloned()
    } else {
        None
    };
    Ok(Labels { names, next })
}

pub(super) fn contains(db: &Connection, file: FileId, label: &Label) -> io::Result<bool> {
    let q = db.prepare(c"SELECT 1 FROM main.file_label WHERE file_id=?1 AND label=?2")?;
    q.bind_bytes(1, &file.0)?;
    q.bind_bytes(2, label.bytes())?;
    if !super::super::work_record::start_one(&q)? {
        return Ok(false);
    }
    q.done()?;
    Ok(true)
}

fn set(db: &Connection, file: FileId, name: &Label, present: bool) -> io::Result<()> {
    let q = if present {
        db.prepare(c"INSERT INTO main.file_label(file_id,label) VALUES(?1,?2) ON CONFLICT(file_id,label) DO NOTHING")?
    } else {
        db.prepare(c"DELETE FROM main.file_label WHERE file_id=?1 AND label=?2")?
    };
    q.bind_bytes(1, &file.0)?;
    q.bind_bytes(2, name.bytes())?;
    q.done()
}

impl WebAdmin<'_> {
    pub(crate) fn file_labels(&self, selected: Selected, after: Option<&Label>) -> io::Result<Labels> {
        self.source.at(|db, c| {
            self.require_label_file(db, &c, selected, false)?;
            list(db, selected.file, after)
        })
    }

    pub(crate) fn set_file_label(
        &self,
        selected: Selected,
        name: &Label,
        present: bool,
        last: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        self.source.dir.require()?;
        let writer = Connection::open(&self.source.dir, true)?;
        self.source.transaction_before_commit(
            &writer,
            c"BEGIN IMMEDIATE",
            |db, c| {
                self.require_label_file(db, &c, selected, true)?;
                set(db, selected.file, name, present)
            },
            last,
        )
    }

    fn require_label_file(&self, db: &Connection, c: &Catalog, selected: Selected, write: bool) -> io::Result<()> {
        let identity = self.current(db, c)?;
        navigation::require_label_file(db, c, &identity, selected, write)
    }
}

#[cfg(test)]
#[path = "../../tests/unit/attached/system/file_labels.rs"]
mod tests;
