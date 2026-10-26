//! Offline configuration drafts only: never opens, initializes or mutates a live pool.
use crate::{Result, print, private_io};
use command_line::Subcommand;
use json_codec::json;
use std::path::{Path, PathBuf};
use storage_engine::{Config, ValidatedConfig};
#[derive(Subcommand)]
pub(crate) enum Command {
    /// Produce an uninitialized draft; does not access or initialize devices.
    Template {
        #[arg(long)]
        production: bool,
        #[arg(long, num_args = 1.., required = true)]
        disks: Vec<PathBuf>,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Edit only an uninitialized draft, with an exact private backup. Not live replacement.
    RegisterDisk {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        slot: String,
        #[arg(long)]
        disk_id: String,
        #[arg(long)]
        path: PathBuf,
        #[arg(long)]
        level: u32,
        #[arg(long)]
        upstream: Option<String>,
        #[arg(long)]
        metadata_member: bool,
    },
}
pub(crate) fn run(command: Command) -> Result<()> {
    match command {
        Command::Template {
            production,
            disks,
            output,
        } => {
            let mut config = Config::development(disks);
            if production {
                config.mode = storage_engine::Mode::Production;
            }
            let config = ValidatedConfig::try_from(config)?;
            if let Some(path) = output {
                if path.to_str().is_none() {
                    return Err("configuration output path must be valid UTF-8".into());
                }
                private_io::write_new(&path, &json_codec::to_vec_pretty(&config)?)?;
                print(json!({"configuration_draft":path,"devices_initialized":false}))
            } else {
                print(config)
            }
        }
        Command::RegisterDisk {
            config,
            slot,
            disk_id,
            path,
            level,
            upstream,
            metadata_member,
        } => {
            // These paths are part of the JSON draft and success receipt.
            // Validate before preserving a backup or changing either file.
            if config.to_str().is_none() || path.to_str().is_none() {
                return Err("configuration and disk paths must be valid UTF-8 for JSON output; the draft was not changed".into());
            }
            let transaction = private_io::DraftTransaction::open(&config)?;
            let backup = register_disk(
                &transaction,
                &config,
                storage_engine::DiskConfig {
                    slot_id: slot.clone(),
                    disk_id,
                    root: path,
                    level,
                    upstream_slot_id: upstream,
                    metadata_member,
                    binding_generation: 1,
                    capacity_bytes: None,
                },
            )?;
            // Durable publication is complete; stdout backpressure must not
            // keep unrelated draft edits in this directory locked out.
            drop(transaction);
            print(
                json!({"slot_id":slot,"draft":config,"backup":backup,"live_binding_changed":false}),
            )
        }
    }
}

/// Keep the stable parent lock alive from the first read through both durable
/// publications. A rejected contender must retry from a fresh draft snapshot.
fn register_disk(
    transaction: &private_io::DraftTransaction,
    config: &Path,
    disk: storage_engine::DiskConfig,
) -> Result<PathBuf> {
    // Match the engine's bounded metadata reader, including whitespace in
    // hand-written drafts. Never read a link or unowned input first.
    const MAX_DRAFT_BYTES: usize = 64 * 1024 * 1024;
    let original = transaction.read_owned_bytes(MAX_DRAFT_BYTES)?;
    let mut draft: Config = storage_engine::decode_strict(&original)?;
    draft.disks.push(disk);
    let draft = ValidatedConfig::try_from(draft)?;
    let candidate = json_codec::to_vec_pretty(&draft)?;
    ensure_uninitialized_draft(transaction, draft)?;
    if candidate.len() > MAX_DRAFT_BYTES {
        return Err("configuration draft exceeds its byte limit".into());
    }
    let backup = registration_backup(config)?;
    transaction
        .write_backup(
            backup.file_name().ok_or("backup file name is required")?,
            &original,
        )
        .map_err(|error| {
            format!(
                "cannot preserve configuration backup {} (the draft was not changed): {error}",
                backup.display()
            )
        })?;
    transaction.replace_private(&candidate)?;
    Ok(backup)
}

/// The selected configuration, not an unrelated --root argument, determines
/// whether this is an offline draft. Stat only: never initialize or lock disks.
fn ensure_uninitialized_draft(
    transaction: &private_io::DraftTransaction,
    config: ValidatedConfig,
) -> Result<()> {
    for marker in [
        "auth.json",
        "management-authority.json",
        "management-auth.json",
        "setup-intent.json",
        "setup-ready.json",
        "writer.lock",
        "index.sqlite",
        "workspaces",
    ] {
        if transaction.entry_exists(std::ffi::OsStr::new(marker))? {
            return Err("disk registration modifies only an uninitialized configuration draft; live migration requires a protected topology plan".into());
        }
    }
    let mut roots = Vec::with_capacity(config.disks.len());
    for disk in &config.disks {
        let root = resolve_draft_disk_root(&disk.root)?;
        // An enrolled disk remains enrolled when the draft was copied outside
        // the system root, or when the new path is nested inside its root.
        for ancestor in root.ancestors() {
            for marker in ["disk.json", ".writer.lock"] {
                reject_initialized_marker(&ancestor.join(marker))?;
            }
        }
        reject_initialized_marker(&root.join("metadata/pool-membership/config.json"))?;
        roots.push(root);
    }
    // Resolve aliases of existing parents before checking the draft roots.
    config.with_disk_roots(roots)?;
    Ok(())
}

fn reject_initialized_marker(path: &Path) -> Result<()> {
    match path.symlink_metadata() {
        Ok(_) => Err("disk registration modifies only an uninitialized configuration draft; live migration requires a protected topology plan".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn resolve_draft_disk_root(path: &Path) -> Result<std::path::PathBuf> {
    let mut existing = path;
    let mut suffix = Vec::new();
    loop {
        match existing.canonicalize() {
            Ok(mut resolved) => {
                if !resolved.is_dir() {
                    return Err("disk root must name a directory".into());
                }
                for name in suffix.into_iter().rev() {
                    resolved.push(name);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // A dangling symlink is not a missing, safely creatable root.
                match existing.symlink_metadata() {
                    Ok(_) => return Err("disk root contains a dangling link".into()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
                suffix.push(
                    existing
                        .file_name()
                        .ok_or("disk root has no existing parent")?,
                );
                existing = existing
                    .parent()
                    .ok_or("disk root has no existing parent")?;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn registration_backup(config: &Path) -> Result<std::path::PathBuf> {
    let mut name = config
        .file_name()
        .ok_or("configuration draft must have a file name")?
        .to_os_string();
    // Preserve the complete name: config.json and config.draft must never
    // share a recovery file. Keep non-UTF8 path bytes intact.
    name.push(".before-registration");
    Ok(config.with_file_name(name))
}

#[cfg(test)]
#[path = "../../../test/rescue/unit/configuration/registration_tests.rs"]
mod registration_tests;
#[cfg(test)]
#[path = "../../../test/rescue/unit/configuration/tests.rs"]
mod tests;
