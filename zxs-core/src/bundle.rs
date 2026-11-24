//! A deterministic interoperable ZIP payload for a code/fragments directory, not a repository.
use crate::{
    Result, invalid,
    native::{Folder, literal, measure},
    r3::Body,
};
use flate2::{Compression, write::DeflateEncoder};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Seek, SeekFrom, Write, sink},
    path::{Path, PathBuf},
};

const MAX_RAW: u64 = 384 << 20;
const MAX_VISITED: usize = 32_000;
const FIXED_DATE: u16 = 33; // DOS 1980-01-01, 00:00:00.
const UTF8: u16 = 1 << 11;
pub const RECIPE: &str = "zxs-deflate-zip32/v1";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub recipe: String,
    pub members: BTreeMap<String, Option<Body>>, // None is a directory. Never embedded in the ZIP.
}

#[derive(Debug, Default, Eq, PartialEq, Serialize)]
pub struct Diff {
    pub added: Vec<String>,
    pub changed: Vec<String>,
    pub removed: Vec<String>,
}

impl Manifest {
    pub fn diff(&self, prior: Option<&Manifest>) -> Diff {
        let mut output = Diff::default();
        for (path, body) in &self.members {
            match prior.and_then(|old| old.members.get(path)) {
                None => output.added.push(path.clone()),
                Some(old) if old != body => output.changed.push(path.clone()),
                Some(_) => (),
            }
        }
        if let Some(old) = prior {
            output.removed.extend(
                old.members
                    .keys()
                    .filter(|path| !self.members.contains_key(*path))
                    .cloned(),
            );
        }
        output
    }
}

pub struct Review {
    root: Folder,
    pub manifest: Manifest,
}

impl Review {
    pub fn read(root: &Path) -> Result<Self> {
        let root = Folder::open(root, false)?;
        let manifest = scan(&root)?;
        Ok(Self { root, manifest })
    }

    pub fn root(&self) -> &Path {
        self.root.path()
    }

    pub fn pack(&self, output: &mut File) -> Result<Body> {
        if output.metadata()?.len() != 0 || self.manifest != scan(&self.root)? {
            return Err(invalid(
                "new output and the exact previously reviewed bundle are required",
            ));
        }
        output.seek(SeekFrom::Start(0))?;
        zip32(&self.root, &self.manifest, output)?;
        if self.manifest != scan(&self.root)? {
            return Err(invalid("the selected directory changed during packaging"));
        }
        output.sync_all()?;
        measure(output, sink()) // Local ZIP headers are rewritten; hash only the finished actual file.
    }
}

pub struct Destination {
    root: PathBuf,
    path: PathBuf,
    parent: Folder,
}

impl Destination {
    pub fn new(review: &Review, path: &Path) -> Result<Self> {
        literal(path)?;
        if path.starts_with(review.root()) {
            return Err(invalid("select a new ZIP outside the bundle tree"));
        }
        let parent = Folder::open(
            path.parent()
                .ok_or_else(|| invalid("ZIP output has no parent"))?,
            false,
        )?;
        parent.absent(
            path.file_name()
                .ok_or_else(|| invalid("ZIP output has no filename"))?,
        )?;
        Ok(Self {
            root: review.root().to_owned(),
            path: path.to_owned(),
            parent,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn publish(&self, review: &Review, confirmed: bool) -> Result<Body> {
        if !confirmed || self.root != review.root() {
            return Err(invalid(
                "confirm this exact selected material and held parent before publishing a ZIP",
            ));
        }
        let leaf = self
            .path
            .file_name()
            .ok_or_else(|| invalid("ZIP output has no filename"))?;
        self.parent.absent(leaf)?;
        let mut staged = self.parent.unlinked()?;
        let body = review.pack(&mut staged)?;
        self.parent.publish(leaf, &staged, &body)?;
        Ok(body)
    }
}

fn matcher(root: &Folder) -> Result<Gitignore> {
    let mut rules = GitignoreBuilder::new(root.path());
    for item in [
        "node_modules/",
        "target/",
        ".venv/",
        "__pycache__/",
        ".DS_Store",
        "Thumbs.db",
        "*.swp",
        "*~",
    ] {
        rules.add_line(None, item)?;
    }
    let ignore = Path::new(".zxsignore");
    match root.source(ignore) {
        Ok(original) => {
            let mut text = String::new();
            (&original.file).take(65_537).read_to_string(&mut text)?;
            original.check()?;
            if text.len() > 65_536 {
                return Err(invalid(".zxsignore exceeds 64 KiB"));
            }
            for line in text.trim_start_matches('\u{feff}').lines() {
                rules.add_line(None, line)?;
            }
        }
        Err(e) if e.downcast_ref::<rustix::io::Errno>() == Some(&rustix::io::Errno::NOENT) => (),
        Err(e) => return Err(e),
    }
    Ok(rules.build()?)
}

fn scan(root: &Folder) -> Result<Manifest> {
    let ignore = matcher(root)?;
    let mut pending = vec![(PathBuf::new(), 0)];
    let mut members = BTreeMap::new();
    let mut aliases = BTreeSet::new();
    let mut seen = 0;
    let mut raw = 0_u64;
    while let Some((parent, depth)) = pending.pop() {
        if depth >= 48 {
            return Err(invalid("bundle directory depth budget exceeded"));
        }
        for name in root.names(&parent)? {
            seen += 1;
            if seen > MAX_VISITED {
                return Err(invalid("bundle visit budget exceeded"));
            }
            let name = name
                .to_str()
                .ok_or_else(|| invalid("ZIP paths must be exact UTF-8"))?;
            if matches!(
                name,
                ".git"
                    | ".zxs-remote-state"
                    | ".workspace.lock"
                    | "workspace.sqlite"
                    | "workspace.sqlite-journal"
                    | "workspace.sqlite-wal"
                    | "workspace.sqlite-shm"
            ) {
                continue;
            }
            let path = parent.join(name);
            // No traversal into symlinks; ignore those explicitly excluded by the confirmed rules.
            let pin = root.pin(&path)?;
            let meta = pin.metadata()?;
            if ignore.matched(&path, meta.is_dir()).is_ignore() {
                continue;
            }
            if name.ends_with([' ', '.'])
                || name
                    .chars()
                    .any(|c| c.is_control() || r#"<>:"\|?*"#.contains(c))
                || name.len() > 255
            {
                return Err(invalid("not an interoperable exact ZIP member name"));
            }
            let text = path
                .to_str()
                .ok_or_else(|| invalid("not an exact UTF-8 ZIP path"))?
                .to_owned();
            if text.len() > 1024 || !aliases.insert(text.to_lowercase()) {
                return Err(invalid("ambiguous or overlong ZIP member paths"));
            }
            let member = if meta.is_dir() {
                pending.push((path, depth + 1));
                None
            } else if meta.is_file() {
                let item = root.source(&path)?;
                if item.before.dev != std::os::unix::fs::MetadataExt::dev(&meta)
                    || item.before.ino != std::os::unix::fs::MetadataExt::ino(&meta)
                {
                    return Err(invalid("path was replaced during bundle inventory"));
                }
                raw = raw
                    .checked_add(item.before.length)
                    .ok_or_else(|| invalid("bundle size overflow"))?;
                if raw > MAX_RAW {
                    return Err(invalid(
                        "first ZIP path accepts at most 384 MiB of ordinary code/fragments",
                    ));
                }
                let body = item.scan(sink())?;
                if body.length != item.before.length {
                    return Err(invalid("the previously budgeted member changed length"));
                }
                Some(body)
            } else {
                return Err(invalid("bundle contains a selected link or special object"));
            };
            if members.insert(text, member).is_some() {
                return Err(invalid("duplicate bundle path"));
            }
        }
    }
    root.check()?;
    Ok(Manifest {
        recipe: RECIPE.to_owned(),
        members,
    })
}

struct Entry {
    name: String,
    start: u32,
    method: u16,
    crc: u32,
    compressed: u32,
    original: u32,
}

struct CountCrc<W> {
    output: W,
    crc: crc32fast::Hasher,
}
impl<W: Write> Write for CountCrc<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let n = self.output.write(bytes)?;
        self.crc.update(&bytes[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.output.flush()
    }
}

fn u16s(f: &mut File, fields: &[u16]) -> std::io::Result<()> {
    for n in fields {
        f.write_all(&n.to_le_bytes())?;
    }
    Ok(())
}
fn u32s(f: &mut File, fields: &[u32]) -> std::io::Result<()> {
    for n in fields {
        f.write_all(&n.to_le_bytes())?;
    }
    Ok(())
}

fn zip32(root: &Folder, manifest: &Manifest, output: &mut File) -> Result<()> {
    let mut rows = Vec::with_capacity(manifest.members.len());
    for (path, expected) in &manifest.members {
        let (name, method) = if expected.is_some() {
            (path.clone(), 8)
        } else {
            (format!("{path}/"), 0)
        };
        let start = u32::try_from(output.stream_position()?)?;
        let width = u16::try_from(name.len())?;
        u32s(output, &[0x0403_4b50])?;
        u16s(output, &[20, UTF8, method, 0, FIXED_DATE])?;
        u32s(output, &[0, 0, 0])?;
        u16s(output, &[width, 0])?;
        output.write_all(name.as_bytes())?;
        let data = output.stream_position()?;
        let (crc, original) = if let Some(expected) = expected {
            let held = root.source(Path::new(path))?;
            let mut encoded = CountCrc {
                output: DeflateEncoder::new(&mut *output, Compression::new(6)),
                crc: crc32fast::Hasher::new(),
            };
            if held.scan(&mut encoded)? != *expected {
                return Err(invalid("reviewed directory member changed"));
            }
            let crc = encoded.crc.finalize();
            encoded.output.finish()?;
            (crc, u32::try_from(expected.length)?)
        } else {
            (0, 0)
        };
        let end = output.stream_position()?;
        let compressed = u32::try_from(end - data)?;
        output.seek(SeekFrom::Start(u64::from(start) + 14))?;
        u32s(output, &[crc, compressed, original])?;
        output.seek(SeekFrom::Start(end))?;
        rows.push(Entry {
            name,
            start,
            method,
            crc,
            compressed,
            original,
        });
    }
    let central = u32::try_from(output.stream_position()?)?;
    for e in &rows {
        u32s(output, &[0x0201_4b50])?;
        u16s(output, &[0x0314, 20, UTF8, e.method, 0, FIXED_DATE])?;
        u32s(output, &[e.crc, e.compressed, e.original])?;
        u16s(output, &[u16::try_from(e.name.len())?, 0, 0, 0, 0])?;
        let external = if e.method == 0 {
            (0o040755 << 16) | 0x10
        } else {
            0o100644 << 16
        };
        u32s(output, &[external, e.start])?;
        output.write_all(e.name.as_bytes())?;
    }
    let bytes = u32::try_from(output.stream_position()? - u64::from(central))?;
    let count = u16::try_from(rows.len())?;
    u32s(output, &[0x0605_4b50])?;
    u16s(output, &[0, 0, count, count])?;
    u32s(output, &[bytes, central])?;
    u16s(output, &[0])?;
    Ok(())
}
