use super::{host, route::{Folder, Id}};
use crate::label_text::{self, Label, MAX};

const MAX_CHILDREN: u32 = 32;
const MAX_ANCESTORS: u32 = 256;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum Kind {
    Directory,
    File,
}

pub(crate) struct Entry {
    pub(crate) id: Id,
    pub(crate) name: String,
    pub(crate) kind: Kind,
    pub(crate) position: Option<Id>,
}

pub(crate) struct Ancestor {
    pub(crate) id: Id,
    pub(crate) name: String,
}

pub(crate) struct Page {
    pub(crate) ancestors: Vec<Ancestor>,
    pub(crate) entries: Vec<Entry>,
    pub(crate) next: Option<Id>,
    pub(crate) scope: [u8; 32],
    pub(crate) writable: bool,
}

fn field_id(group: u32, index: u32) -> Option<Id> {
    Id::parse(&host::field::<32>(group, index, 0).ok()??)
}

fn name(group: u32, index: u32) -> Option<String> {
    let s = host::field::<255>(group, index, 1).ok()??;
    (!s.is_empty() && !s.contains(['/', '\0'])).then_some(s)
}

fn bounded<T>(group: u32, limit: u32, parse: impl Fn(u32) -> Option<T>) -> Option<Vec<T>> {
    let count = host::rows(group)?;
    (count <= limit).then_some(())?;
    (0..count).map(parse).collect()
}

pub(crate) fn decode(folder: &Folder) -> Option<Page> {
    let ancestors = bounded(0, MAX_ANCESTORS, |row| {
        Some(Ancestor { id: field_id(0, row)?, name: name(0, row)? })
    })?;
    if ancestors.last().map(|row| row.id) != folder.parent {
        return None;
    }
    let entries = bounded(1, MAX_CHILDREN, |row| {
        let kind = match host::field::<9>(1, row, 2).ok()??.as_str() {
            "directory" => Kind::Directory,
            "file" => Kind::File,
            _ => return None,
        };
        let position = match host::field::<32>(1, row, 3).ok()? {
            Some(text) => Some(Id::parse(&text)?),
            None => None,
        };
        if (kind == Kind::File) != position.is_some() { return None; }
        Some(Entry { id: field_id(1, row)?, name: name(1, row)?, kind, position })
    })?;
    let next = match host::field::<32>(2, 0, 0).ok()? {
        None => None,
        Some(value) => Some(Id::parse(&value)?),
    };
    if next.is_some() && (entries.is_empty() || next == folder.page) {
        return None;
    }
    let scope = label_text::hex(&host::field::<64>(2, 0, 1).ok()??)?;
    let writable = match host::field::<1>(2, 0, 2).ok()??.as_str() {
        "0" => false, "1" => true, _ => return None,
    };
    Some(Page { ancestors, entries, next, scope, writable })
}

pub(crate) fn labels(after: Option<&Label>) -> Option<(Vec<Label>, Option<Label>)> {
    let names = bounded(3, MAX_CHILDREN, |row| {
        Label::from_hex(&host::field::<{ MAX * 2 }>(3, row, 0).ok()??)
    })?;
    if names.windows(2).any(|pair| pair[0].bytes() >= pair[1].bytes())
        || names.first().zip(after).is_some_and(|(first, prior)| first.bytes() <= prior.bytes()) {
        return None;
    }
    let next = match host::field::<{ MAX * 2 }>(4, 0, 0).ok()? {
        None => None,
        Some(text) => Some(Label::from_hex(&text)?),
    };
    if let Some(label) = &next
        && (names.len() != MAX_CHILDREN as usize || names.last() != Some(label)) {
        return None;
    }
    Some((names, next))
}
