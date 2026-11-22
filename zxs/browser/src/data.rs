use crate::{host, route::{Folder, Id}};

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
}

pub(crate) struct Ancestor {
    pub(crate) id: Id,
    pub(crate) name: String,
}

pub(crate) struct Page {
    pub(crate) ancestors: Vec<Ancestor>,
    pub(crate) entries: Vec<Entry>,
    pub(crate) next: Option<Id>,
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

pub(crate) fn decode(folder: Folder) -> Option<Page> {
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
        Some(Entry { id: field_id(1, row)?, name: name(1, row)?, kind })
    })?;
    let next = match host::field::<32>(2, 0, 0).ok()? {
        None => None,
        Some(value) => Some(Id::parse(&value)?),
    };
    if next.is_some() && (entries.is_empty() || next == folder.page) {
        return None;
    }
    Some(Page { ancestors, entries, next })
}
