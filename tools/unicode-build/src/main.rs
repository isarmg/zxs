//! Reproducibly generate the fixed naming tables from pinned Unicode data.
//!
//! `src/naming/data/sources.tsv` is the only authority for the Unicode version,
//! every input and test-fixture digest and every generated output digest.
#![forbid(unsafe_code)]
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    fmt::Write as _,
    fs,
    path::{Component, Path, PathBuf},
};

mod normalization;
mod properties;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const MANIFEST: &str = "src/naming/data/sources.tsv";
const INPUTS: [&str; 4] = [
    "unicode-data",
    "derived-core-properties",
    "case-folding",
    "derived-normalization-props",
];
const FIXTURES: [&str; 1] = ["normalization-test"];
const OUTPUTS: [&str; 2] = ["properties", "normalization"];

fn cp(value: &str) -> Result<u32> {
    Ok(u32::from_str_radix(value.trim(), 16)?)
}

fn sha256(bytes: &[u8]) -> String {
    let mut hash = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(&mut hash, "{byte:02x}").expect("invariant: writing to a String cannot fail");
    }
    hash
}

#[derive(Clone, Debug, PartialEq)]
enum Kind {
    Input,
    Fixture,
    Output,
}

/// One manifest record: a repository-relative file and its pinned SHA-256.
#[derive(Clone, Debug)]
struct Entry {
    kind: Kind,
    role: String,
    path: PathBuf,
    sha256: String,
}

#[derive(Debug)]
struct Manifest {
    text: String,
    version: String,
    entries: Vec<Entry>,
}

impl Manifest {
    fn parse(text: &str) -> Result<Self> {
        let mut version = None;
        let mut entries: Vec<Entry> = Vec::new();
        for line in text.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fields = line.split('\t').collect::<Vec<_>>();
            match fields[..] {
                ["version", value] if version.is_none() => {
                    if value.split('.').count() != 3
                        || !value.split('.').all(|part| part.parse::<u8>().is_ok())
                    {
                        return Err(format!("invalid Unicode version {value:?}").into());
                    }
                    version = Some(value.to_owned());
                }
                [kind, role, path, sha256, ..] => {
                    let kind = match (kind, fields.len()) {
                        ("input", 5) => Kind::Input,
                        ("fixture", 5) => Kind::Fixture,
                        ("output", 4) => Kind::Output,
                        _ => return Err(format!("invalid manifest record: {line}").into()),
                    };
                    let known = match kind {
                        Kind::Input => INPUTS.contains(&role),
                        Kind::Fixture => FIXTURES.contains(&role),
                        Kind::Output => OUTPUTS.contains(&role),
                    };
                    let path = PathBuf::from(path);
                    let relative = path
                        .components()
                        .all(|component| matches!(component, Component::Normal(_)));
                    let digest = sha256.len() == 64
                        && sha256
                            .bytes()
                            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
                    if !known || !relative || !digest || entries.iter().any(|e| e.role == role) {
                        return Err(format!("invalid manifest record: {line}").into());
                    }
                    entries.push(Entry {
                        kind,
                        role: role.to_owned(),
                        path,
                        sha256: sha256.to_owned(),
                    });
                }
                _ => return Err(format!("invalid manifest record: {line}").into()),
            }
        }
        let version = version.ok_or("manifest lacks a Unicode version")?;
        if entries.len() != INPUTS.len() + FIXTURES.len() + OUTPUTS.len() {
            return Err("manifest must list every input, fixture and output once".into());
        }
        Ok(Self {
            text: text.to_owned(),
            version,
            entries,
        })
    }

    fn entry(&self, role: &str) -> &Entry {
        self.entries
            .iter()
            .find(|entry| entry.role == role)
            .expect("invariant: parse admitted every known role exactly once")
    }

    fn of(&self, kind: Kind) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(move |entry| entry.kind == kind)
    }

    /// The same manifest text with output digests replaced.
    fn with_outputs(&self, outputs: &[(Entry, String)]) -> String {
        let mut text = String::new();
        for line in self.text.split_inclusive('\n') {
            let fields = line.trim_end_matches('\n').split('\t').collect::<Vec<_>>();
            match outputs
                .iter()
                .find(|(entry, _)| fields.first() == Some(&"output") && fields[1] == entry.role)
            {
                Some((entry, content)) => {
                    let path = entry.path.to_str().expect("invariant: parsed from UTF-8");
                    let digest = sha256(content.as_bytes());
                    writeln!(text, "output\t{}\t{path}\t{digest}", entry.role)
                        .expect("invariant: writing to a String cannot fail");
                }
                None => text.push_str(line),
            }
        }
        text
    }
}

/// Read a pinned file below `root`, failing on any digest difference.
fn pinned(root: &Path, entry: &Entry) -> Result<String> {
    let bytes = fs::read(root.join(&entry.path))
        .map_err(|error| format!("{}: {error}", entry.path.display()))?;
    if sha256(&bytes) != entry.sha256 {
        return Err(format!("pinned digest differs: {}", entry.path.display()).into());
    }
    Ok(String::from_utf8(bytes)?)
}

/// Generate every output from the manifest's inputs below `root` only.
fn generate(root: &Path, manifest: &Manifest) -> Result<Vec<(Entry, String)>> {
    let read = |role| pinned(root, manifest.entry(role));
    let unicode = read("unicode-data")?;
    let properties = properties::generate(
        &unicode,
        &read("derived-core-properties")?,
        &read("case-folding")?,
        &manifest.version,
    )?;
    let normalization = normalization::generate(&normalization::parse(
        &unicode,
        &read("derived-normalization-props")?,
    )?)?;
    Ok(vec![
        (manifest.entry("properties").clone(), properties),
        (manifest.entry("normalization").clone(), normalization),
    ])
}

/// Compare generated outputs with the files below `tree` and the manifest.
fn differences(tree: &Path, outputs: &[(Entry, String)]) -> Vec<String> {
    let mut problems = Vec::new();
    for (entry, content) in outputs {
        if sha256(content.as_bytes()) != entry.sha256 {
            problems.push(format!(
                "{} differs from its manifest digest",
                entry.path.display()
            ));
        }
        match fs::read(tree.join(&entry.path)) {
            Ok(bytes) if bytes == content.as_bytes() => {}
            Ok(_) => problems.push(format!(
                "{} differs from the generated bytes",
                entry.path.display()
            )),
            Err(error) => problems.push(format!("{}: {error}", entry.path.display())),
        }
    }
    problems
}

fn write_atomically(path: &Path, content: &str) -> Result<()> {
    let pending = path.with_extension("pending");
    fs::write(&pending, content)?;
    fs::rename(pending, path)?;
    Ok(())
}

/// Copy only the manifest and its inputs into an empty directory, generate
/// there, and compare the replayed outputs with the tree and the manifest.
fn replay(root: &Path, manifest: &Manifest, directory: &Path) -> Result<Vec<String>> {
    if fs::read_dir(directory)?.next().is_some() {
        return Err(format!("replay directory is not empty: {}", directory.display()).into());
    }
    for path in std::iter::once(Path::new(MANIFEST))
        .chain(manifest.of(Kind::Input).map(|entry| entry.path.as_path()))
    {
        let target = directory.join(path);
        fs::create_dir_all(
            target
                .parent()
                .expect("invariant: relative file paths have parents"),
        )?;
        fs::copy(root.join(path), target)?;
    }
    let copied = Manifest::parse(&fs::read_to_string(directory.join(MANIFEST))?)?;
    let outputs = generate(directory, &copied)?;
    for (entry, content) in &outputs {
        let target = directory.join(&entry.path);
        fs::create_dir_all(
            target
                .parent()
                .expect("invariant: relative file paths have parents"),
        )?;
        fs::write(target, content)?;
    }
    let mut problems = differences(root, &outputs);
    problems.extend(differences(directory, &outputs));
    Ok(problems)
}

enum Mode {
    Check,
    Write,
    Replay(PathBuf),
}

fn run(root: &Path, mode: Mode) -> Result<String> {
    let manifest = Manifest::parse(&fs::read_to_string(root.join(MANIFEST))?)?;
    for fixture in manifest.of(Kind::Fixture) {
        pinned(root, fixture)?;
    }
    let version = &manifest.version;
    match mode {
        Mode::Check => {
            let problems = differences(root, &generate(root, &manifest)?);
            if !problems.is_empty() {
                return Err(format!(
                    "generated Unicode tables differ; review inputs and explicitly regenerate:\n{}",
                    problems.join("\n")
                )
                .into());
            }
            Ok(format!(
                "ZXS Unicode {version} inputs and generated tables: PASS"
            ))
        }
        Mode::Write => {
            let outputs = generate(root, &manifest)?;
            for (entry, content) in &outputs {
                write_atomically(&root.join(&entry.path), content)?;
            }
            write_atomically(&root.join(MANIFEST), &manifest.with_outputs(&outputs))?;
            Ok(format!("ZXS Unicode {version} tables written"))
        }
        Mode::Replay(directory) => {
            let problems = replay(root, &manifest, &directory)?;
            if !problems.is_empty() {
                return Err(
                    format!("clean-directory replay differs:\n{}", problems.join("\n")).into(),
                );
            }
            Ok(format!(
                "ZXS Unicode {version} clean-directory replay in {}: PASS",
                directory.display()
            ))
        }
    }
}

fn main() -> Result<()> {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut mode = Mode::Check;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--write" => mode = Mode::Write,
            "--check" => mode = Mode::Check,
            "--replay" => {
                mode = Mode::Replay(PathBuf::from(
                    args.next().ok_or("missing replay directory")?,
                ))
            }
            "--product-root" => root = PathBuf::from(args.next().ok_or("missing product root")?),
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }
    println!("{}", run(&root, mode)?);
    Ok(())
}

#[cfg(test)]
#[path = "../../../test/tools/unicode-build/tests.rs"]
mod tests;
