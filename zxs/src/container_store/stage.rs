//! Per-operation custody of original private staging directories and records.
use super::invalid;
use crate::fs::{HeldDir, OpaquePin};
use std::{
    ffi::{CStr, CString},
    fs::File,
    io::{self, Write},
    os::unix::fs::MetadataExt,
};

pub(super) struct DataStage<'a> {
    parent: &'a HeldDir,
    name: CString,
    pub(super) directory: HeldDir,
    files: Vec<(&'static CStr, File)>,
}

impl<'a> DataStage<'a> {
    pub(super) fn new(parent: &'a HeldDir, name: CString) -> io::Result<Self> {
        let directory = parent.new_child_directory(&name)?;
        directory.only(None)?;
        Ok(Self {
            parent,
            name,
            directory,
            files: Vec::new(),
        })
    }

    fn require_named(&self) -> io::Result<()> {
        let named = self
            .parent
            .child_directory(&self.name)?
            .ok_or_else(|| invalid("original staging directory lost its name"))?;
        if named.entity != self.directory.entity || !named.same_mount(&self.directory)? {
            return Err(invalid("staging directory differs from the original"));
        }
        Ok(())
    }

    pub(super) fn create(&mut self, name: &'static CStr) -> io::Result<&mut File> {
        self.require_named()?;
        let file = self.directory.create_opaque_record(name)?;
        self.files.push((name, file));
        self.files
            .last_mut()
            .map(|(_, file)| file)
            .ok_or_else(|| invalid("new staging record is not tracked"))
    }

    pub(super) fn write(&mut self, name: &'static CStr, bytes: &[u8]) -> io::Result<()> {
        let file = self.create(name)?;
        file.write_all(bytes)?;
        file.sync_all()
    }

    fn current_file(&self, name: &CStr, original: &File) -> io::Result<Option<OpaquePin>> {
        let Some(named) = self.directory.opaque_pin(name)? else {
            return Ok(None);
        };
        let previous = original.metadata()?;
        let current = named.file.metadata()?;
        if !self.directory.shares_file_mount(original)?
            || (previous.dev(), previous.ino()) != (current.dev(), current.ino())
        {
            return Err(invalid("staging record differs from the original file"));
        }
        Ok(Some(named))
    }

    fn require_files(&self) -> io::Result<()> {
        self.require_named()?;
        for (name, original) in &self.files {
            self.current_file(name, original)?
                .ok_or_else(|| invalid("staging record lost its name"))?;
        }
        self.require_named()
    }

    pub(super) fn publish_directory(
        &self,
        parent: &HeldDir,
        name: &CStr,
        authorize: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        self.require_files()?;
        self.parent
            .rename_child_if_absent(&self.name, parent, name, authorize)
    }

    fn clean(&self) -> io::Result<()> {
        self.require_named()?;
        // A successful publication has moved the whole stage, so the
        // original stage name is no longer eligible for cleanup.
        let records = self
            .files
            .iter()
            .map(|(name, file)| Ok((*name, self.current_file(name, file)?)))
            .collect::<io::Result<Vec<_>>>()?;
        for (name, pin) in &records {
            if let Some(pin) = pin {
                self.require_named()?;
                self.directory.unlink_opaque_original(name, pin)?;
            }
        }
        // Unknown leaves stay put. Only remove the original empty stage.
        self.parent
            .unlink_empty_opaque_original(&self.name, &self.directory)
    }
}

impl Drop for DataStage<'_> {
    fn drop(&mut self) {
        let _ = self.clean();
    }
}
