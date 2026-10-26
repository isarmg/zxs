//! Reproducibly generate the fixed naming tables from checked-in Unicode data.
#![forbid(unsafe_code)]
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    error::Error,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};
type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn source(root: &Path, name: &str, expected: &str) -> Result<String> {
    let bytes = fs::read(root.join("src/naming/data").join(name))?;
    let mut hash = String::with_capacity(64);
    for byte in Sha256::digest(&bytes) {
        write!(&mut hash, "{byte:02x}").expect("writing to String cannot fail");
    }
    if hash != expected {
        return Err(format!("Unicode 16 input checksum differs: {name}").into());
    }
    Ok(String::from_utf8(bytes)?)
}
fn cp(value: &str) -> Result<u32> {
    Ok(u32::from_str_radix(value.trim(), 16)?)
}
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
fn generate(root: &Path) -> Result<String> {
    let unicode = source(
        root,
        "unicode-data-16.0.0.txt",
        "ff58e5823bd095166564a006e47d111130813dcf8bf234ef79fa51a870edb48f",
    )?;
    let derived = source(
        root,
        "derived-core-properties-16.0.0.txt",
        "39d35161f2954497f69e08bdb9e701493f476a3d30222de20028feda36c1dabd",
    )?;
    let folding = source(
        root,
        "case-folding-16.0.0.txt",
        "6f1f9c588eb4a5c718d9e8f93b782685e5c7fec872cf05e8e6878053599e09bb",
    )?;
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
    let mut out = String::from(
        "// Fixed Unicode 16.0.0 tables; regenerate only as an explicit naming policy change.\n",
    );
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
fn main() -> Result<()> {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut write = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--write" => write = true,
            "--check" => write = false,
            "--product-root" => root = PathBuf::from(args.next().ok_or("missing product root")?),
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }
    let generated = generate(&root)?;
    let target = root.join("src/naming/src/unicode16.rs");
    if write {
        let pending = target.with_extension("rs.pending");
        fs::write(&pending, &generated)?;
        fs::rename(pending, &target)?;
    } else if fs::read_to_string(&target)? != generated {
        return Err("fixed Unicode table differs; review inputs and explicitly regenerate".into());
    }
    println!("ZXS Unicode 16.0.0 inputs and generated tables: PASS");
    Ok(())
}
