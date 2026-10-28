//! Naming property tables: the letter/mark/number allow list, the
//! Default_Ignorable_Code_Point reject list and full (C + F) case folding.
use crate::{Result, cp};
use std::{collections::BTreeMap, fmt::Write as _};

fn compact(mut ranges: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    ranges.sort_unstable();
    let mut out: Vec<(u32, u32)> = Vec::new();
    for (start, end) in ranges {
        if let Some(last) = out.last_mut()
            && start <= last.1 + 1
        {
            last.1 = last.1.max(end);
            continue;
        }
        out.push((start, end));
    }
    out
}
pub(crate) fn generate(
    unicode: &str,
    derived: &str,
    folding: &str,
    version: &str,
) -> Result<String> {
    let mut allowed = Vec::new();
    let mut first = None;
    for line in unicode.lines().filter(|line| !line.is_empty()) {
        let f = line.split(';').collect::<Vec<_>>();
        if f.len() < 3 {
            return Err("invalid UnicodeData record".into());
        }
        let code = cp(f[0])?;
        let allowed_category = matches!(f[2].as_bytes().first(), Some(b'L' | b'M' | b'N'));
        if f[1].ends_with(", First>") {
            first = Some((code, allowed_category));
        } else if f[1].ends_with(", Last>") {
            let (start, include) = first.take().ok_or("unpaired Unicode range")?;
            if include != allowed_category || code < start {
                return Err("invalid Unicode range".into());
            }
            if include {
                allowed.push((start, code));
            }
        } else if allowed_category {
            allowed.push((code, code));
        }
    }
    if first.is_some() {
        return Err("unterminated Unicode range".into());
    }
    let mut ignored = Vec::new();
    for line in derived.lines() {
        let fields = line
            .split('#')
            .next()
            .expect("invariant: split yields at least one segment")
            .split(';')
            .map(str::trim)
            .collect::<Vec<_>>();
        if fields.len() < 2 || fields[1] != "Default_Ignorable_Code_Point" {
            continue;
        }
        let (start, end) = fields[0].split_once("..").unwrap_or((fields[0], fields[0]));
        ignored.push((cp(start)?, cp(end)?));
    }
    let mut fold = BTreeMap::new();
    for line in folding.lines() {
        let fields = line
            .split('#')
            .next()
            .expect("invariant: split yields at least one segment")
            .split(';')
            .map(str::trim)
            .collect::<Vec<_>>();
        if fields.len() < 3 || !matches!(fields[1], "C" | "F") {
            continue;
        }
        let mut mapped = String::new();
        for value in fields[2].split_whitespace() {
            write!(mapped, "\\u{{{:04x}}}", cp(value)?)?;
        }
        fold.insert(cp(fields[0])?, mapped);
    }
    let mut out = format!(
        "// Fixed Unicode {version} tables; regenerate only as an explicit naming policy change.\n"
    );
    writeln!(out, "pub const UNICODE_VERSION: &str = \"{version}\";")?;
    ignored.sort_unstable();
    for (name, ranges) in [("ALLOWED", compact(allowed)), ("IGNORED", ignored)] {
        writeln!(out, "pub const {name}: &[(u32, u32)] = &[")?;
        for (start, end) in ranges {
            writeln!(out, "    (0x{start:x}, 0x{end:x}),")?;
        }
        out.push_str("];\n");
    }
    out.push_str("pub const FOLD: &[(u32, &str)] = &[\n");
    for (code, mapped) in fold {
        writeln!(out, "    (0x{code:x}, \"{mapped}\"),")?;
    }
    out.push_str("];\n");
    Ok(out)
}
