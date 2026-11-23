use super::*;
use crate::{
    container_store::{create_file, ensure_dir},
    test_support::PrivateTemp,
};
use std::{fs, path::Path};

struct Site {
    _temp: PrivateTemp,
    system: HeldDir,
    recycle: HeldDir,
    target: DataRoot,
}

const MARK: &CStr = c".zxs-role-v1";

impl Site {
    fn new() -> Self {
        let temp = PrivateTemp::under(&std::env::temp_dir());
        let held = |leaf: &str| {
            let path = temp.0.join(leaf);
            ensure_dir(&path).unwrap();
            HeldDir::trusted_storage(&path).unwrap()
        };
        let system = held("system");
        let recycle = held("recycle");
        let target = DataRoot {
            root: held("data"),
            token: [7; 32],
        };
        drop(system.opaque_owner(c"container-owner").unwrap());
        drop(recycle.opaque_owner(c"container-owner").unwrap());
        for leaf in [c"container-routes", c"container-moves"] {
            system.new_child_directory(leaf).unwrap();
        }
        for leaf in [c"incoming-containers", c"containers"] {
            recycle.new_child_directory(leaf).unwrap();
        }
        create_file(&target.root.path.join(".zxs-role-v1")).unwrap();
        Self {
            _temp: temp,
            system,
            recycle,
            target,
        }
    }

    fn initialize(&self) -> io::Result<()> {
        // Local siblings exercise the format step, not real separate-device
        // admission, the state lease, role marker authentication or the IPC peer.
        initialize_selected(
            &self.system,
            &self.recycle,
            [5; 16],
            &self.target,
            MARK,
            || Ok(()),
        )
    }

    fn seed(&self, n: u8) {
        if n >= 1 {
            drop(self.target.root.opaque_owner(c"owner").unwrap());
        }
        if n >= 2 {
            self.target.root.new_child_directory(c"data").unwrap();
        }
        if n >= 3 {
            self.target.root.new_child_directory(c"staging").unwrap();
        }
    }
}

fn identities(root: &Path) -> Vec<(u64, u64)> {
    ["owner", "data", "staging"]
        .iter()
        .filter_map(|name| {
            let m = fs::symlink_metadata(root.join(name)).ok()?;
            Some((m.dev(), m.ino()))
        })
        .collect()
}

#[test]
fn each_admissible_empty_prefix_resumes_without_replacing_existing_entries() {
    for n in 0..=3 {
        let site = Site::new();
        site.seed(n);
        let previous = identities(&site.target.root.path);
        site.initialize().unwrap();
        let first = identities(&site.target.root.path);
        assert_eq!(first.len(), 3);
        assert_eq!(&first[..previous.len()], previous.as_slice());
        site.initialize().unwrap();
        assert_eq!(first, identities(&site.target.root.path));
    }
}

#[test]
fn unknown_content_bad_prefix_or_history_is_never_cleanup_authority() {
    let site = Site::new();
    create_file(&site.target.root.path.join("original")).unwrap();
    assert!(site.initialize().is_err());
    assert!(site.target.root.path.join("original").exists());
    assert!(!site.target.root.path.join("owner").exists());

    let site = Site::new();
    site.target.root.new_child_directory(c"staging").unwrap();
    assert!(site.initialize().is_err());
    assert!(!site.target.root.path.join("owner").exists());

    let site = Site::new();
    site.seed(3);
    let remnant = site.target.root.path.join("staging/body");
    create_file(&remnant).unwrap();
    assert!(site.initialize().is_err());
    assert!(remnant.exists());

    let site = Site::new();
    let ledger = site.system.new_child_directory(c"retired-data").unwrap();
    let stage = ledger
        .path
        .join(format!(".{}.incoming", hex(site.target.token)));
    create_file(&stage).unwrap();
    assert!(site.initialize().is_err());
    assert!(stage.exists());
    assert!(!site.target.root.path.join("owner").exists());

    let site = Site::new();
    routes::Routes::open(&site.system)
        .unwrap()
        .reserve([2; 16], site.target.token)
        .unwrap();
    assert!(site.initialize().is_err());
    assert!(!site.target.root.path.join("owner").exists());
}

#[test]
fn noncooperating_owner_or_interrupted_parent_cannot_advance_the_next_prefix() {
    let site = Site::new();
    let _lease = site.target.root.opaque_owner(c"owner").unwrap();
    assert!(site.initialize().is_err());
    assert!(!site.target.root.path.join("data").exists());

    let site = Site::new();
    let interrupted = || {
        if site.target.root.path.join("data").exists() {
            Err(io::Error::other("parent unavailable"))
        } else {
            Ok(())
        }
    };
    assert!(
        initialize_selected(
            &site.system,
            &site.recycle,
            [5; 16],
            &site.target,
            MARK,
            interrupted
        )
        .is_err()
    );
    assert!(site.target.root.path.join("data").is_dir());
    assert!(!site.target.root.path.join("staging").exists());
    site.initialize().unwrap();
}
