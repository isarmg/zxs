//! Fixed deployment names. Parser defaults only choose paths; the
//! read-only checks inspect already admitted held directory members before
//! their separate operational gates. No mount, formatting or grant lives here.
use super::BIN;
use crate::{
    maintenance::HostMounts,
    path_rules::Kind,
    pinned::{Entity, Mount, Pinned},
};
use rustix::fs::{Mode, OFlags, RawDir, ResolveFlags, openat2};
use std::{
    ffi::{CStr, OsStr, OsString},
    fs::File,
    io,
    mem::MaybeUninit,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Path, PathBuf},
};

pub(crate) const TOP: &str = "/zxs";
pub(crate) const STATE_COMPONENTS: [&CStr; 2] = [c"zxs", c"state"];
pub(crate) const STATE: &str = "/zxs/state";
pub(crate) const CONTROL_MODE: u32 = 0o755;
pub(crate) const IMAGE: &str = "/zxs/bin/zxs";
pub(crate) const DISKS: &str = "/zxs/disks";
pub(crate) const SYSTEM: &str = "/zxs/disks/system";
pub(crate) const DATA: &str = "/zxs/disks/data";
pub(crate) const RECYCLE: &str = "/zxs/disks/recycle";
const LOG: &str = "/zxs/log";

fn overlaps(path: &Path, reserved: &str) -> bool {
    path.starts_with(reserved) || Path::new(reserved).starts_with(path)
}
fn separated_media(path: &Path) -> bool {
    ![STATE, BIN, LOG].iter().any(|p| overlaps(path, p))
        && ![DISKS, DATA].iter().any(|p| Path::new(p) == path)
}
pub(crate) fn media(path: &Path) -> Result<(), &'static str> {
    Kind::StorageRoot.check_path(path.as_os_str().as_bytes())?;
    if !separated_media(path) {
        return Err(
            "a volume must be separate from protected /zxs state, bin, log and grouping directories",
        );
    }
    Ok(())
}
pub(crate) fn image_separate(path: &Path) -> bool {
    ![STATE, LOG, DISKS].iter().any(|p| overlaps(path, p))
}
pub(crate) fn data_label(label: OsString) -> Result<PathBuf, &'static str> {
    let name = label
        .to_str()
        .filter(|v| {
            !(v.is_empty() || matches!(*v, "." | ".."))
                && v.len() <= 255
                && !v.contains('/')
                && !v.chars().any(char::is_control)
        })
        .ok_or("DATA label must be one nonempty UTF-8 path component")?;
    let p = Path::new(DATA).join(name);
    media(&p)?;
    Ok(p)
}

fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "the original protected /zxs path member or reserved boundary was not verified",
    )
}
fn stamp(dir: &File) -> io::Result<(Entity, Mount, [i64; 4])> {
    let m = dir.metadata()?;
    if !m.is_dir() || m.nlink() == 0 {
        return Err(denied());
    }
    Ok((
        Entity::of(&m)?,
        Mount::on(dir)?.ok_or_else(denied)?,
        [m.mtime(), m.mtime_nsec(), m.ctime(), m.ctime_nsec()],
    ))
}
// Bounded raw-byte membership, not a recursive scan or a name-discovery
// API. The already-admitted original directory is the only data source.
// A successful lookup on a casefold mount is not proof of its spelling.
pub(crate) fn raw_member(
    dir: &File,
    name: &OsStr,
    also: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let original = stamp(dir)?;
    let reader = File::from(openat2(
        dir,
        c".",
        OFlags::RDONLY
            | OFlags::CLOEXEC
            | OFlags::DIRECTORY
            | OFlags::NOFOLLOW
            | OFlags::NOATIME
            | OFlags::NONBLOCK,
        Mode::empty(),
        ResolveFlags::BENEATH
            | ResolveFlags::NO_XDEV
            | ResolveFlags::NO_SYMLINKS
            | ResolveFlags::NO_MAGICLINKS,
    )?);
    if stamp(&reader)? != original {
        return Err(denied());
    }
    let mut buf = [MaybeUninit::uninit(); 8192];
    let mut it = RawDir::new(&reader, &mut buf);
    let names = std::iter::from_fn(|| {
        it.next().map(|row| {
            row.map(|r| r.file_name().to_bytes().to_vec())
                .map_err(Into::into)
        })
    });
    exactly_one_member(names, name.as_bytes())?;
    also()?;
    if stamp(dir)? != original || stamp(&reader)? != original {
        return Err(denied());
    }
    Ok(())
}
fn exactly_one_member(
    names: impl Iterator<Item = io::Result<Vec<u8>>>,
    wanted: &[u8],
) -> io::Result<()> {
    let mut seen = Vec::new();
    let mut bytes = 0usize;
    // Match the real cold consumer: negative on loss of EOF, duplicate,
    // or either bound even if the desired name occurred first.
    for name in names {
        let name = name?;
        if !(1..=255).contains(&name.len()) {
            return Err(denied());
        }
        if matches!(name.as_slice(), b"." | b"..") {
            continue;
        }
        bytes += name.len() + 1;
        if seen.len() == 4096 || bytes > 256 * 1024 {
            return Err(denied());
        }
        seen.push(name);
    }
    seen.sort_unstable();
    if seen.windows(2).any(|w| w[0] == w[1])
        || seen.binary_search_by(|v| v.as_slice().cmp(wanted)).is_err()
    {
        return Err(denied());
    }
    Ok(())
}
// When the actual original /zxs inode appears under a selected whole-mount
// alias, apply exactly the same restrictions to the remainder. Check the
// first name on /zxs and a terminal child of its exact `disks` group; no
// arbitrary old data directory contents are enumerated.
fn media_members(path: &Path, level: usize) -> io::Result<[Option<(usize, &OsStr)>; 2]> {
    let suffix = path
        .components()
        .skip(level + 1)
        .map(std::path::Component::as_os_str)
        .collect::<Vec<_>>();
    let head = *suffix.first().ok_or_else(denied)?;
    let relative = suffix.iter().collect::<PathBuf>();
    if !separated_media(&Path::new(TOP).join(relative)) {
        return Err(denied());
    }
    Ok([
        Some((level, head)),
        (suffix.len() == 2 && head == "disks").then(|| (level + 1, suffix[1])),
    ])
}
pub(crate) fn require_original_media<'a>(
    product: Entity,
    selected: impl IntoIterator<Item = &'a Pinned>,
) -> io::Result<()> {
    let mut view = None;
    for pin in selected {
        let Some(level) = pin.visible_chain().iter().position(|p| *p == product) else {
            continue;
        };
        let members = media_members(pin.path(), level)?;
        if view.is_none() {
            view = Some(HostMounts::discovery_view(false)?);
        }
        pin.same_admitted_chain(
            view.as_ref().expect("one actual local mount cut"),
            |held, i| {
                for &(_, name) in members.iter().flatten().filter(|&&(at, _)| at == i) {
                    raw_member(held, name, || Ok(()))?;
                }
                Ok(())
            },
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn layout_v18_rebases_actual_product_members_and_requires_raw_spelling() {
        let syntax = |p: &str| {
            media_members(Path::new(p), 2).map(|v| v.map(|m| m.map(|(i, n)| (i, n.to_owned()))))
        };
        for path in [
            "/alias/product",
            "/alias/product/state/volume",
            "/alias/product/bin/file",
            "/alias/product/log/v",
            "/alias/product/disks",
            "/alias/product/disks/data",
        ] {
            assert!(syntax(path).is_err(), "{path}");
        }
        assert_eq!(
            syntax("/alias/product/disks/system").unwrap(),
            [Some((2, "disks".into())), Some((3, "system".into()))]
        );
        assert_eq!(
            syntax("/alias/product/disks/data/project").unwrap(),
            [Some((2, "disks".into())), None]
        );
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("disks")).unwrap();
        let original = File::open(tmp.path()).unwrap();
        assert!(raw_member(&original, OsStr::new("disks"), || Ok(())).is_ok());
        assert!(
            raw_member(&original, OsStr::new("DISKS"), || panic!(
                "alias must not reach lookup"
            ))
            .is_err()
        );
    }
    #[test]
    fn layout_v18_raw_spelling_does_not_accept_early_match_before_bounded_complete_eof() {
        let around = |n| {
            [Ok(b".".to_vec()), Ok(b"zxs".to_vec()), Ok(b"..".to_vec())]
                .into_iter()
                .chain((0..n).map(|i| Ok(format!("other-{i}").into_bytes())))
        };
        assert!(exactly_one_member(around(4095), b"zxs").is_ok());
        assert!(exactly_one_member(around(4096), b"zxs").is_err());
        assert!(exactly_one_member(around(2).chain([Ok(b"other-1".to_vec())]), b"zxs").is_err());
        assert!(
            exactly_one_member(
                around(2).chain([Err(io::ErrorKind::Interrupted.into())]),
                b"zxs"
            )
            .is_err()
        );
        let wide = (0..1024).map(|i| Ok(format!("{i:0255}").into_bytes()));
        assert!(exactly_one_member([Ok(b"zxs".to_vec())].into_iter().chain(wide), b"zxs").is_err());
    }
    #[test]
    fn layout_v18_fixed_paths_separate_operands_without_a_legacy_fallback() {
        let mut parent = PathBuf::from("/");
        for part in STATE_COMPONENTS {
            parent.push(std::ffi::OsStr::from_bytes(part.to_bytes()));
        }
        assert_eq!(parent, Path::new(STATE));
        assert_eq!(Path::new(BIN).join("zxs"), Path::new(IMAGE));
        for accepted in [
            SYSTEM,
            RECYCLE,
            "/zxs/disks/data/项目",
            "/another/admitted/pool",
        ] {
            assert!(media(Path::new(accepted)).is_ok(), "{accepted}");
        }
        for reserved in [
            "/",
            "/zxs",
            STATE,
            "/zxs/state/deployment.v1",
            BIN,
            "/zxs/bin/nested",
            LOG,
            "/zxs/log/other",
            DISKS,
            DATA,
        ] {
            assert!(media(Path::new(reserved)).is_err(), "{reserved}");
        }
        for reserved in [
            "/",
            "/zxs",
            STATE,
            "/zxs/state/zxs",
            DISKS,
            "/zxs/disks/data/label/zxs",
            LOG,
        ] {
            assert!(!image_separate(Path::new(reserved)), "{reserved}");
        }
        for accepted in [IMAGE, BIN, "/selected/bin/zxs", "/zxs-other/bin/zxs"] {
            assert!(image_separate(Path::new(accepted)), "{accepted}");
        }
        assert_eq!(
            data_label("资料 1".into()).unwrap(),
            Path::new("/zxs/disks/data/资料 1")
        );
        for label in ["", "/absolute", "a/b", "..", ".", "bad\nname"] {
            assert!(data_label(label.into()).is_err());
        }
    }
}
