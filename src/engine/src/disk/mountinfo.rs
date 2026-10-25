use crate::{Error, Result};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(super) struct Mount {
    point: PathBuf,
    pub(super) kind: String,
    pub(super) source: PathBuf,
}

/// Resolve only the mount table; device canonicalization and topology checks
/// remain in production_device. The caller supplies its canonical absolute root.
pub(super) fn resolve(mountinfo: &str, canonical: &Path) -> Result<Mount> {
    if !canonical.is_absolute() {
        return Err(invalid("managed mount lookup requires an absolute path"));
    }
    let mut selected = None;
    // Only LF and ASCII space delimit kernel records/fields. In particular,
    // lines()/split_whitespace() would discard valid CR or Unicode path bytes.
    for line in mountinfo.split_terminator('\n') {
        // Never skip a malformed row: it might hide a more specific mount and
        // cause us to incorrectly resolve the managed root to an ancestor.
        let mount = parse(line)?;
        if canonical.starts_with(&mount.point)
            && selected.as_ref().is_none_or(|previous: &Mount| {
                mount.point.as_os_str().len() > previous.point.as_os_str().len()
            })
        {
            selected = Some(mount);
        }
    }
    selected.ok_or_else(|| invalid("cannot resolve managed mount"))
}

fn parse(line: &str) -> Result<Mount> {
    // Format: https://www.kernel.org/doc/html/latest/filesystems/proc.html
    // Section 3.5. Unknown optional fields are intentionally ignored.
    let (left, right) = line
        .split_once(" - ")
        .ok_or_else(|| invalid("mountinfo separator missing"))?;
    let fields = left.split(' ').collect::<Vec<_>>();
    let filesystem = right.split(' ').collect::<Vec<_>>();
    if fields.len() < 6
        || filesystem.len() != 3
        || fields
            .iter()
            .chain(&filesystem)
            .any(|field| field.is_empty() || field.contains(['\0', '\t', '\n']))
        || !decimal(fields[0])
        || !decimal(fields[1])
        || !fields[2]
            .split_once(':')
            .is_some_and(|(major, minor)| decimal(major) && decimal(minor))
    {
        return Err(invalid("malformed mountinfo fields"));
    }
    // The root is filesystem-specific (nsfs emits labels such as net:[inode]),
    // so validate its encoding without requiring an absolute pathname.
    decode_field(fields[3], Escapes::Path)?;
    let point = PathBuf::from(decode_field(fields[4], Escapes::Path)?);
    if !point.is_absolute() {
        return Err(invalid("mountinfo mount point must be absolute"));
    }
    Ok(Mount {
        point,
        kind: filesystem[0].to_owned(),
        source: decode_field(filesystem[1], Escapes::Source)?.into(),
    })
}

fn decimal(field: &str) -> bool {
    !field.is_empty() && field.bytes().all(|byte| byte.is_ascii_digit())
}

enum Escapes {
    Path,
    Source,
}

fn decode_field(encoded: &str, escapes: Escapes) -> Result<String> {
    // fs/proc_namespace.c::show_mountinfo and fs/btrfs/super.c::btrfs_show_devname
    // escape space, tab, LF and backslash. The generic source mangle() also
    // escapes '#'. Decode once: \134040 names a literal backslash plus "040",
    // not a space. Reject escapes the kernel does not emit, including NUL.
    let mut decoded = String::with_capacity(encoded.len());
    let mut chars = encoded.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let ch = match (chars.next(), chars.next(), chars.next()) {
                (Some('0'), Some('4'), Some('0')) => ' ',
                (Some('0'), Some('1'), Some('1')) => '\t',
                (Some('0'), Some('1'), Some('2')) => '\n',
                (Some('1'), Some('3'), Some('4')) => '\\',
                (Some('0'), Some('4'), Some('3')) if matches!(escapes, Escapes::Source) => '#',
                _ => return Err(invalid("invalid mountinfo escape")),
            };
            decoded.push(ch);
        } else if matches!(ch, '\0' | ' ' | '\t' | '\n') {
            return Err(invalid("unescaped mountinfo field delimiter"));
        } else {
            decoded.push(ch);
        }
    }
    Ok(decoded)
}

fn invalid(message: &str) -> Error {
    Error::new(storage_contract::ErrorCode::DiskIdentityMismatch, message)
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/disk/mountinfo_tests.rs"]
mod tests;
