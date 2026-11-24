//! One authorized offline DATA move under held STATE and role authority.
mod records;
mod source;

#[cfg(target_arch = "x86_64")]
mod entry;
#[cfg(target_arch = "x86_64")]
pub(crate) use entry::{
    MoveRequest, MoveRoles, execute_move, require_move_absent, require_move_scope,
    require_stage_absent,
};
#[cfg(target_arch = "x86_64")]
pub(crate) use records::Completion;

use super::{Birth, Checked, Digest, Guard, Intent, Scope, digest, invalid, proof};
use crate::{
    container_store::{DataRoot, hex, parse_container_id},
    fs::{HeldDir, OpaquePin, ReadAccess, local_uid},
};
use source::Source;
use std::{
    ffi::{CStr, CString, c_char, c_int, c_uint},
    fs::File,
    io,
    os::{
        fd::AsRawFd,
        unix::{
            ffi::OsStrExt,
            fs::{FileExt, MetadataExt},
        },
    },
};

const ACCESS: ReadAccess = ReadAccess::NoAtime;
const OPERAND_DOMAIN: &[u8] = b"ZXS disk move operand v1\0";
const ACTIVE_PREFIX: &str = ".server-stage-move-";
const MOVE_PARENT: &CStr = c"data-relocations";
const MAX_PRIVATE_ENTRIES: usize = 1 << 17;

unsafe extern "C" {
    fn zxs_upload_birth(fd: c_int, inode: *mut u64, seconds: *mut i64, nanos: *mut u32) -> c_int;
    fn renameat2(
        from: c_int,
        source: *const c_char,
        into: c_int,
        target: *const c_char,
        flags: c_uint,
    ) -> c_int;
}

struct Roots<'a> {
    state: &'a HeldDir,
    system: &'a HeldDir,
    recycle: &'a HeldDir,
    data: (&'a DataRoot, &'a DataRoot),
}

struct Selected<'a> {
    scope: Scope,
    system_token: Digest,
    operands: (Digest, Digest),
    roots: Roots<'a>,
    outer: &'a dyn Fn() -> io::Result<()>,
    pulse: &'a dyn Fn() -> io::Result<()>,
    leases: [Lease<'a>; 4],
    routes: HeldDir,
    original_data: HeldDir,
    target_data: HeldDir,
    target_staging: HeldDir,
    moves: HeldDir,
    incoming_r: HeldDir,
    arrived_r: HeldDir,
    names: Names,
}

struct Names {
    cid: CString,
    operation: CString,
    route_stage: CString,
    target_stage: CString,
    inactive: CString,
    active: CString,
}

impl Names {
    fn new(scope: Scope) -> io::Result<Self> {
        if !scope.valid() {
            return Err(invalid("invalid original DATA move scope"));
        }
        let cid = hex(scope.container);
        let pair = format!("{cid}-{}", hex(scope.operation));
        Ok(Self {
            cid: native_name(&cid)?,
            operation: native_name(&pair)?,
            route_stage: native_name(&format!(".{cid}.incoming"))?,
            target_stage: native_name(&format!("move-{pair}"))?,
            inactive: native_name(&format!(".move-guard-draft-{pair}"))?,
            active: native_name(&format!("{ACTIVE_PREFIX}{pair}"))?,
        })
    }
}

struct Lease<'a> {
    root: &'a HeldDir,
    name: &'static CStr,
    original: File,
}

impl<'a> Lease<'a> {
    fn hold(root: &'a HeldDir, name: &'static CStr) -> io::Result<Self> {
        // Opening an existing owner must not sync the whole role before
        // the selected operation has even proved its record and native cut.
        let original = pin(root, name)?.file;
        original.try_lock().map_err(io::Error::from)?;
        let lease = Self {
            root,
            name,
            original,
        };
        lease.require()?;
        Ok(lease)
    }

    fn require(&self) -> io::Result<()> {
        let named = pin(self.root, self.name)?;
        let observed = named.file.metadata()?;
        if observed.len() != 0 {
            return Err(invalid("an offline DATA move owner ceased to be empty"));
        }
        same_file(self.root, &self.original, &named.file)?;
        named.require(self.root, self.name)
    }
}

impl<'a> Selected<'a> {
    #[cfg(test)]
    fn retain(
        roots: Roots<'a>,
        identity: (Scope, Digest, (Digest, Digest)),
        outer: &'a dyn Fn() -> io::Result<()>,
    ) -> io::Result<Self> {
        Self::retain_live(roots, identity, outer, outer)
    }

    fn retain_live(
        roots: Roots<'a>,
        (scope, system_token, operands): (Scope, Digest, (Digest, Digest)),
        outer: &'a dyn Fn() -> io::Result<()>,
        pulse: &'a dyn Fn() -> io::Result<()>,
    ) -> io::Result<Self> {
        let (source, target) = roots.data;
        let roster = [
            roots.state,
            roots.system,
            roots.recycle,
            &source.root,
            &target.root,
        ];
        outer()?;
        role_roots(roster)?;
        Self::retain_names_live(roots, (scope, system_token, operands), outer, pulse)
    }

    // These narrow fixtures exercise the native cuts on two temporary filesystems.
    #[cfg(test)]
    fn retain_names(
        roots: Roots<'a>,
        identity: (Scope, Digest, (Digest, Digest)),
        outer: &'a dyn Fn() -> io::Result<()>,
    ) -> io::Result<Self> {
        Self::retain_names_live(roots, identity, outer, outer)
    }

    fn retain_names_live(
        roots: Roots<'a>,
        (scope, system_token, operands): (Scope, Digest, (Digest, Digest)),
        outer: &'a dyn Fn() -> io::Result<()>,
        pulse: &'a dyn Fn() -> io::Result<()>,
    ) -> io::Result<Self> {
        outer()?;
        let (source, target) = roots.data;
        let names = Names::new(scope)?;
        let tokens = [system_token, source.token, target.token];
        if tokens.contains(&[0; 32])
            || tokens[0] == tokens[1]
            || tokens[0] == tokens[2]
            || tokens[1] == tokens[2]
            || (operand(&source.root)?, operand(&target.root)?) != operands
            || operands.0 == operands.1
        {
            return Err(invalid(
                "the original DATA move role or operand differs from its selected scope",
            ));
        }
        let leases = [
            Lease::hold(roots.system, c"container-owner")?,
            Lease::hold(roots.recycle, c"container-owner")?,
            Lease::hold(&source.root, c"owner")?,
            Lease::hold(&target.root, c"owner")?,
        ];
        // Existing-only. The standard Store open/sync/sweep is not used.
        let routes = child(roots.system, c"container-routes")?;
        let moves = child(roots.system, c"container-moves")?;
        let incoming_r = child(roots.recycle, c"incoming-containers")?;
        let arrived_r = child(roots.recycle, c"containers")?;
        let original_data = child(&source.root, c"data")?;
        let target_data = child(&target.root, c"data")?;
        let target_staging = child(&target.root, c"staging")?;
        let selected = Self {
            scope,
            system_token,
            operands,
            roots,
            outer,
            pulse,
            leases,
            routes,
            original_data,
            target_data,
            target_staging,
            moves,
            incoming_r,
            arrived_r,
            names,
        };
        selected.require()?;
        Ok(selected)
    }

    fn require(&self) -> io::Result<()> {
        (self.outer)()?;
        self.require_local()
    }

    fn require_local(&self) -> io::Result<()> {
        self.roots.state.require_named()?;
        for lease in &self.leases {
            lease.require()?;
        }
        for directory in [
            &self.routes,
            &self.original_data,
            &self.target_data,
            &self.target_staging,
            &self.moves,
            &self.incoming_r,
            &self.arrived_r,
        ] {
            directory.require_named()?;
        }
        Ok(())
    }

    fn require_own_guard_only(&self) -> io::Result<()> {
        let mut remaining = MAX_PRIVATE_ENTRIES;
        self.roots.state.each_name(ACCESS, |name| {
            remaining = remaining
                .checked_sub(1)
                .ok_or_else(|| invalid("the original STATE inventory exceeds its bound"))?;
            if name.starts_with(ACTIVE_PREFIX.as_bytes()) && name != self.names.active.to_bytes() {
                return Err(invalid("another active or unknown DATA move guard exists"));
            }
            Ok(())
        })
    }

    fn pre_ready_names(&self) -> io::Result<()> {
        self.require()?;
        self.require_own_guard_only()?;
        let (source, target) = self.roots.data;
        if let Some(retired) = self.roots.system.child_directory(c"retired-data")? {
            for token in [source.token, target.token] {
                let suffix = hex(token);
                absent(&retired, &native_name(&suffix)?)?;
                absent(&retired, &native_name(&format!(".{suffix}.incoming"))?)?;
            }
            retired.require_named()?;
        }
        if let Some(uploads) = self.roots.system.child_directory(c"upload-intents")? {
            absent(&uploads, &self.names.cid)?;
            let cid = hex(self.scope.container);
            for slot in 0..16 {
                absent(
                    &uploads,
                    &native_name(&format!(".{cid}-0{slot:x}.incoming"))?,
                )?;
            }
            uploads.require_named()?;
        }
        absent(self.roots.state, c".server-stage-retire-data")?;
        absent(&self.moves, &self.names.cid)?;
        self.incoming_r.each_name(ACCESS, |_| {
            Err(invalid("unattributed RECYCLE staging is retained"))
        })?;
        let mut remaining = MAX_PRIVATE_ENTRIES;
        self.arrived_r.each_name(ACCESS, |name| {
            remaining = remaining
                .checked_sub(1)
                .ok_or_else(|| invalid("RECYCLE inventory exceeds the pre-READY bound"))?;
            let (cid, suffix) = name
                .split_at_checked(32)
                .ok_or_else(|| invalid("unknown RECYCLE envelope"))?;
            let request = suffix
                .strip_prefix(b"-")
                .ok_or_else(|| invalid("unknown RECYCLE envelope"))?;
            parse_container_id(request)?;
            if parse_container_id(cid)? == self.scope.container {
                return Err(invalid("a same-container RECYCLE envelope is retained"));
            }
            Ok(())
        })?;
        self.require()
    }

    fn vacancies(&self, durable: bool) -> io::Result<()> {
        self.vacancies_for_copy(durable, false)
    }

    fn vacancies_for_copy(&self, durable: bool, resuming: bool) -> io::Result<()> {
        let positions = [
            (&self.routes, &self.names.route_stage),
            (&self.target_data, &self.names.cid),
            (&self.target_staging, &self.names.target_stage),
        ];
        let positions = &positions[..if resuming { 2 } else { 3 }];
        if durable {
            for &(parent, name) in positions {
                // This flushes only the exact held parents. No DATA body,
                // name, stage or CR1 is created, removed or replaced here.
                parent.confirm_absence(name)?;
            }
        }
        for &(parent, name) in positions {
            absent(parent, name)?;
        }
        Ok(())
    }

    fn observe_source(&self) -> io::Result<Source> {
        Source::capture(
            &self.original_data,
            self.scope,
            self.roots.data.0.token,
            self.pulse,
        )
    }
}

fn role_roots(ordered: [&HeldDir; 5]) -> io::Result<()> {
    for (i, at) in ordered.iter().enumerate() {
        at.require_named()?;
        let metadata = at.file.metadata()?;
        if metadata.mode() & 0o7777 != 0o700
            || metadata.uid() != local_uid()
            || ordered[..i].iter().any(|old| {
                old.entity == at.entity
                    || old.path.starts_with(&at.path)
                    || at.path.starts_with(&old.path)
            })
        {
            return Err(invalid(
                "offline DATA move held roles overlap or are not private",
            ));
        }
    }
    let separate = |a: &HeldDir, b: &HeldDir| {
        Ok::<_, io::Error>(a.entity.device != b.entity.device && !a.same_mount(b)?)
    };
    let [_, system, recycle, from, to] = ordered;
    if !separate(from, to)?
        || !separate(system, recycle)?
        || !separate(recycle, from)?
        || !separate(recycle, to)?
    {
        return Err(invalid(
            "the involved DATA and RECYCLE roles are not distinct filesystems",
        ));
    }
    Ok(())
}

fn operand(role: &HeldDir) -> io::Result<Digest> {
    let lexical = role.path.as_os_str().as_bytes();
    let length =
        u16::try_from(lexical.len()).map_err(|_| invalid("DATA move operand is too long"))?;
    let mut hash = crate::hash::Sha256::new();
    hash.update(OPERAND_DOMAIN)?;
    hash.update(&length.to_be_bytes())?;
    hash.update(lexical)?;
    Ok(hash.finish())
}

fn birth(file: &File) -> io::Result<Birth> {
    let mut out = Birth {
        inode: 0,
        seconds: 0,
        nanos: 0,
    };
    // SAFETY: the original file and exactly typed destination fields remain live.
    if unsafe {
        zxs_upload_birth(
            file.as_raw_fd(),
            &raw mut out.inode,
            &raw mut out.seconds,
            &raw mut out.nanos,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    if !out.valid() || out.inode != file.metadata()?.ino() {
        return Err(invalid("unproved native DATA move file incarnation"));
    }
    Ok(out)
}

fn native_name(value: &str) -> io::Result<CString> {
    CString::new(value).map_err(|_| invalid("invalid exact DATA move native name"))
}

fn child(parent: &HeldDir, name: &CStr) -> io::Result<HeldDir> {
    parent
        .child_directory(name)?
        .ok_or_else(|| invalid("required original DATA move directory is absent"))
}

fn absent(parent: &HeldDir, name: &CStr) -> io::Result<()> {
    parent.require_named()?;
    if parent.pin_record(name)?.is_some() {
        return Err(invalid("a private DATA move position is already occupied"));
    }
    parent.require_named()
}

fn pin(parent: &HeldDir, name: &CStr) -> io::Result<OpaquePin> {
    parent
        .opaque_pin_with(name, ACCESS)?
        .ok_or_else(|| invalid("required DATA move record is absent"))
}

fn same_file(parent: &HeldDir, original: &File, observed: &File) -> io::Result<()> {
    let old = original.metadata()?;
    let new = observed.metadata()?;
    if (old.dev(), old.ino()) != (new.dev(), new.ino())
        || !parent.shares_file_mount(original)?
        || !parent.shares_file_mount(observed)?
    {
        return Err(invalid("a named DATA move file was replaced"));
    }
    parent.require_named()
}

fn read_raw(pin: &OpaquePin, length: usize) -> io::Result<Vec<u8>> {
    let extent = u64::try_from(length).map_err(|_| invalid("invalid DATA move record extent"))?;
    if pin.file.metadata()?.len() != extent {
        return Err(invalid(
            "DATA move record extent differs from its exact expected size",
        ));
    }
    let mut bytes = vec![0; length];
    pin.file.read_exact_at(&mut bytes, 0)?;
    if pin.file.read_at(&mut [0; 1], extent)? != 0 {
        return Err(invalid("DATA move record has trailing bytes"));
    }
    Ok(bytes)
}

fn no_replace(parent: &HeldDir, source: &CStr, destination: &CStr) -> io::Result<()> {
    parent.checked_child_path(source)?;
    parent.checked_child_path(destination)?;
    let fd = parent.file.as_raw_fd();
    // SAFETY: held original same parent, both checked NUL-terminated component names.
    if unsafe { renameat2(fd, source.as_ptr(), fd, destination.as_ptr(), 1) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
