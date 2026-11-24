//! Small immutable phase records must never be visible under their
//! authoritative name as an empty or partly written file.
use super::super::{ACCESS, native_name, renameat2};
use super::{BoundJournal, HeldDir, OpaquePin, absent, create, invalid, pin, read_raw, same_file};
use std::{ffi::CStr, io, os::fd::AsRawFd};

pub(super) fn scoped_leaf(
    scope: super::super::Scope,
    name: &CStr,
    draft: bool,
) -> io::Result<std::ffi::CString> {
    let suffix = if draft { ".incoming" } else { "" };
    native_name(&format!(
        ".{}-{}.{}{suffix}",
        crate::container_store::hex(scope.container),
        crate::container_store::hex(scope.operation),
        name.to_str()
            .map_err(|_| invalid("invalid DATA move stage name"))?,
    ))
}

impl BoundJournal {
    pub(super) fn scoped_leaf(&self, name: &CStr, draft: bool) -> io::Result<std::ffi::CString> {
        scoped_leaf(self.original.value().scope, name, draft)
    }

    pub(super) fn draft_absent(&self, name: &CStr) -> io::Result<()> {
        absent(&self.parent, &self.scoped_leaf(name, true)?)
    }

    pub(super) fn drafts_absent(&self, names: &[&CStr]) -> io::Result<()> {
        names.iter().try_for_each(|name| self.draft_absent(name))
    }

    pub(super) fn publish(
        &self,
        name: &CStr,
        expected: &[u8],
        require: impl Fn() -> io::Result<()>,
    ) -> io::Result<OpaquePin> {
        let draft = self.scoped_leaf(name, true)?;
        self.require_original()?;
        publish(
            (&self.parent, &draft),
            (&self.directory, name),
            expected,
            || {
                self.require_original()?;
                require()
            },
        )
    }
}

pub(super) fn publish(
    from: (&HeldDir, &CStr),
    into: (&HeldDir, &CStr),
    expected: &[u8],
    require: impl Fn() -> io::Result<()>,
) -> io::Result<OpaquePin> {
    from.0.checked_child_path(from.1)?;
    into.0.checked_child_path(into.1)?;
    if !from.0.same_mount(into.0)? {
        return Err(invalid(
            "DATA move stage publication crossed its original mount",
        ));
    }
    require()?;
    absent(into.0, into.1)?;
    let staged = match from.0.opaque_pin_with(from.1, ACCESS)? {
        Some(draft) => {
            if prefix_matches(from, &draft, expected)? {
                draft.file.sync_all()?;
                draft
            } else {
                require()?;
                absent(into.0, into.1)?;
                from.0.unlink_opaque_original(from.1, &draft)?;
                require()?;
                create(from.0, from.1, expected)?
            }
        }
        None => create(from.0, from.1, expected)?,
    };
    require()?;
    staged.require(from.0, from.1)?;
    absent(into.0, into.1)?;
    // SAFETY: these are checked single components in held same-mount directories,
    // with no replacement. The closed-over original authority is required above.
    if unsafe {
        renameat2(
            from.0.file.as_raw_fd(),
            from.1.as_ptr(),
            into.0.file.as_raw_fd(),
            into.1.as_ptr(),
            1,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    let final_pin = pin(into.0, into.1)?;
    same_file(into.0, &staged.file, &final_pin.file)?;
    if read_raw(&final_pin, expected.len())? != expected {
        return Err(invalid("installed DATA move stage bytes differ"));
    }
    final_pin.require(into.0, into.1)?;
    absent(from.0, from.1)?;
    Ok(final_pin)
}

pub(super) fn prefix_matches(
    (parent, name): (&HeldDir, &CStr),
    draft: &OpaquePin,
    expected: &[u8],
) -> io::Result<bool> {
    let size = usize::try_from(draft.file.metadata()?.len())
        .map_err(|_| invalid("oversize DATA move inactive record"))?;
    let prefix = expected
        .get(..size)
        .ok_or_else(|| invalid("unknown DATA move inactive record is retained"))?;
    draft.require(parent, name)?;
    if read_raw(draft, size)? != prefix {
        return Err(invalid("unknown DATA move inactive record is retained"));
    }
    draft.require(parent, name)?;
    Ok(size == expected.len())
}

#[cfg(test)]
#[path = "../../../../tests/unit/data_move_atomic_phase.rs"]
mod tests;
