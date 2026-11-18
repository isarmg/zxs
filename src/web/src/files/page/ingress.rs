use super::object::{Object, direct_child, field_error, lookup_path};
use super::{Context, DecodeError, DirectoryKey, Entry, Page, PageParts};
use json_codec::Value;
use std::collections::{BTreeMap, BTreeSet};

const MAX_CURSOR_BYTES: usize = 32_768;

pub(super) fn page(
    value: Value,
    slot: &str,
    directory: &DirectoryKey,
    limit: usize,
    selected: Option<&Entry>,
) -> Result<Page, DecodeError> {
    let mut fields = Object::parse(value, "page")?;
    if fields.text("slot_id")? != slot {
        return Err(DecodeError::Relation(
            "page slot differs from the requested slot",
        ));
    }
    let directory_key = directory.0.as_str();
    let actual = lookup_path(&fields.text("path")?, "path")?;
    if actual.as_ref().map_or("", |path| path.key()) != directory_key {
        return Err(DecodeError::Relation(
            "page path differs from the requested directory",
        ));
    }
    let backup = fields.backup()?;
    let writable = fields.boolean("writable")?;
    if writable && backup {
        return Err(DecodeError::Relation(
            "backup pages must be read-only",
        ));
    }
    let entries = entries(fields.array("entries")?, directory_key, limit, backup)?;
    let selected = selection(
        fields.take("selected_entry")?,
        selected,
        directory_key,
        backup,
        &entries,
    )?;
    let total = fields.integer("total_entries")?;
    let directory_file_count = fields.integer("directory_file_count")?;
    let listed_files = entries
        .iter()
        .filter(|entry| entry.kind() == storage_contract::NamespaceEntryKind::File)
        .count();
    let selected_outside_page = selected.as_ref().is_some_and(|selected| {
        selected.kind() == storage_contract::NamespaceEntryKind::File
            && !entries
                .iter()
                .any(|entry| entry.path.key == selected.path.key)
    });
    if total < entries.len() as u64
        || directory_file_count < (listed_files + usize::from(selected_outside_page)) as u64
    {
        return Err(DecodeError::Relation(
            "directory counts are smaller than their page and selection observations",
        ));
    }
    let snapshot = fields.text("snapshot")?;
    if snapshot.is_empty() || snapshot.len() > MAX_CURSOR_BYTES {
        return Err(field_error("snapshot", "a nonempty bounded snapshot token"));
    }
    let next = cursor(&mut fields, "next_cursor")?;
    let previous = cursor(&mut fields, "previous_cursor")?;
    let partial_page = !entries.is_empty() && total > entries.len() as u64;
    if (next.is_some() || previous.is_some()) != partial_page {
        return Err(DecodeError::Relation(
            "exactly nonempty partial pages require pagination cursors",
        ));
    }
    let context = context(fields.take("context")?)?;
    match (&context, backup) {
        (Some(_), true) => return Err(DecodeError::Relation(
            "backup listings cannot have an ordinary write context",
        )),
        (None, true) => {}
        (None, false) if writable && directory_key.is_empty()
            && total == 0 && directory_file_count == 0
            && entries.is_empty() && selected.is_none() => {}
        (None, false) => return Err(DecodeError::Relation(
            "only an empty primary disk root may lack its registered namespace",
        )),
        (Some(context), false) => {
            let actual = context.repository_path.as_ref().map_or("", |path| path.key());
            if actual != directory_key {
                return Err(DecodeError::Relation(
                    "the registered namespace path differs from the requested folder",
                ));
            }
            for entry in entries.iter().chain(selected.iter()) {
                primary_location(entry, context)?;
            }
        }
    }
    // Directory branch/preview fields are not a second wire format. Even
    // empty/false extras are rejected by the ordinary page contract.
    fields.finish()?;
    Ok(Page {
        parts: PageParts {
            entries,
            selected,
            next,
            previous,
            total,
            context,
            writable,
            directory_file_count,
        },
    })
}
fn cursor(fields: &mut Object, field: &'static str) -> Result<Option<String>, DecodeError> {
    let cursor = fields.nullable_text(field)?;
    if cursor
        .as_ref()
        .is_some_and(|cursor| cursor.is_empty() || cursor.len() > MAX_CURSOR_BYTES)
    {
        return Err(field_error(field, "null or a nonempty bounded cursor"));
    }
    Ok(cursor)
}
fn entries(
    values: Vec<Value>,
    directory_key: &str,
    limit: usize,
    backup: bool,
) -> Result<Vec<Entry>, DecodeError> {
    if values.len() > limit {
        return Err(field_error(
            "entries",
            "no larger than the requested page size",
        ));
    }
    let mut ids = BTreeMap::new();
    let mut paths = BTreeSet::new();
    let mut entries = Vec::with_capacity(values.len());
    for value in values {
        let entry = super::entry::parse(value)?;
        if !direct_child(&entry, directory_key) || entry.backup() != backup {
            return Err(DecodeError::Relation(
                "entry lies outside the requested directory or disk mode",
            ));
        }
        if let Some((kind, file_id)) = ids.insert(
            entry.entry_id().to_owned(),
            (entry.kind(), entry.file_id().to_owned()),
        ) && !file_alias(kind, &file_id, &entry)
        {
            return Err(DecodeError::Relation(
                "directory page reuses an identity for a different entity",
            ));
        }
        if !paths.insert(entry.path.key.clone()) {
            return Err(DecodeError::Relation(
                "directory page repeats a logical path",
            ));
        }
        entries.push(entry);
    }
    Ok(entries)
}
fn file_alias(kind: storage_contract::NamespaceEntryKind, file_id: &str, entry: &Entry) -> bool {
    kind == storage_contract::NamespaceEntryKind::File
        && entry.kind() == kind
        && entry.file_id() == file_id
}
fn selection(
    value: Value,
    requested: Option<&Entry>,
    directory_key: &str,
    backup: bool,
    entries: &[Entry],
) -> Result<Option<Entry>, DecodeError> {
    if value == Value::Null {
        if requested.is_some_and(|requested| {
            entries.iter().any(|entry| {
                entry.entry_id == requested.entry_id && entry.path.key == requested.path.key
            })
        }) {
            return Err(DecodeError::Relation(
                "selected entry is missing from its own page observation",
            ));
        }
        return Ok(None);
    }
    let entry = super::entry::parse(value)?;
    let Some(requested) = requested else {
        return Err(DecodeError::Relation(
            "server returned an unrequested selection",
        ));
    };
    if entry.entry_id != requested.entry_id
        || entry.path.key != requested.path.key
        || entry.kind() != requested.kind()
        || entry.file_id() != requested.file_id()
        || !direct_child(&entry, directory_key)
        || entry.backup() != backup
    {
        return Err(DecodeError::Relation(
            "selected entry differs from the requested identity",
        ));
    }
    if entries.iter().any(|listed| {
        listed.entry_id == entry.entry_id
            && listed.path.key != entry.path.key
            && !file_alias(listed.kind(), listed.file_id(), &entry)
    }) {
        return Err(DecodeError::Relation(
            "selected identity differs from its listed aliases",
        ));
    }
    if entries
        .iter()
        .any(|listed| listed.path.key == entry.path.key && listed != &entry)
    {
        return Err(DecodeError::Relation(
            "selected entry disagrees with its page observation",
        ));
    }
    Ok(Some(entry))
}
fn context(value: Value) -> Result<Option<Context>, DecodeError> {
    if value == Value::Null {
        return Ok(None);
    }
    let mut fields = Object::parse(value, "context")?;
    let repository_id = fields.id("repository_id")?;
    let repository_path = lookup_path(&fields.text("repository_path")?, "repository_path")?;
    fields.id("branch_id")?;
    let base_head = fields.id("base_head")?;
    fields.finish()?;
    Ok(Some(Context {
        repository_id,
        repository_path,
        base_head,
    }))
}
fn primary_location(entry: &Entry, context: &Context) -> Result<(), DecodeError> {
    let Some(location) = &entry.location else {
        return Err(DecodeError::Relation("primary entries require a registered location"));
    };
    let path = location.path.as_ref().ok_or(DecodeError::Relation(
        "primary entries require their complete ordinary path",
    ))?;
    // The page already proved that the displayed path is a direct child of the
    // requested parent. Do not strip a first component or compare just its leaf.
    if location.repository_id != context.repository_id || path.as_str() != entry.path.text {
        return Err(DecodeError::Relation(
            "current entry location differs from its directory context",
        ));
    }
    Ok(())
}
