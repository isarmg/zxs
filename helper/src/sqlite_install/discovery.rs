//! Stateless discovery only. No credentials, filesystem inventory, database
//! connection, lifecycle control or authorization capability is produced.
use super::{bad, markers::DiscoveredSystem};
use crate::{
    maintenance::DiscoveryMount,
    pinned::{self, DiscoveryAdmission, DiscoveryPin, Entity, Mount},
};
use linux_raw_sys::general::BTRFS_SUPER_MAGIC;
use std::{
    fmt::Write,
    fs::Metadata,
    io,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::Path,
};

#[derive(Clone, Copy, Eq, PartialEq)]
struct Directory {
    entity: Entity,
    uid: u32,
    gid: u32,
    mode: u32,
    links: u64,
}
impl Directory {
    fn of(m: &Metadata) -> io::Result<Self> {
        Ok(Self {
            entity: Entity::of(m)?,
            uid: m.uid(),
            gid: m.gid(),
            mode: m.mode(),
            links: m.nlink(),
        })
    }
    fn candidate(self) -> bool {
        self.mode == 0o040_700 && self.links != 0 && ![0, u32::MAX].contains(&self.uid)
    }
}
struct Candidate {
    ids: [[u8; 16]; 3],
    mount: u64,
}
fn right_mount(actual: Mount, dir: Directory, magic: u32, row: &DiscoveryMount) -> bool {
    actual.at_root==Some(true)&&actual.id==row.id&&magic==row.magic
        // Btrfs subvolume stat dev can differ from mountinfo's superblock
        // dev. No physical-disk or owned-media conclusion comes from this.
        &&(magic==BTRFS_SUPER_MAGIC||dir.entity.dev==row.device)
}
fn inspect(
    path: &Path,
    automatic: Option<&DiscoveryMount>,
    admit: &dyn DiscoveryAdmission,
) -> io::Result<Option<Candidate>> {
    let pin = DiscoveryPin::select(path, admit)?;
    let mount = pin.require_named()?;
    let start = Directory::of(&pin.metadata()?)?;
    let magic = pin.magic()?;
    if automatic.is_some_and(|row| !right_mount(mount, start, magic, row)) {
        return Err(bad(
            "visible original mount differs from the kernel mount table",
        ));
    }
    if !start.candidate() {
        return Ok(None);
    }
    let dir = pin.directory()?; // original descriptor, read-only/no atime
    let recheck = || -> io::Result<()> {
        pin.require_directory(&dir, mount)?;
        if Directory::of(&dir.metadata()?)? != start
            || pinned::local_data_filesystem(&dir)? != magic
            || Directory::of(&pin.metadata()?)? != start
        {
            return Err(bad("selected directory or mount changed during discovery"));
        }
        Ok(())
    };
    recheck()?;
    let observed = DiscoveredSystem::probe(&dir, start.uid)?;
    if let Some(evidence) = &observed {
        evidence.require(&dir)?;
    }
    recheck()?;
    Ok(observed.map(|evidence| Candidate {
        ids: evidence.coordinates(),
        mount: mount.id,
    }))
}

#[derive(Default)]
struct Report {
    rows: String,
    candidates: usize,
    unknown: usize,
}
fn hex(id: &[u8; 16]) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}
fn escaped(path: &Path) -> String {
    path.as_os_str()
        .as_bytes()
        .iter()
        .copied()
        .flat_map(std::ascii::escape_default)
        .map(char::from)
        .collect()
}
impl Report {
    fn note(&mut self, path: &Path, outcome: io::Result<Option<Candidate>>) {
        match outcome {
            Ok(Some(c)) => {
                self.candidates += 1;
                let [pool, volume, init] = c.ids;
                let _ = writeln!(
                    self.rows,
                    "candidate system=\"{}\" mount={} pool={} volume={} historical_init={}",
                    escaped(path),
                    c.mount,
                    hex(&pool),
                    hex(&volume),
                    hex(&init)
                );
            }
            Ok(None) => (),
            // No raw I/O/path error or candidate content in the report.
            Err(_) => self.unknown += 1,
        }
    }
    fn finish(self, scope: &str) -> (u8, String) {
        let label = match (self.candidates, self.unknown) {
            (2.., _) => "ambiguous",
            (_, 1..) => "incomplete",
            (1, 0) => "candidate-only",
            _ => "none-in-scope",
        };
        (
            if label == "candidate-only" { 0 } else { 2 },
            format!(
                "discovery={label} scope={scope} candidates={} unconfirmed={} database=metadata-only authorization=none\n{}这些不是已验证的原池或离线许可；没有候选不代表未安装。不存路径，也不启动、安装或写入。",
                self.candidates, self.unknown, self.rows
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stateless_discovery_never_resolves_ambiguity_or_fabricates_known_absence() {
        let got = || {
            Ok(Some(Candidate {
                ids: [[1; 16], [2; 16], [3; 16]],
                mount: 17,
            }))
        };
        let mut report = Report::default();
        report.note(Path::new("/mnt/actual\nS"), got());
        report.note(Path::new("/mnt/second"), got());
        let (code, text) = report.finish("mounted-local-filesystem-roots");
        assert_eq!(code, 2);
        assert!(text.starts_with("discovery=ambiguous"));
        assert!(text.contains("/mnt/actual\\nS") && text.contains("authorization=none"));
        let mut report = Report::default();
        report.note(
            Path::new("/mnt/denied"),
            Err(io::ErrorKind::PermissionDenied.into()),
        );
        report.note(Path::new("/mnt/visible"), got());
        assert!(
            report
                .finish("mounts")
                .1
                .starts_with("discovery=incomplete")
        );
        assert!(
            Report::default()
                .finish("mounts")
                .1
                .starts_with("discovery=none-in-scope")
        );
    }
    #[test]
    fn stateless_discovery_mount_ids_and_exact_root_flag_cannot_be_substituted() {
        let row = DiscoveryMount {
            path: "/mnt/a".into(),
            id: 17,
            device: 33,
            magic: linux_raw_sys::general::EXT4_SUPER_MAGIC,
        };
        let real = Mount {
            id: 17,
            at_root: Some(true),
        };
        let dir = Directory {
            entity: Entity { dev: 33, ino: 55 },
            uid: 1000,
            gid: 1000,
            mode: 0o040_700,
            links: 1,
        };
        assert!(right_mount(real, dir, row.magic, &row));
        assert!(!right_mount(Mount { id: 18, ..real }, dir, row.magic, &row));
        assert!(!right_mount(
            Mount {
                at_root: None,
                ..real
            },
            dir,
            row.magic,
            &row
        ));
        assert!(!right_mount(real, dir, BTRFS_SUPER_MAGIC, &row));
    }
    #[test]
    #[ignore = "needs an explicit non-root private test area on the same supported local mount as /"]
    fn stateless_discovery_retains_real_exact_marker_and_metadata_only() {
        use crate::sqlite_install::{DATABASE, MARKER, catalog::claimed_marker};
        use std::{
            fs::{self, File},
            os::unix::fs::PermissionsExt,
        };
        // One explicitly created disposable path, no real mount table
        // enumeration. This fixture authorizes NO mount crossing at all.
        struct Flat {
            fds: pinned::DiscoveryFdInfo,
            id: u64,
        }
        impl DiscoveryAdmission for Flat {
            fn root_mount(&self) -> io::Result<u64> {
                Ok(self.id)
            }
            fn crossing(&self, _: u64, _: &Path) -> io::Result<Option<u64>> {
                Ok(None)
            }
            fn admit(&self, fd: &File) -> io::Result<u64> {
                let id = self.fds.mount(fd)?;
                if id != self.id {
                    return Err(bad("test area must share the one admitted root mount"));
                }
                Ok(id)
            }
        }
        assert!(std::env::var_os("HELPER_TEST_DATA_PARENT").is_some());
        assert!(!rustix::process::getuid().is_root());
        let temp = crate::test_area::data_directory();
        let path = temp.path();
        let fds = pinned::DiscoveryFdInfo::open().unwrap();
        let root = File::from(
            rustix::fs::open(
                c"/",
                rustix::fs::OFlags::PATH | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::empty(),
            )
            .unwrap(),
        );
        let gate = Flat {
            id: fds.mount(&root).unwrap(),
            fds,
        };
        let sample = |p: &Path| inspect(p, None, &gate);
        let db = path.join(DATABASE);
        let mark = path.join(std::ffi::OsStr::from_bytes(MARKER));
        let set = |p: &Path, bytes: &[u8]| {
            fs::write(p, bytes).unwrap();
            fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
        };
        assert!(sample(path).unwrap().is_none());
        set(
            &mark,
            &claimed_marker(2, &[1; 16], &[2; 16], &[3; 16], &[4; 32]),
        );
        assert!(sample(path).unwrap().is_none());
        set(
            &mark,
            &claimed_marker(0, &[1; 16], &[2; 16], &[3; 16], &[4; 32]),
        );
        assert!(sample(path).is_err());
        set(&db, b"intentional non-SQL text: discovery opens no DB");
        set(&path.join("untouched"), b"not a control object");
        assert_eq!(
            sample(path).unwrap().unwrap().ids,
            [[1; 16], [2; 16], [3; 16]]
        );
        fs::rename(&db, path.join("different-name")).unwrap();
        std::os::unix::fs::symlink("different-name", &db).unwrap();
        assert!(sample(path).is_err());
        assert!(sample(&db).is_err());
    }
}
