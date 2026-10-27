//! A real terminal-workspace mirror failure after every publication proof exists.
use std::{
    fs, io,
    path::PathBuf,
    sync::{Arc, Barrier},
    thread::JoinHandle,
};

/// Keep the original manifest bytes; only obstruct its replacement after the
/// engine's existing publication boundary. No production test hook is needed.
pub fn obstruct_terminal_mirror(
    engine: &mut storage_engine::Engine,
    workspace: &str,
) -> JoinHandle<PathBuf> {
    let manifest = engine
        .root()
        .join("workspaces")
        .join(workspace)
        .join("workspace.json");
    let barrier = Arc::new(Barrier::new(2));
    engine.inject_pause_once("after_local_publications", Arc::clone(&barrier));
    std::thread::spawn(move || {
        barrier.wait();
        let preserved = manifest.with_file_name("workspace-before-reconciliation.json");
        let obstruction = (|| -> io::Result<()> {
            fs::rename(&manifest, &preserved)?;
            fs::create_dir(&manifest)
        })();
        // Release the commit worker even if the fixture's filesystem action
        // failed, so its concrete harness error can be reported instead of hung.
        barrier.wait();
        obstruction.unwrap();
        manifest
    })
}

pub fn restore_terminal_mirror(manifest: &std::path::Path) {
    fs::remove_dir(manifest).unwrap();
    fs::rename(
        manifest.with_file_name("workspace-before-reconciliation.json"),
        manifest,
    )
    .unwrap();
}
