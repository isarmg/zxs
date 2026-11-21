//! Mount evidence in the current maintenance thread's namespace.
//! Ancestry never searches other paths or recovers a hidden parent. The
//! separate discovery cut returns only bounded, admitted mountpoint names.
use crate::pinned::{DiscoveryAdmission,DiscoveryFdInfo,Entity, Mount, Pinned, invalid, local_data_filesystem};
use linux_raw_sys::general::{BTRFS_SUPER_MAGIC, EXT4_SUPER_MAGIC, F2FS_SUPER_MAGIC, XFS_SUPER_MAGIC};
use rustix::fs::{Mode, OFlags, fgetxattr, makedev, openat};
use rustix::io::Errno;
use std::{collections::BTreeMap, ffi::OsStr, fs::File, io::{self, Read},
    os::unix::{ffi::OsStrExt,fs::MetadataExt},path::{Path,PathBuf}};

const TABLE_BYTES: usize = 4 * 1024 * 1024;
const TABLE_ROWS: usize = 16_384;
const DISCOVERY_ROOTS: usize = 64;
fn unknown(why: &'static str) -> io::Error { io::Error::new(io::ErrorKind::Unsupported, why) }

fn no_claim(dir: &File) -> io::Result<()> {
    let mut v = [0u8; 1];
    match fgetxattr(dir, c"user.zxs.volume", &mut v[..]) {
        Err(Errno::NODATA) => Ok(()),
        Ok(_) | Err(Errno::RANGE) => Err(invalid("selected storage root or its ancestor already has a volume attribute")),
        Err(e) => Err(e.into()),
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct Row { parent: u64, device: u64, whole: bool, devtmpfs: bool, site_level:Option<usize>, magic:Option<u32>, namespace_root:bool }
fn site_level(point:&[u8])->Option<usize>{match point{b"/var"=>Some(0),b"/var/lib"=>Some(1),_=>None}}
fn data_magic(fs:&[u8])->Option<u32>{match fs{
    b"ext2"|b"ext3"|b"ext4"=>Some(EXT4_SUPER_MAGIC),
    b"xfs"=>Some(XFS_SUPER_MAGIC),
    b"f2fs"=>Some(F2FS_SUPER_MAGIC),
    b"btrfs"=>Some(BTRFS_SUPER_MAGIC),
    _=>None,
}}

pub(crate) struct Mounts(BTreeMap<u64, Row>);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DiscoveryMount {
    pub(crate) path: PathBuf,
    pub(crate) id: u64,
    pub(crate) device: u64,
    pub(crate) magic: u32,
}
// mountinfo escapes only whitespace and backslash. Parse byte-for-byte, not
// as shell, URL or Path normalization; SYSTEM's SQL path must also be UTF-8.
fn discovery_path(mut raw: &[u8]) -> Option<PathBuf> {
    let mut name=Vec::new();
    while let Some((&byte,tail))=raw.split_first(){
        if name.len()>=crate::path_rules::MAX_PATH_BYTES{return None;}
        match byte {
            b'\\'=>{
                let code=tail.get(..3)?;
                name.push(match code{b"040"=>b' ',b"011"=>b'\t',b"012"=>b'\n',b"134"=>b'\\',_=>return None});
                raw=&tail[3..];
            }
            b' '|b'\t'|b'\n'|0=>return None,
            v=>{name.push(v);raw=tail;}
        }
    }
    if name.as_slice()!=b"/"{crate::pinned::Kind::StorageRoot.check_path(&name).ok()?;}
    std::str::from_utf8(&name).ok()?;
    Some(PathBuf::from(OsStr::from_bytes(&name)))
}

struct DiscoveryTable{mounts:Mounts,names:BTreeMap<PathBuf,Option<(u64,u64)>>,roots:Vec<DiscoveryMount>}
impl DiscoveryTable{
    fn named(&self,path:&Path,parent:Option<u64>)->io::Result<Option<u64>>{
        let no=||unknown("no unambiguous exact local mount at this discovery crossing");
        let Some(row)=self.names.get(path) else {return Ok(None);};
        let (id,up)=(*row).ok_or_else(no)?;
        if !self.mounts.discovery_ancestry(id)||parent.is_some_and(|p|p!=up){return Err(no());}
        Ok(Some(id))
    }
}
/// One process-local discovery cut; its table cannot be populated by CLI,
/// a file argument, Helper config or an external mount/identity cache.
pub(crate) struct DiscoveryView{table:DiscoveryTable,fds:DiscoveryFdInfo}
impl DiscoveryView{
    pub(crate) fn roots(&self)->&[DiscoveryMount]{&self.table.roots}
    pub(crate) fn same_scope(&self,other:&Self)->bool{
        self.table.names==other.table.names&&self.table.mounts.0==other.table.mounts.0&&self.table.roots==other.table.roots
    }
}
impl DiscoveryAdmission for DiscoveryView{
    fn root_mount(&self)->io::Result<u64>{
        self.table.named(Path::new("/"),None)?.ok_or_else(||unknown("cannot prove a local visible root for discovery"))
    }
    fn crossing(&self,parent:u64,prefix:&Path)->io::Result<Option<u64>>{self.table.named(prefix,Some(parent))}
    fn admit(&self,fd:&File)->io::Result<u64>{
        let id=self.fds.mount(fd)?;
        if !self.table.mounts.discovery_ancestry(id){return Err(unknown("a live discovery path is not on an admitted local mount"));}
        Ok(id)
    }
}

impl Mounts {
    pub(crate) fn current() -> io::Result<Self> {
        let f = super::thread_evidence::mountinfo()?;
        let mut bytes = Vec::new();
        f.take(TABLE_BYTES as u64 + 1).read_to_end(&mut bytes)?;
        Self::parse(&bytes)
    }

    pub(crate) fn discovery_view(automatic:bool) -> io::Result<DiscoveryView> {
        let f=super::thread_evidence::passive_mountinfo()?;
        let mut bytes=Vec::new();
        f.take(TABLE_BYTES as u64+1).read_to_end(&mut bytes)?;
        Ok(DiscoveryView{table:Self::discovery_table(&bytes,automatic)?,fds:DiscoveryFdInfo::open()?})
    }
    #[cfg(test)]
    fn discover(bytes:&[u8])->io::Result<Vec<DiscoveryMount>>{
        Ok(Self::discovery_table(bytes,true)?.roots)
    }
    fn discovery_table(bytes:&[u8],automatic:bool)->io::Result<DiscoveryTable>{
        let mut roots=Vec::new();let mut names=BTreeMap::new();
        let table=Self::parse_each(bytes,|id,row,point|{
            let Some(path)=discovery_path(point)else{return Ok(());};
            names.entry(path.clone()).and_modify(|entry|*entry=None).or_insert(Some((id,row.parent)));
            // No overlay, network, device-free pseudo filesystems, autofs or
            // filesystem subtree binds in automatic discovery. Explicit
            // selection is the only fallback, not a recursive path scan.
            if let Some(magic)=row.magic.filter(|_|automatic&&row.whole&&row.device!=0&&!row.namespace_root) {
                if roots.len()==DISCOVERY_ROOTS{
                    return Err(unknown("too many eligible mounted roots; select one --system directory"));
                }
                roots.push(DiscoveryMount{path,id,device:row.device,magic});
            }
            Ok(())
        })?;
        // A reachable local mount can itself be nested under NFS, autofs or
        // another unsupported parent. Skip these before any pathname walk.
        roots.retain(|r|table.discovery_ancestry(r.id));
        roots.sort_unstable_by(|a,b|(&a.path,a.id).cmp(&(&b.path,b.id)));
        Ok(DiscoveryTable{mounts:table,names,roots})
    }
    fn discovery_ancestry(&self,mut id:u64)->bool{
        for _ in 0..=256{
            let Some(row)=self.0.get(&id).filter(|r|r.magic.is_some()&&r.device!=0) else{return false;};
            if row.namespace_root{return true;}
            if row.parent==id{return false;}
            id=row.parent;
        }
        false
    }

    // mountinfo(5), numeric mount ID, parent, major:minor, then the root
    // *inside* the filesystem. Field five's visible mountpoint is not this root.
    // No stored paths are decoded, logged or subsequently opened.
    fn parse(bytes: &[u8]) -> io::Result<Self> {
        Self::parse_each(bytes,|_,_,_|Ok(()))
    }
    fn parse_each(bytes: &[u8],mut visit:impl FnMut(u64,Row,&[u8])->io::Result<()>) -> io::Result<Self> {
        let bad = || unknown("incomplete, oversized or ambiguous mountinfo");
        if bytes.is_empty() || bytes.len() > TABLE_BYTES || !bytes.ends_with(b"\n") { return Err(bad()); }
        let mut rows = BTreeMap::new();
        fn number(b: &[u8]) -> Option<u64> {
            if b.is_empty() || !b.iter().all(u8::is_ascii_digit) { return None; }
            std::str::from_utf8(b).ok()?.parse().ok()
        }
        for line in bytes[..bytes.len()-1].split(|&c| c == b'\n') {
            if rows.len() >= TABLE_ROWS { return Err(bad()); }
            let mut words = line.split(|&c| c == b' ');
            let id = words.next().and_then(number).filter(|v| *v != 0).ok_or_else(bad)?;
            let parent = words.next().and_then(number).filter(|v| *v != 0).ok_or_else(bad)?;
            let raw = words.next().ok_or_else(bad)?;
            let colon = raw.iter().position(|&c| c == b':').ok_or_else(bad)?;
            let device_number = |b: &[u8]| number(b).and_then(|n| u32::try_from(n).ok()).ok_or_else(bad);
            let device = makedev(device_number(&raw[..colon])?, device_number(&raw[colon+1..])?);
            let root = words.next().filter(|w| w.starts_with(b"/")).ok_or_else(bad)?;
            let point=words.next().filter(|w|w.starts_with(b"/")).ok_or_else(bad)?;
            if !words.next().is_some_and(|w| !w.is_empty()) || !words.any(|w| w == b"-") { return Err(bad()); }
            let fs_type=words.next().filter(|v|!v.is_empty()).ok_or_else(bad)?;
            let row=Row {parent,device,whole:root==b"/",devtmpfs:fs_type==b"devtmpfs",
                site_level:site_level(point),magic:data_magic(fs_type),namespace_root:point==b"/"};
            if !(0..2).all(|_| words.next().is_some_and(|w| !w.is_empty())) || words.next().is_some()
                || rows.insert(id,row).is_some() {
                return Err(bad());
            }
            visit(id,row,point)?;
        }
        Ok(Self(rows))
    }

    /// Numeric superblock matches in this thread's mount namespace. They do
    /// not resolve a stack of devices or prove an unlisted device is unused.
    pub(crate) fn ids_for_device(&self, device:u64) -> impl Iterator<Item=u64> + '_ {
        self.0.iter().filter_map(move |(&id,r)| (r.device==device).then_some(id))
    }

    pub(crate) fn is_full_devtmpfs(&self, mount:u64, device:u64) -> bool {
        self.0.get(&mount).is_some_and(|r| r.whole && r.devtmpfs && r.device==device)
    }

    /// Generic live kernel mount evidence: parent mount ID and this complete
    /// mount's superblock device (field 3, not the name or an inode's st_dev).
    /// Neither field is a persistent medium ID or physical-drive certificate.
    pub(crate) fn full_mount(&self, id: u64) -> io::Result<(u64, u64)> {
        self.0.get(&id).filter(|r| r.whole && r.device != 0).map(|r| (r.parent, r.device))
            .ok_or_else(|| unknown("cannot establish a complete selected mount"))
    }

    /// Only the two fixed, already provisioned OS ancestors of the
    /// dedicated Samba instance may independently change mounts. An
    /// ordinary subtree bind is not a complete root. Btrfs reports an
    /// anon_dev in stat and its superblock device in mountinfo, so use
    /// its original mounted subvolume root inode in that branch instead.
    /// Does not infer the physical drive or distinguish deliberate binds
    /// of an entire independently selected filesystem/subvolume root.
    pub(crate) fn require_instance_parent_mount(&self,level:usize,child:Mount,parent:u64,
        entity:Entity,magic:u32)->io::Result<()>{
        if child.id!=parent && child.at_root==Some(true) && level<2
            && self.0.get(&child.id).is_some_and(|row|row.site_level==Some(level)
                && row.parent==parent && row.magic==Some(magic) && row.device!=0
                && (if magic==BTRFS_SUPER_MAGIC{
                    entity.ino==256 // BTRFS_FIRST_FREE_OBJECTID
                }else{row.whole&&entity.dev==row.device})){
            Ok(())
        }else{
            Err(unknown("the original /var or /var/lib mount transition could not be attributed to a qualified mounted root"))
        }
    }

    fn whole(&self, id: u64, device: u64) -> io::Result<Row> {
        match self.0.get(&id) {
            Some(r) if r.whole && r.device == device => Ok(*r),
            _ => Err(unknown("cannot establish complete filesystem ancestry (subtree mount, subvolume or unknown mount)")),
        }
    }

    fn require_root_boundary(&self, mount: Mount, device: u64) -> io::Result<()> {
        let row = self.whole(mount.id, device)?;
        // A missing parent row is ambiguous: both a chroot at a *whole*
        // mounted filesystem and a normal host with hidden initramfs can have
        // it. Neither is positive evidence that the ascent reached its boundary.
        if mount.at_root == Some(true) && row.parent == mount.id { return Ok(()); }
        Err(unknown("kernel did not positively expose the self-parent mount root"))
    }

    /// Starting with a fresh description of the already checked leaf, walk ..
    /// only over the exact forward-resolved entities. Unreadable, hidden or
    /// reparented ancestry is not treated as an unclaimed original.
    pub(super) fn unclaimed_chain(&self, name: &Pinned, leaf: &File) -> io::Result<Vec<u64>> {
        self.chain(name, leaf, None).map(|(mounts, _)| mounts)
    }

    pub(super) fn pending_chain(&self, name: &Pinned, leaf: &File,
        image: &super::genesis::FreshGenesis<'_>, row: usize) -> io::Result<(Vec<u64>, bool)> {
        self.chain(name, leaf, Some((image, row)))
    }

    fn chain(&self, name: &Pinned, leaf: &File, original: Option<(&super::genesis::FreshGenesis<'_>, usize)>)
        -> io::Result<(Vec<u64>, bool)> {
        let mut mounts = Vec::with_capacity(name.visible_chain().len());
        let mut labelled = false;
        let mut parent_fd: Option<File> = None;
        let mut child: Option<(Mount, Row)> = None;
        let mut expected = name.visible_chain().iter().rev().peekable();
        while let Some(entity) = expected.next() {
            let dir = parent_fd.as_ref().unwrap_or(leaf);
            let m = dir.metadata()?;
            if !m.is_dir() || m.nlink() == 0 || Entity::of(&m)? != *entity {
                return Err(invalid("selected directory ancestry changed"));
            }
            let mount = Mount::on(dir)?.ok_or_else(|| unknown("ancestor has no mount identity"))?;
            let row = self.whole(mount.id, m.dev())?;
            if let Some((c, r)) = child {
                if c.id != mount.id && (c.at_root != Some(true) || r.parent != mount.id) {
                    return Err(unknown("cannot verify a mount transition or its covered parent"));
                }
            }
            // Unknown/union filesystems, denied xattrs and empty markers do not
            // count as unclaimed. No enumeration of ancestor contents is needed.
            local_data_filesystem(dir)?;
            if let Some((image, row)) = original.filter(|_| mounts.is_empty()) {
                // Only the selected leaf may already bear its own original label.
                labelled = super::identity::original_pending_label(dir, image, row)?;
            } else { no_claim(dir)?; }
            mounts.push(mount.id);
            if expected.peek().is_none() {
                self.require_root_boundary(mount, m.dev())?;
                break;
            }
            child = Some((mount, row));
            parent_fd = Some(File::from(openat(dir, c"..", OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty())?));
        }
        Ok((mounts, labelled))
    }
}

#[cfg(test)]
#[path = "../../tests/maintenance/ancestry.rs"]
mod tests;
