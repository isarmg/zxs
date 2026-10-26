//! Rust-only production builder. JavaScript consists of generated WASM bindings
//! and a generated generic module loader; product behavior lives in Rust.
use anyhow::{Context, Result, bail};
mod bootstrap;
mod wasm_release;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

fn sha256_hex(bytes: impl AsRef<[u8]>) -> String {
    use std::fmt::Write as _;
    let mut hex = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(&mut hex, "{byte:02x}").expect("writing to String cannot fail");
    }
    hex
}

fn gzip_asset(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut gzip = compression::streams::GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), compression::streams::Compression::best());
    gzip.write_all(bytes)?;
    Ok(gzip.finish()?)
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "build".into());
    let root = match args.next().as_deref() {
        Some("--product-root") => {
            PathBuf::from(args.next().context("missing product root")?).canonicalize()?
        }
        Some(value) => bail!("unknown option {value}"),
        None => std::env::current_dir()?.canonicalize()?,
    };
    if args.next().is_some() {
        bail!("unexpected argument");
    }
    let web = root.join("src/web");
    let build = root.join("build");
    fs::create_dir_all(&build)?;
    reject_link(&build)?;
    if !web.join("Cargo.toml").is_file() {
        bail!("product does not contain src/web/Cargo.toml");
    }
    if command == "build" {
        verify_rust_sources(&root)?;
    }
    let lock_path = build.join(".web-build-lock");
    let lock = BuildLock { path:Some(lock_path.clone()), _file:fs::OpenOptions::new().write(true).create_new(true).open(&lock_path).context("another Web build is running (remove a stale build/.web-build-lock only after confirming its process exited)")? };
    let result = build_locked(&root, &web, &build, &command);
    lock.finish(result)?;
    if command == "build" {
        println!(
            "{}",
            json_codec::json!({"event":"built","dist":build.join("web"),"runtime":"web-interface"})
        );
    }
    Ok(())
}

fn build_locked(root: &Path, web: &Path, build: &Path, command: &str) -> Result<()> {
    let inputs_before = source_inventory(root)?;
    let target = std::env::var_os("ZXS_WEB_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target/web"));
    // Keep an explicitly relative output override relative to the caller, even
    // though Cargo itself must run at the inventoried product root.
    let target = std::path::absolute(target)?;
    let mut cargo = Command::new("cargo");
    cargo.arg(match command {
        "build" => "build",
        "check" => "check",
        "test" => "test",
        _ => bail!("usage: web-build build|check|test [--product-root DIR]"),
    });
    cargo
        // Cargo discovers repository configuration and toolchains from its cwd,
        // not from --manifest-path. Use the same root that we inventory.
        .current_dir(root)
        .args(["--locked", "--offline", "--package", "web-app"])
        .arg("--target-dir")
        .arg(&target);
    if command != "test" {
        cargo.args(["--target", "wasm32-unknown-unknown"]);
    } else {
        cargo.args(["--package", "web-interface", "--package", "view-macros"]);
    }
    if command == "build" {
        cargo.arg("--release");
        // Browser delivery optimizes size; the native service retains the root
        // release profile. Both builds share the root workspace and lockfile.
        wasm_release::configure(&mut cargo);
    }
    if !cargo.status().context("cannot run cargo")?.success() {
        bail!("browser Cargo {command} failed");
    }
    if command != "build" {
        return Ok(());
    }
    let bindgen = std::env::var_os("ZXS_WEB_BINDGEN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("wasm-bindgen"));
    let version = Command::new(&bindgen)
        .arg("--version")
        .output()
        .context("wasm-bindgen CLI 0.2.129 is required (ZXS_WEB_BINDGEN may point to it)")?;
    if !version.status.success()
        || String::from_utf8_lossy(&version.stdout).trim() != "wasm-bindgen 0.2.129"
    {
        bail!("wasm-bindgen CLI must be exactly 0.2.129");
    }
    let staging = build.join(".web-staging");
    if staging.exists() {
        reject_link(&staging)?;
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir_all(staging.join("pkg"))?;
    let status = Command::new(bindgen)
        .args(["--target", "web", "--no-typescript", "--out-name", "zxs"])
        .arg("--out-dir")
        .arg(staging.join("pkg"))
        .arg(target.join("wasm32-unknown-unknown/release/web_app.wasm"))
        .status()?;
    if !status.success() {
        bail!("wasm-bindgen failed");
    }
    fs::copy(web.join("styles.css"), staging.join("zxs.css"))?;
    copy_tree(&web.join("public"), &staging)?;
    copy_distribution_material(root, &staging)?;
    fs::write(staging.join("boot.js"), bootstrap::loader())?;
    fs::write(staging.join("index.html"), bootstrap::index())?;
    let wasm = staging.join("pkg/zxs_bg.wasm");
    fs::write(
        wasm.with_extension("wasm.gz"),
        gzip_asset(&fs::read(&wasm)?)?,
    )?;
    verify_rust_sources(root)?;
    let inputs_after = source_inventory(root)?;
    if inputs_before != inputs_after {
        bail!("Web sources changed during compilation; retry with a stable source tree");
    }
    let mut inventory = BTreeMap::new();
    inventory_tree(&staging, &staging, &mut inventory)?;
    fs::write(
        staging.join("build-manifest.json"),
        json_codec::to_vec_pretty(&build_manifest(&inventory, &inputs_after))?,
    )?;
    let dist = build.join("web");
    let backup = build.join(".web-backup");
    if backup.exists() {
        bail!("build/.web-backup needs inspection after an interrupted publication");
    }
    let had_previous = dist.exists();
    if had_previous {
        reject_link(&dist)?;
        if !dist.join("build-manifest.json").is_file() {
            bail!("refusing to replace unrecognized build/web");
        }
        fs::rename(&dist, &backup)?;
    }
    if let Err(error) = fs::rename(&staging, &dist) {
        if had_previous {
            fs::rename(&backup, &dist).context("failed to restore previous Web distribution")?;
        }
        return Err(error.into());
    }
    if had_previous {
        fs::remove_dir_all(&backup)?;
    }
    Ok(())
}

fn verify_rust_sources(root: &Path) -> Result<()> {
    if !Command::new("python3")
        .current_dir(root)
        .args(["tools/rust-sources/manage.py", "verify"])
        .status()
        .context("cannot verify local Rust source inventory")?
        .success()
    {
        bail!("local Rust source verification failed");
    }
    Ok(())
}
fn copy_distribution_material(root: &Path, staging: &Path) -> Result<()> {
    let legal = staging.join("compliance");
    fs::create_dir_all(legal.join("licenses"))?;
    copy_tree(&root.join("compliance/licenses"), &legal.join("licenses"))?;
    // Generated distributions carry the canonical legal bytes and source
    // receipts. Historical development metadata stays in the source archive.
    for relative in [
        "README.md",
        "NOTICE.md",
        "rust-sources.json",
        "provenance/origins.json",
        "provenance/document-migration.json",
        "provenance/shared/vendor/rust-notices/provenance.json",
    ] {
        let output = legal.join(relative);
        fs::create_dir_all(output.parent().context("legal output parent missing")?)?;
        fs::write(output, read_input(&root.join("compliance").join(relative))?)?;
    }
    Ok(())
}

fn build_manifest(
    files: &BTreeMap<String, String>,
    inputs: &BTreeMap<String, String>,
) -> json_codec::Value {
    json_codec::json!({"runtime":"web-interface","files":files,"inputs":inputs,"target":"wasm32-unknown-unknown","wasm_bindgen":"0.2.129"})
}

struct BuildLock {
    path: Option<PathBuf>,
    _file: fs::File,
}
#[derive(Debug)]
struct BuildAndCleanupError {
    build: anyhow::Error,
    cleanup: std::io::Error,
}
impl std::fmt::Display for BuildAndCleanupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}; additionally failed to release Web build lock: {}",
            self.build, self.cleanup
        )
    }
}
impl std::error::Error for BuildAndCleanupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.build.as_ref())
    }
}
impl BuildLock {
    fn finish(mut self, result: Result<()>) -> Result<()> {
        let path = self
            .path
            .take()
            .expect("invariant: build lock is released once");
        match (result, fs::remove_file(path)) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) => Err(error),
            (Ok(()), Err(error)) => Err(error).context("failed to release Web build lock"),
            (Err(build), Err(cleanup)) => Err(BuildAndCleanupError { build, cleanup }.into()),
        }
    }
}
impl Drop for BuildLock {
    fn drop(&mut self) {
        // Unwinding cannot return cleanup failures to main.
        if let Some(path) = self.path.take()
            && let Err(error) = fs::remove_file(&path)
        {
            eprintln!(
                "failed to release Web build lock {} during unwinding: {error}",
                path.display()
            );
        }
    }
}
const INPUT_SPEC_PATH: &str = "tools/web-build/build-inputs.json";
const INPUT_SPEC: &str = include_str!("../build-inputs.json");

fn source_inventory(root: &Path) -> Result<BTreeMap<String, String>> {
    // The release validator consumes this same source-owned specification. Do
    // not accept a stale builder paired with a newer inventory definition.
    if read_input(&root.join(INPUT_SPEC_PATH))? != INPUT_SPEC.as_bytes() {
        bail!("Web input specification changed; rebuild web-build first");
    }
    let spec: json_codec::Value = json_codec::from_str(INPUT_SPEC)?;
    let object = spec
        .as_object()
        .context("Web input specification must be an object")?;
    if object.len() != 2 || !object.contains_key("directories") || !object.contains_key("files") {
        bail!("invalid Web input specification fields");
    }
    let mut seen = BTreeSet::new();
    let mut directories = Vec::new();
    let mut inventory = BTreeMap::new();
    for kind in ["directories", "files"] {
        let paths = spec[kind]
            .as_array()
            .context("Web input paths must be arrays")?;
        for path in paths {
            let path = path.as_str().context("Web input paths must be strings")?;
            if path.is_empty()
                || path.contains(['\\', ':'])
                || path.chars().any(char::is_control)
                || path.split('/').any(|part| matches!(part, "" | "." | ".."))
                || !seen.insert(path)
            {
                bail!("invalid or duplicate Web input path: {path}");
            }
            if directories
                .iter()
                .any(|parent: &&str| path.starts_with(&format!("{parent}/")))
            {
                bail!("overlapping Web input path: {path}");
            }
            if kind == "directories" {
                if seen
                    .iter()
                    .any(|other| *other != path && other.starts_with(&format!("{path}/")))
                {
                    bail!("overlapping Web input directory: {path}");
                }
                directories.push(path);
                inventory_tree(root, &root.join(path), &mut inventory)?;
            } else {
                inventory.insert(path.into(), sha256_hex(read_input(&root.join(path))?));
            }
        }
    }
    if !inventory.contains_key(INPUT_SPEC_PATH) {
        bail!("Web input specification must inventory itself");
    }
    Ok(inventory)
}
fn input_type(path: &Path) -> Result<fs::FileType> {
    // Checking only the final component follows links hidden in configured
    // source-directory parents (for example src/web -> an external checkout).
    // Keep the builder's boundary identical to the release receipt verifier.
    for parent in path.ancestors().skip(1) {
        if parent.as_os_str().is_empty() {
            continue;
        }
        let kind = fs::symlink_metadata(parent)?.file_type();
        if kind.is_symlink() {
            bail!("symbolic links are not build inputs: {}", parent.display());
        }
        if !kind.is_dir() {
            bail!("expected build input directory: {}", parent.display());
        }
    }
    let kind = fs::symlink_metadata(path)?.file_type();
    if kind.is_symlink() {
        bail!("symbolic links are not build inputs: {}", path.display());
    }
    if !kind.is_file() && !kind.is_dir() {
        bail!("special files are not build inputs: {}", path.display());
    }
    Ok(kind)
}
fn reject_link(path: &Path) -> Result<()> {
    input_type(path).map(|_| ())
}
fn read_input(path: &Path) -> Result<Vec<u8>> {
    if !input_type(path)?.is_file() {
        bail!("expected regular build input: {}", path.display());
    }
    Ok(fs::read(path)?)
}
fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    if !input_type(source)?.is_dir() {
        bail!("expected build input directory: {}", source.display());
    }
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let path = entry.path();
        let kind = input_type(&path)?;
        let output = destination.join(entry.file_name());
        if kind.is_dir() {
            fs::create_dir_all(&output)?;
            copy_tree(&path, &output)?;
        } else {
            fs::copy(&path, &output)?;
        }
    }
    Ok(())
}
fn inventory_tree(
    root: &Path,
    directory: &Path,
    output: &mut BTreeMap<String, String>,
) -> Result<()> {
    if !input_type(directory)?.is_dir() {
        bail!("expected inventory directory: {}", directory.display());
    }
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        let relative = path.strip_prefix(root)?;
        let mut parts = Vec::new();
        for part in relative {
            let name = part.to_str().context("non-UTF-8 build input path")?;
            if name.contains(['\\', ':']) || name.chars().any(char::is_control) {
                bail!("unsafe build input path: {}", relative.display());
            }
            parts.push(name);
        }
        if input_type(&path)?.is_dir() {
            inventory_tree(root, &path, output)?;
        } else {
            output.insert(parts.join("/"), sha256_hex(read_input(&path)?));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../test/tools/web-build/unit/main_tests.rs"]
mod tests;
