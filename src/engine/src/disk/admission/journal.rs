//! Journal descriptors are one revocable resource, including the held parent.
//! Only complete journal operations cross this boundary, never a File or lease.
use super::*;
use crate::disk::{create_journal, open_existing_journal};
#[cfg(feature = "fault-injection")]
use crate::journal::{JournalFault, OpeningFault};
use crate::journal::{JournalNamespace, Scan, journal_name, scan_frames_observed};

pub(crate) struct JournalIo {
    source: JournalSource,
}

enum JournalSource {
    Bound(DiskIoResource<JournalFiles>),
    Unbound(JournalFiles),
}

struct JournalFiles {
    file: Option<File>,
    directory: File,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Access {
    Scan,
    Write,
}

impl JournalIo {
    pub(in crate::disk) fn new(directory: File, lease: Option<DiskIoLease>) -> Result<Self> {
        let files = JournalFiles {
            file: None,
            directory,
        };
        let source = match lease {
            Some(lease) => {
                let files = lease.hold(files);
                files.check_running()?;
                JournalSource::Bound(files)
            }
            None => JournalSource::Unbound(files),
        };
        Ok(Self { source })
    }

    // Private to this resource implementation. Callers cannot extract or clone
    // the descriptors through a generic callback, even during an admitted tail.
    fn with_files<T>(
        &mut self,
        access: Access,
        run: impl FnOnce(&mut JournalFiles, &mut dyn FnMut(usize) -> Result<()>) -> Result<T>,
    ) -> Result<T> {
        match &mut self.source {
            JournalSource::Bound(files) => {
                let mut guard = files.access_checked()?;
                if access == Access::Write {
                    files.check_journal_authority()?;
                }
                run(&mut guard.value().resource, &mut |_| files.check_running())
            }
            JournalSource::Unbound(files) => run(files, &mut |_| Ok(())),
        }
    }

    pub(crate) fn check_writer(&self) -> Result<()> {
        match &self.source {
            JournalSource::Bound(files) => {
                files.check_running()?;
                files.check_journal_authority()
            }
            JournalSource::Unbound(_) => Ok(()),
        }
    }

    /// The path is selected by this authority, not by whichever root happened
    /// to open it. A successor root cannot lend its authority to an old journal.
    pub(crate) fn require_configured(&self, config: &ValidatedConfig) -> Result<()> {
        if let JournalSource::Bound(files) = &self.source
            && !same_authority(&files.binding.authority, config)
        {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "journal namespace differs from its held root authority",
            ));
        }
        Ok(())
    }

    pub(crate) fn require_genesis(&self) -> Result<()> {
        if let JournalSource::Bound(files) = &self.source
            && files.binding.authority.generation().is_some()
        {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "genesis journal cannot use a membership generation root",
            ));
        }
        Ok(())
    }

    pub(crate) fn scan(&mut self, namespace: JournalNamespace) -> Result<Scan> {
        self.with_files(Access::Scan, |files, observe| {
            files.file = open_existing_journal(&files.directory, &journal_name())?;
            match &files.file {
                Some(file) => {
                    scan_frames_observed(io::BufReader::new(file), namespace, &mut |bytes| {
                        observe(bytes)
                    })
                }
                None => Ok(Scan {
                    records: Vec::new(),
                    valid_bytes: 0,
                    torn_tail: false,
                }),
            }
        })
    }

    /// A reserved first-generation slot has a tiny closed record prefix. This
    /// limit charges every actual read, even if the file grows during scanning.
    pub(crate) fn scan_bounded(
        &mut self,
        namespace: JournalNamespace,
        maximum: usize,
    ) -> Result<Scan> {
        self.with_files(Access::Scan, |files, observe| {
            files.file = open_existing_journal(&files.directory, &journal_name())?;
            match &files.file {
                Some(file) => {
                    if file.metadata()?.len() > maximum as u64 {
                        return Err(Error::new(
                            ErrorCode::ResourceLimit,
                            "reserved generation journal exceeds its byte limit",
                        ));
                    }
                    let mut total = 0usize;
                    let scan =
                        scan_frames_observed(io::BufReader::new(file), namespace, &mut |bytes| {
                            total = total.checked_add(bytes).ok_or_else(|| {
                                Error::new(
                                    ErrorCode::ResourceLimit,
                                    "reserved journal read budget overflow",
                                )
                            })?;
                            if total > maximum {
                                return Err(Error::new(
                                    ErrorCode::ResourceLimit,
                                    "reserved generation journal exceeds its byte limit",
                                ));
                            }
                            observe(bytes)
                        })?;
                    files.verify_named()?;
                    Ok(scan)
                }
                None => Ok(Scan {
                    records: Vec::new(),
                    valid_bytes: 0,
                    torn_tail: false,
                }),
            }
        })
    }

    pub(crate) fn create(&mut self) -> Result<()> {
        self.with_files(Access::Write, |files, _| {
            files.file = Some(create_journal(&files.directory, &journal_name())?);
            files.sync_open(
                #[cfg(feature = "fault-injection")]
                None,
            )
        })
    }

    pub(crate) fn finish_open(
        &mut self,
        #[cfg(feature = "fault-injection")] fault: Option<OpeningFault>,
    ) -> Result<()> {
        self.with_files(Access::Write, |files, _| {
            if files.file.is_none() {
                files.file = Some(create_journal(&files.directory, &journal_name())?);
            }
            files.sync_open(
                #[cfg(feature = "fault-injection")]
                fault,
            )
        })
    }

    pub(crate) fn synchronize(
        &mut self,
        #[cfg(feature = "fault-injection")] fault: Option<OpeningFault>,
    ) -> Result<()> {
        self.with_files(Access::Write, |files, _| {
            files.sync_open(
                #[cfg(feature = "fault-injection")]
                fault,
            )
        })
    }

    pub(crate) fn verify_singleton(&mut self) -> Result<()> {
        self.with_files(Access::Write, |files, _| files.verify_named())
    }

    pub(crate) fn repair(&mut self, valid_bytes: u64) -> Result<()> {
        self.with_files(Access::Write, |files, _| files.repair(valid_bytes))
    }

    pub(crate) fn append(
        &mut self,
        bytes: &[u8],
        repair: Option<u64>,
        namespace: JournalNamespace,
        #[cfg(feature = "fault-injection")] fault: Option<JournalFault>,
    ) -> Result<()> {
        self.with_files(Access::Write, |files, _| {
            // One guard covers truncation, append, fsync and the final name check.
            // Quiesce can cancel idle owners, but cannot close an in-flight
            // descriptor or retire its writer fence before this real tail.
            if let Some(valid_bytes) = repair {
                files.repair(valid_bytes)?;
            }
            let mut file = files.file();
            #[cfg(feature = "fault-injection")]
            if matches!(fault, Some(JournalFault::PartialWrite)) {
                file.write_all(&bytes[..bytes.len() / 2])?;
                return Err(io::Error::from_raw_os_error(28).into());
            }
            file.write_all(bytes)?;
            #[cfg(feature = "fault-injection")]
            if matches!(fault, Some(JournalFault::Sync)) {
                return Err(io::Error::from_raw_os_error(5).into());
            }
            file.sync_data()?;
            if namespace != JournalNamespace::Active {
                files.verify_named()?;
            }
            Ok(())
        })
    }
}

impl DiskIoResource<JournalFiles> {
    fn check_journal_authority(&self) -> Result<()> {
        let state = self.binding.gate.state()?;
        if !same_authority(&state.authority, &self.binding.authority) {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "journal writer belongs to a previous membership authority",
            ));
        }
        Ok(())
    }
}

impl JournalFiles {
    fn file(&self) -> &File {
        self.file
            .as_ref()
            .expect("invariant: journal writer has opened its file")
    }

    fn sync_open(
        &self,
        #[cfg(feature = "fault-injection")] fault: Option<OpeningFault>,
    ) -> Result<()> {
        #[cfg(feature = "fault-injection")]
        if fault == Some(OpeningFault::FileSync) {
            return Err(io::Error::from_raw_os_error(5).into());
        }
        self.file().sync_data()?;
        #[cfg(feature = "fault-injection")]
        if fault == Some(OpeningFault::DirectorySync) {
            return Err(io::Error::from_raw_os_error(5).into());
        }
        self.directory.sync_all()?;
        Ok(())
    }

    fn repair(&self, valid_bytes: u64) -> Result<()> {
        self.file().set_len(valid_bytes)?;
        self.file().sync_data()?;
        Ok(())
    }

    fn verify_named(&self) -> Result<()> {
        use platform_io::fs_safety::linux::FileIdentity;
        let named = open_existing_journal(&self.directory, &journal_name())?.ok_or_else(|| {
            Error::new(
                ErrorCode::LogFork,
                "membership journal name disappeared before receipt",
            )
        })?;
        if FileIdentity::from_fd(&named)? != FileIdentity::from_fd(self.file())? {
            return Err(Error::new(
                ErrorCode::LogFork,
                "membership journal name changed before receipt",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/journal/resource_ownership_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../../test/engine/unit/journal/blocked_io_tests.rs"]
mod blocked_io_tests;
