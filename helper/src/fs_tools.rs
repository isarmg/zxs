//! Guidance and optional inspection of the checker itself, NEVER a runner.
//! A format name is an operator selection, not detection evidence.
mod install;

use std::{ffi::OsString, fmt};
pub(crate) use install::{Evidence, Record, RuntimeFile};

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) enum Target { OfflineImage, IsolatedMembers, ImportedPool }
impl Target {
    fn name(self)->&'static str {match self {
        Self::OfflineImage=>"independent-unmounted-complete-image",
        Self::IsolatedMembers=>"isolated-complete-unmounted-member-set",
        Self::ImportedPool=>"one-explicit-already-imported-pool",
    }}
    fn operand(self)->&'static str{match self{Self::OfflineImage=>"<image>",Self::IsolatedMembers=>"<member>",Self::ImportedPool=>"<pool>"}}
}

#[derive(Clone,Copy)]
pub(crate) struct Profile {
    pub(crate) format:&'static str, pub(crate) package:&'static str,
    pub(crate) directory:&'static str, pub(crate) tool:&'static str,
    pub(crate) options:&'static [&'static str], pub(crate) target:Target, pub(crate) limit:&'static str,
}
const PROFILES:[Profile;7]=[
    Profile{format:"ext4",package:"e2fsprogs",directory:"sbin",tool:"e2fsck",
        options:&["-f","-n"],target:Target::OfflineImage,limit:"unmounted; no external journal for a single-image plan; unresolved journal is not clean"},
    Profile{format:"ntfs",package:"ntfs-3g",directory:"bin",tool:"ntfsfix",
        options:&["-n"],target:Target::OfflineImage,limit:"limited NTFS checks only; full diagnosis/repair belongs to Windows chkdsk"},
    Profile{format:"zfs",package:"zfsutils-linux",directory:"sbin",tool:"zpool",
        options:&["status"],target:Target::ImportedPool,limit:"pool/vdev report, not a partition check; exit zero alone is not healthy; no import or scrub"},
    Profile{format:"xfs",package:"xfsprogs",directory:"sbin",tool:"xfs_repair",
        options:&["-n","-f"],target:Target::OfflineImage,limit:"self-contained image only; external log/realtime device outside scope; dirty log means incomplete, never zero it"},
    Profile{format:"btrfs",package:"btrfs-progs",directory:"bin",tool:"btrfs",
        options:&["check","--readonly"],target:Target::IsolatedMembers,limit:"may look up other devices with the same FSID; an isolated full member set is required, no repair"},
    Profile{format:"exfat",package:"exfatprogs",directory:"sbin",tool:"fsck.exfat",
        options:&["-n"],target:Target::OfflineImage,limit:"confirm exFAT; never apply dosfstools exit meanings"},
    Profile{format:"fat32",package:"dosfstools",directory:"sbin",tool:"fsck.fat",
        options:&["-n"],target:Target::OfflineImage,limit:"confirm FAT32 rather than FAT12/16 or exFAT; no prompt or automatic repair"},
];

pub(crate) fn hex(s:&str)->Option<[u8;32]> {
    if s.len()!=64 || !s.as_bytes().iter().all(u8::is_ascii_hexdigit){return None;}
    let mut out=[0;32];
    for (i,v) in out.iter_mut().enumerate(){*v=u8::from_str_radix(&s[i*2..i*2+2],16).ok()?;}
    Some(out)
}

#[derive(Debug)]
pub(crate) struct Query { selected:Option<usize>, expected:Option<[u8;32]> }
impl Query {
    pub(crate) fn parse(mut args:impl Iterator<Item=OsString>)->Result<Self,&'static str>{
        let Some(raw)=args.next() else {return Ok(Self{selected:None,expected:None});};
        let i=raw.to_str().and_then(|s|PROFILES.iter().position(|p|p.format==s))
            .ok_or("expected one of ext4, ntfs, zfs, xfs, btrfs, exfat, fat32")?;
        let expected=match args.next() {
            None=>None,
            Some(flag) if flag=="--sha256"=>Some(args.next().and_then(|v|hex(v.to_str()?))
                .ok_or("expected exactly 64 hexadecimal characters from an independent trusted digest")?),
            Some(_)=>return Err("expected only --sha256 DIGEST after the format"),
        };
        if args.next().is_some(){return Err("unexpected fs-tools argument; no device, tool path or raw flags accepted");}
        Ok(Self{selected:Some(i),expected})
    }

    pub(crate) fn observe(&self)->Report {
        Report{rows:PROFILES.iter().enumerate().filter(|(i,_)|self.selected.is_none_or(|n|n==*i))
            .map(|(_,profile)|Row{profile:*profile,installed:install::inspect(profile,self.expected.as_ref())}).collect(),
            requested_hash:self.expected.is_some()}
    }
}
pub(crate) struct Row {pub(crate) profile:Profile,pub(crate) installed:Evidence}
pub(crate) struct Report {pub(crate) rows:Vec<Row>,requested_hash:bool}
impl Report {
    pub(crate) fn exit(&self)->u8 {
        if self.rows.iter().any(|r|matches!(r.installed,Evidence::Observed(Record::Different))){return 1;}
        if self.rows.iter().all(|r|matches!(r.installed,Evidence::Observed(_)) ||
            (!self.requested_hash && matches!(r.installed,Evidence::Missing))){0} else {2}
    }
}
impl fmt::Display for Report {
    fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{
        write!(f,"source=fixed-local-install-slots checker=not-executed device=not-opened repair=not-enabled")?;
        for row in &self.rows {
            let p=row.profile;
            write!(f,"\nformat={} package={} tool=/usr/{}/{} slot={} sha256={} version=unverified signature=unverified",
                p.format,p.package,p.directory,p.tool,row.installed.slot(),row.installed.digest())?;
            write!(f,"\n  target={} preview={}",p.target.name(),p.tool)?;
            for opt in p.options{write!(f," {opt}")?;}
            write!(f," {}\n  scope={}",p.target.operand(),p.limit)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path="../tests/fs_tools/profiles.rs"]
mod tests;
