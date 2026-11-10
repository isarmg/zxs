use super::*;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

#[test]
fn one_inode_excludes_another_owner_and_alias_until_guard_drop() {
    let root = tempfile::tempdir().unwrap();
    let uid = rustix::process::geteuid().as_raw();
    let owner = ServiceLease::acquire(root.path(), uid).unwrap();
    assert!(matches!(
        ServiceLease::acquire(root.path(), uid),
        Err(Error::Lock { .. })
    ));
    let alias = root.path().join(".");
    assert!(ServiceLease::acquire(&alias, uid).is_err());
    owner.recheck().unwrap();
    drop(owner);
    ServiceLease::acquire(root.path(), uid).unwrap();
}
#[test]
fn missing_linked_wrong_owner_and_writable_roots_are_rejected_without_creation() {
    let parent = tempfile::tempdir().unwrap();
    let uid = rustix::process::geteuid().as_raw();
    let root = parent.path().join("root");
    assert!(ServiceLease::acquire(&root, uid).is_err());
    assert!(!root.exists());
    fs::create_dir(&root).unwrap();
    assert!(ServiceLease::acquire(&root, uid.wrapping_add(1)).is_err());
    let linked = parent.path().join("linked");
    symlink(&root, &linked).unwrap();
    assert!(ServiceLease::acquire(&linked, uid).is_err());
    fs::set_permissions(&root, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(matches!(
        ServiceLease::acquire(&root, uid),
        Err(Error::Permissions { .. })
    ));
}
#[test]
fn path_replacement_does_not_silently_transfer_the_held_inode_proof() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("root");
    fs::create_dir(&root).unwrap();
    let uid = rustix::process::geteuid().as_raw();
    let guard = ServiceLease::acquire(&root, uid).unwrap();
    fs::rename(&root, parent.path().join("retained")).unwrap();
    fs::create_dir(&root).unwrap();
    assert!(matches!(
        guard.recheck(),
        Err(Error::IdentityChanged { .. })
    ));
}

#[test]
fn cloned_lease_does_not_unlock_the_remaining_owner() {
    let root = tempfile::tempdir().unwrap();
    let uid = rustix::process::geteuid().as_raw();
    let owner = ServiceLease::acquire(root.path(), uid).unwrap();
    let remaining = owner.clone();
    drop(owner);
    assert!(matches!(
        ServiceLease::acquire(root.path(), uid),
        Err(Error::Lock { .. })
    ));
    remaining.recheck().unwrap();
    drop(remaining);
    ServiceLease::acquire(root.path(), uid).unwrap();
}

#[test]
fn published_name_keeps_the_same_inode_locked() {
    let parent = tempfile::tempdir().unwrap();
    let pending = parent.path().join("pending");
    let published = parent.path().join("published");
    fs::create_dir(&pending).unwrap();
    let uid = rustix::process::geteuid().as_raw();
    let owner = ServiceLease::acquire(&pending, uid).unwrap();
    let identity = owner.identity();
    fs::rename(&pending, &published).unwrap();
    let owner = owner.published_at(&published).unwrap();
    assert_eq!(owner.identity(), identity);
    assert!(matches!(
        ServiceLease::acquire(&published, uid),
        Err(Error::Lock { .. })
    ));
    drop(owner);
    ServiceLease::acquire(&published, uid).unwrap();
}

fn child(mode: &str, root: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "service_lease::tests::process_lease_child",
            "--nocapture",
        ])
        .env("ZXS_SERVICE_LEASE_TEST_CHILD", mode)
        .env("ZXS_SERVICE_LEASE_TEST_ROOT", root);
    command
}

fn wait_ready(output: &mut impl std::io::BufRead) {
    let mut line = String::new();
    loop {
        assert_ne!(
            output.read_line(&mut line).unwrap(),
            0,
            "child exited before admission"
        );
        if line.contains("lease-ready") {
            break;
        }
        line.clear();
    }
}

fn exit_when_requested() {
    use std::io::{BufRead, Write};
    println!("lease-ready");
    std::io::stdout().flush().unwrap();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).unwrap();
    assert_eq!(line, "exit\n");
}

#[test]
fn admitted_root_replacement_is_rejected_before_sharing() {
    let parent = tempfile::tempdir().unwrap();
    let host = parent.path().join("host");
    fs::create_dir(&host).unwrap();
    assert!(child("replacement", &host).status().unwrap().success());
}

#[test]
fn entered_scope_reuses_the_owner_without_holding_it_forever() {
    let root = tempfile::tempdir().unwrap();
    assert!(child("scope", root.path()).status().unwrap().success());
}

#[test]
fn two_internal_hops_keep_the_lease_after_prior_owners_exit() {
    use std::io::Write;
    use std::process::Stdio;
    let root = tempfile::tempdir().unwrap();
    let uid = rustix::process::geteuid().as_raw();
    let owner = ServiceLease::acquire(root.path(), uid).unwrap();
    let mut command = child("relay", root.path());
    command.stdin(Stdio::piped()).stdout(Stdio::piped());
    let fd = owner.inherit_to(&mut command).unwrap();
    command.env("ZXS_SERVICE_LEASE_TEST_FD", fd.to_string());
    let mut child = command.spawn().unwrap();
    assert!(
        rustix::io::fcntl_getfd(owner.root.directory())
            .unwrap()
            .contains(rustix::io::FdFlags::CLOEXEC)
    );
    drop(command);
    let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
    wait_ready(&mut output);
    drop(owner);
    assert!(matches!(
        ServiceLease::acquire(root.path(), uid),
        Err(Error::Lock { .. })
    ));
    child.stdin.take().unwrap().write_all(b"exit\n").unwrap();
    std::io::copy(&mut output, &mut std::io::sink()).unwrap();
    assert!(child.wait().unwrap().success());
    ServiceLease::acquire(root.path(), uid).unwrap();
}

#[test]
fn ordinary_exec_does_not_receive_the_lease_descriptor() {
    let root = tempfile::tempdir().unwrap();
    let owner = ServiceLease::acquire(root.path(), rustix::process::geteuid().as_raw()).unwrap();
    let descriptor = format!("/proc/self/fd/{}", owner.root.directory().as_raw_fd());
    assert!(
        std::process::Command::new("/bin/sh")
            .args(["-c", "test ! -e \"$1\"", "lease-check", &descriptor])
            .status()
            .unwrap()
            .success()
    );
    owner.recheck().unwrap();
}

#[test]
#[allow(unsafe_code)]
fn inherited_descriptor_rejects_standard_io_and_invalid_numbers() {
    // SAFETY: these numbers transfer no owned Rust descriptor; invalid numbers
    // and standard I/O are rejected before File construction.
    unsafe {
        assert!(matches!(
            ServiceLease::receive_file(-1),
            Err(Error::Inheritance(_))
        ));
        assert!(matches!(
            ServiceLease::receive_file(0),
            Err(Error::Inheritance(_))
        ));
        assert!(matches!(
            ServiceLease::receive_file(i32::MAX),
            Err(Error::Inheritance(_))
        ));
    }
}

#[test]
#[allow(unsafe_code)]
fn process_lease_child() {
    use std::io::Write;
    let Ok(mode) = std::env::var("ZXS_SERVICE_LEASE_TEST_CHILD") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("ZXS_SERVICE_LEASE_TEST_ROOT").unwrap());
    let uid = rustix::process::geteuid().as_raw();
    if mode == "scope" {
        let owner = ServiceLease::acquire(&root, uid).unwrap().enter().unwrap();
        let shared = ServiceLease::acquire(&root, uid).unwrap();
        assert!(Arc::ptr_eq(&owner.root, &shared.root));
        assert!(matches!(owner.clone().enter(), Err(Error::AlreadyEntered)));
        let bundle = crate::publication::BundleDestination::open(&root).unwrap();
        bundle.publish("record", &[("intent", b"exact")]).unwrap();
        drop(bundle);
        let independent = OpenAt2Root::open(&root, MountPolicy::AllowNestedMounts).unwrap();
        assert!(independent.directory().try_lock().is_err());
        drop(owner);
        shared.recheck().unwrap();
        drop(shared);
        independent.directory().try_lock().unwrap();
        independent.directory().unlock().unwrap();
        assert!(ServiceLease::current().is_none());
        return;
    }
    if mode == "replacement" {
        let owner = ServiceLease::acquire(&root, uid).unwrap().enter().unwrap();
        fs::rename(&root, root.with_file_name("retained")).unwrap();
        fs::create_dir(&root).unwrap();
        assert!(matches!(
            ServiceLease::acquire(&root, uid),
            Err(Error::IdentityChanged { .. })
        ));
        assert!(crate::publication::BundleDestination::open(&root).is_err());
        drop(owner);
        return;
    }
    assert!(mode == "inherited" || mode == "relay");
    let fd = std::env::var("ZXS_SERVICE_LEASE_TEST_FD")
        .unwrap()
        .parse()
        .unwrap();
    // SAFETY: the parent deliberately inherited this sole descriptor into this
    // fresh test process; no File or borrowed handle has been constructed for it.
    let file = unsafe { ServiceLease::receive_file(fd) }.unwrap();
    assert!(
        rustix::io::fcntl_getfd(&file)
            .unwrap()
            .contains(rustix::io::FdFlags::CLOEXEC)
    );
    let directory = OpenAt2Root::from_directory(file, MountPolicy::AllowNestedMounts).unwrap();
    let owner = ServiceLease::acquire_directory(&root, uid, directory)
        .unwrap()
        .enter()
        .unwrap();
    if mode == "relay" {
        let mut command = child("inherited", &root);
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped());
        let fd = owner.inherit_to(&mut command).unwrap();
        command.env("ZXS_SERVICE_LEASE_TEST_FD", fd.to_string());
        let mut child = command.spawn().unwrap();
        drop(command);
        let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
        wait_ready(&mut output);
        drop(owner);
        exit_when_requested();
        child.stdin.take().unwrap().write_all(b"exit\n").unwrap();
        std::io::copy(&mut output, &mut std::io::sink()).unwrap();
        assert!(child.wait().unwrap().success());
        return;
    }
    exit_when_requested();
    drop(owner);
}
