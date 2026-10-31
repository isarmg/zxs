//! Test-only Debian server transport. Chrome remains on the host with its sandbox.
use super::{Command, Path, Process, Result, spawn, with_cleanup};
use std::{
    fs, io::Write, net::SocketAddr, os::unix::fs::MetadataExt, path::PathBuf, sync::OnceLock,
};

static IMAGE: OnceLock<String> = OnceLock::new();
pub(super) fn configure(image: Option<String>, embedded: bool) -> Result<()> {
    if let Some(image) = image {
        if !embedded
            || image.is_empty()
            || image.starts_with('-')
            || !image
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._:/@-".contains(&b))
        {
            return Err(
                "server container requires --embedded-web and an explicit image name".into(),
            );
        }
        IMAGE
            .set(image)
            .map_err(|_| "server image already configured")?;
    }
    Ok(())
}

fn docker_control(args: &[String]) -> Command {
    let mut command = Command::new("timeout");
    command
        .args(["--kill-after=5s", "20s", "docker"])
        .args(args);
    command
}

fn output(args: &[String]) -> Result<String> {
    let output = docker_control(args).output()?;
    if !output.status.success() {
        return Err(format!(
            "bounded docker {} failed (status {}): {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let mut text = String::from_utf8(output.stdout)?;
    if args.first().is_some_and(|arg| arg == "logs") {
        text.push_str(&String::from_utf8(output.stderr)?);
    }
    Ok(text)
}
fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|value| (*value).to_owned()).collect()
}
fn cleanup_with(name: &str, mut execute: impl FnMut(&[String]) -> Result<String>) -> Result<()> {
    let list = strings(&[
        "ps",
        "--all",
        "--no-trunc",
        "--filter",
        &format!("id={name}"),
        "--format",
        "{{.ID}}",
    ]);
    let existing = execute(&list)?;
    if existing.trim().is_empty() {
        return Ok(());
    }
    if existing.trim() != name {
        return Err("unexpected container identity during cleanup".into());
    }
    let stopped = execute(&strings(&["stop", "--time", "5", name]));
    let removed = execute(&strings(&["rm", name]));
    let remaining = execute(&list)?;
    if !remaining.trim().is_empty() {
        return Err(format!("server container remains after cleanup: {remaining}").into());
    }
    stopped?;
    removed?;
    Ok(())
}

fn docker_args(
    command: &Command,
    image: &str,
    name: &str,
    uid: u32,
    gid: u32,
) -> Result<Vec<String>> {
    if uid == 0 {
        return Err("container browser fixture must run as a non-root host UID".into());
    }
    let binary = Path::new(command.get_program()).canonicalize()?;
    if !binary.is_file() {
        return Err("server binary is not a regular file".into());
    }
    let root = command
        .get_current_dir()
        .ok_or("server fixture requires an explicit cwd")?
        .canonicalize()?;
    let metadata = root.metadata()?;
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err(format!(
            "fixture root must be a private directory owned by UID {uid}: {} (owner {}, mode {:o})",
            root.display(),
            metadata.uid(),
            metadata.mode() & 0o777
        )
        .into());
    }
    if root.parent().is_none() || root == Path::new("/tmp") || root == Path::new("/work") {
        return Err("server mount must be an individual private fixture".into());
    }
    let args: Vec<String> = command
        .get_args()
        .map(|a| {
            a.to_str()
                .map(str::to_owned)
                .ok_or("non-UTF8 server argument")
        })
        .collect::<std::result::Result<_, _>>()?;
    if args
        .iter()
        .any(|a| a == "--web-dir" || a.starts_with("--web-dir="))
    {
        return Err("container server cannot mount external Web assets".into());
    }
    if args.iter().any(|arg| arg.starts_with("--listen=")) {
        return Err("container server requires the explicit parsed listener form".into());
    }
    let listeners: Vec<_> = args.windows(2).filter(|a| a[0] == "--listen").collect();
    if listeners.len() != 1 {
        return Err("container server requires one explicit loopback listener".into());
    }
    let listen: SocketAddr = listeners[0][1].parse()?;
    if listen.ip() != std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST) || listen.port() == 0 {
        return Err("container server listener must be 127.0.0.1 with a nonzero port".into());
    }
    for path in [&binary, &root] {
        if path.to_str().ok_or("non-UTF8 mount")?.contains([',', '\n']) {
            return Err("invalid bind mount path".into());
        }
    }
    let mut result = strings(&[
        "create",
        "--name",
        name,
        "--network",
        "host",
        "--user",
        &format!("{uid}:{gid}"),
        "--cap-drop=ALL",
        "--security-opt=no-new-privileges",
        "--read-only",
        "--mount",
        &format!("type=bind,src={},dst=/opt/zxs,readonly", binary.display()),
        "--mount",
        &format!("type=bind,src={},dst={}", root.display(), root.display()),
        "--workdir",
        root.to_str().ok_or("non-UTF8 cwd")?,
        "--entrypoint",
        "/opt/zxs",
        image,
    ]);
    result.extend(args);
    Ok(result)
}

enum Backend {
    Native(Option<Process>),
    Container { name: String, log: PathBuf },
}
pub(super) struct ServerProcess {
    backend: Backend,
    stopped: bool,
}
impl ServerProcess {
    fn stop(&mut self) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        match &mut self.backend {
            Backend::Native(process) => {
                drop(process.take());
            }
            Backend::Container { name, log } => {
                let logs = output(&strings(&["logs", name]));
                let cleanup = cleanup_with(name, output);
                if cleanup.is_ok() {
                    self.stopped = true;
                }
                if let Ok(logs) = &logs {
                    fs::OpenOptions::new()
                        .append(true)
                        .open(log)?
                        .write_all(logs.as_bytes())?;
                }
                cleanup?;
                logs?;
            }
        }
        self.stopped = true;
        Ok(())
    }
    pub(super) fn finish(mut self, action: impl FnOnce() -> Result<()>) -> Result<()> {
        with_cleanup(action, || self.stop())
    }
}
impl Drop for ServerProcess {
    fn drop(&mut self) {
        if !self.stopped {
            eprintln!("server fixture fallback cleanup (explicit finish was not reached)");
            if let Err(error) = self.stop() {
                eprintln!("server fixture cleanup FAILED: {error}");
            }
        }
    }
}
pub(super) fn launch(command: &mut Command, log: &Path) -> Result<ServerProcess> {
    let Some(image) = IMAGE.get() else {
        return Ok(ServerProcess {
            backend: Backend::Native(Some(spawn(command, log)?)),
            stopped: false,
        });
    };
    fn id(flag: &str) -> Result<u32> {
        let result = Command::new("id").arg(flag).output()?;
        if !result.status.success() {
            return Err("cannot resolve host fixture identity".into());
        }
        Ok(std::str::from_utf8(&result.stdout)?.trim().parse()?)
    }
    let name = format!(
        "zxs-browser-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    let args = docker_args(command, image, &name, id("-u")?, id("-g")?)?;
    fs::File::create(log)?;
    // A successful create returns immutable ownership identity. A failed
    // create (including a name collision) never authorizes deleting that name.
    let container = output(&args).map_err(|error| {
        format!("docker create did not provide cleanup authority for diagnostic name {name}: {error}; creation outcome and cleanup are unconfirmed, no name-based deletion attempted; isolated runner teardown is required")
    })?;
    let container = container.trim();
    if container.len() != 64 || !container.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("docker create returned no valid complete identity for diagnostic name {name}; cleanup is unconfirmed, no name-based deletion attempted; isolated runner teardown is required").into());
    }
    let process = ServerProcess {
        backend: Backend::Container {
            name: container.to_owned(),
            log: log.to_owned(),
        },
        stopped: false,
    };
    if let Err(error) = output(&strings(&["start", container])) {
        return process.finish(|| Err(error)).map(|()| unreachable!());
    }
    Ok(process)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, Command) {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let binary = root.path().join("zxs");
        fs::write(&binary, b"not executed: command-construction fixture").unwrap();
        let mut command = Command::new(binary);
        command
            .current_dir(root.path())
            .args(["--listen", "127.0.0.1:1234"]);
        (root, command)
    }

    #[test]
    fn container_is_nonroot_loopback_and_has_only_two_explicit_mounts() {
        let (_root, command) = fixture();
        let uid = _root.path().metadata().unwrap().uid();
        let args = docker_args(&command, "zxs-debian13-runtime", "unique", uid, 1002).unwrap();
        assert_eq!(args[0], "create");
        assert!(
            args.windows(2)
                .any(|a| a == ["--user", &format!("{uid}:1002")])
        );
        assert!(args.windows(2).any(|a| a == ["--network", "host"]));
        assert!(args.iter().any(|a| a == "--cap-drop=ALL"));
        assert!(args.iter().any(|a| a == "--security-opt=no-new-privileges"));
        assert_eq!(args.iter().filter(|a| *a == "--mount").count(), 2);
        assert!(args.iter().any(|a| a.ends_with("dst=/opt/zxs,readonly")));
        assert!(
            !args
                .iter()
                .any(|a| a.contains("privileged") || a.contains("no-sandbox"))
        );
    }

    #[test]
    fn root_external_listen_or_web_mount_are_refused() {
        let (_root, mut command) = fixture();
        assert!(docker_args(&command, "image", "id", 0, 0).is_err());
        let mut external = Command::new(command.get_program());
        external
            .current_dir(_root.path())
            .args(["--listen", "0.0.0.0:1234"]);
        command = external;
        assert!(docker_args(&command, "image", "id", 1000, 1000).is_err());
        let (_root, mut command) = fixture();
        command.arg("--listen=0.0.0.0:1234");
        assert!(docker_args(&command, "image", "id", 1000, 1000).is_err());
        let (_root, mut command) = fixture();
        command.args(["--web-dir", "/work/build/web"]);
        assert!(docker_args(&command, "image", "id", 1000, 1000).is_err());
    }

    #[test]
    fn fixture_directory_must_be_private_and_owned_by_host_user() {
        use std::os::unix::fs::PermissionsExt;
        let (root, command) = fixture();
        let uid = root.path().metadata().unwrap().uid();
        assert!(docker_args(&command, "image", "id", uid + 1, 1000).is_err());
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(docker_args(&command, "image", "id", uid, 1000).is_err());
    }

    #[test]
    fn all_docker_control_commands_have_bounded_timeout() {
        let command = docker_control(&strings(&["stop", "owned-id"]));
        assert_eq!(command.get_program(), "timeout");
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(
            args,
            ["--kill-after=5s", "20s", "docker", "stop", "owned-id"]
        );
    }

    #[test]
    fn stop_remove_and_verify_use_exact_owned_id() {
        let id = "a".repeat(64);
        let mut calls = Vec::new();
        cleanup_with(&id, |args| {
            calls.push(args.to_vec());
            Ok(if calls.len() == 1 {
                id.clone()
            } else {
                String::new()
            })
        })
        .unwrap();
        assert_eq!(calls.len(), 4);
        assert_eq!(calls[1], strings(&["stop", "--time", "5", &id]));
        assert_eq!(calls[2], strings(&["rm", &id]));
        assert_eq!(calls[0], calls[3]);
    }

    #[test]
    fn permission_or_remaining_container_cannot_be_a_pass() {
        let id = "b".repeat(64);
        let mut count = 0;
        assert!(
            cleanup_with(&id, |_| {
                count += 1;
                match count {
                    1 => Ok(id.clone()),
                    2 => Err("permission denied".into()),
                    _ => Ok(String::new()),
                }
            })
            .is_err()
        );
        assert_eq!(count, 4); // Still attempts removal and checks actual absence.
        assert!(cleanup_with(&id, |_| Ok(id.clone())).is_err());
        assert!(cleanup_with(&id, |_| Err("docker socket denied".into())).is_err());
    }

    #[test]
    fn failed_acceptance_still_cleans_and_cleanup_failure_fails_success() {
        let mut cleaned = false;
        assert!(
            with_cleanup(
                || Err("assertion failed".into()),
                || {
                    cleaned = true;
                    Ok(())
                }
            )
            .is_err()
        );
        assert!(cleaned);
        assert!(with_cleanup(|| Ok(()), || Err("cleanup denied".into())).is_err());
    }
}
