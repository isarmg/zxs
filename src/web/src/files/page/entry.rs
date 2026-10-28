use super::object::{Object, display_path, field_error, portable_path};
use super::{Body, DecodeError, Entry, Location, Ownership};
use json_codec::Value;

pub(super) fn parse(value: Value) -> Result<Entry, DecodeError> {
    let mut fields = Object::parse(value, "entry")?;
    let entry_id = fields.id("entry_id")?;
    let path = display_path(fields.text("path")?)?;
    let name = fields.text("name")?;
    if path.text.rsplit('/').next() != Some(name.as_str()) {
        return Err(DecodeError::Relation(
            "entry name must match its display path",
        ));
    }
    let kind = fields.text("kind")?;
    let backup = fields.backup()?;
    let size = fields.integer("size")?;
    let file_id = fields.text("file_id")?;
    let (body, location, note, read_only) = match kind.as_str() {
        "directory" => directory(&mut fields, size, &file_id, backup)?,
        "file" => file(&mut fields, size, file_id, backup)?,
        _ => return Err(field_error("kind", "file or directory")),
    };
    fields.finish()?;
    Ok(Entry {
        entry_id,
        path,
        body,
        location,
        note,
        backup,
        read_only,
    })
}

type Details = (Body, Option<Location>, Option<String>, bool);
fn directory(
    fields: &mut Object,
    size: u64,
    file_id: &str,
    backup: bool,
) -> Result<Details, DecodeError> {
    if size != 0 || !file_id.is_empty() || !fields.text("version_id")?.is_empty() {
        return Err(DecodeError::Relation(
            "directories have no file identity, size or version",
        ));
    }
    let (location, note) = if fields.contains("repository_id") {
        if backup {
            return Err(DecodeError::Relation("backup directories are virtual"));
        }
        let location = location(fields, false, false)?;
        if location.selected_branch_id.is_some() {
            return Err(DecodeError::Relation(
                "directories do not select a file branch",
            ));
        }
        (Some(location), Some(fields.text("note")?))
    } else {
        (None, None)
    };
    let read_only = if backup {
        if !fields.boolean("read_only")? {
            return Err(DecodeError::Relation("backup entries are read-only"));
        }
        true
    } else {
        false
    };
    Ok((Body::Directory, location, note, read_only))
}
fn file(
    fields: &mut Object,
    size: u64,
    file_id: String,
    backup: bool,
) -> Result<Details, DecodeError> {
    if file_id.is_empty() || file_id.len() > 256 {
        return Err(field_error(
            "file_id",
            "a nonempty identity of at most 256 bytes",
        ));
    }
    let historical = if backup {
        fields.boolean("historical_only")?
    } else {
        false
    };
    let location = location(fields, true, historical)?;
    let note = fields.text("note")?;
    let node_id = fields.text("node_id")?;
    if node_id.len() > 256 || backup && node_id.is_empty() {
        return Err(field_error(
            "node_id",
            "a bounded identity, nonempty for backup files",
        ));
    }
    let read_only = fields.boolean("read_only")?;
    if backup && !read_only {
        return Err(DecodeError::Relation("backup entries are read-only"));
    }
    let (version_id, ownership) = content(fields, backup)?;
    Ok((
        Body::File {
            file_id,
            size,
            version_id,
            node_id,
            ownership,
        },
        Some(location),
        Some(note),
        read_only,
    ))
}
fn location(fields: &mut Object, file: bool, historical: bool) -> Result<Location, DecodeError> {
    let repository_id = fields.id("repository_id")?;
    let path = fields.nullable_text("repository_path")?;
    let path = match (path, historical) {
        (None, true) => None,
        (Some(path), false) if !path.is_empty() => portable_path(&path, "repository_path")?,
        _ => {
            return Err(DecodeError::Relation(
                "only historical backup files omit repository paths",
            ));
        }
    };
    if fields.boolean("container")? != file {
        return Err(DecodeError::Relation("container must identify a file"));
    }
    let selected_branch_id = fields.nullable_id("selected_branch_id")?;
    if historical && selected_branch_id.is_some() {
        return Err(DecodeError::Relation(
            "historical backup files have no selected branch",
        ));
    }
    Ok(Location {
        repository_id,
        path,
        selected_branch_id,
    })
}
fn content(fields: &mut Object, backup: bool) -> Result<(Option<String>, Ownership), DecodeError> {
    let version = fields.nullable_id("version_id")?;
    let own = fields.nullable_id("own_version_id")?;
    // Some("") explicitly suppresses the visible version on pending branches.
    let source = fields.nullable_text("source_version_id")?;
    if source.as_ref().is_some_and(|source| source.len() > 256) {
        return Err(field_error(
            "source_version_id",
            "an identity of at most 256 bytes or null",
        ));
    }
    let materialized = fields.boolean("materialized")?;
    let inherited = fields.boolean("inherited")?;
    let state = fields.text("content_state")?;
    let visible = fields.text("visible_content_state")?;
    if !matches!(
        visible.as_str(),
        "LIVE" | "AVAILABLE" | "PURGED" | "MIGRATED"
    ) {
        return Err(field_error(
            "visible_content_state",
            "a current content-state token",
        ));
    }
    if inherited != source.is_some()
        || materialized != (!inherited && own.is_some())
        || (inherited && own.is_some())
        || state != if inherited { "PENDING" } else { &visible }
    {
        return Err(DecodeError::Relation(
            "file content ownership fields disagree",
        ));
    }
    if !backup {
        let expected = if inherited {
            source.as_deref().filter(|id| !id.is_empty())
        } else {
            own.as_deref()
        };
        if version.as_deref() != expected {
            return Err(DecodeError::Relation(
                "visible version disagrees with current ownership",
            ));
        }
    } else if version.is_none() {
        return Err(DecodeError::Relation(
            "backup files require a stored version",
        ));
    }
    let ownership = if inherited {
        Ownership::Inherited
    } else if materialized {
        Ownership::Materialized
    } else {
        Ownership::Empty
    };
    Ok((version, ownership))
}
