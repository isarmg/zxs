//! A successful pure archive computation, reused only after the same ordered
//! reads on the current held target. No State, root, vote or writer is retained.
use super::*;
use super::super::databases::scoped::encoding_memo::ArchiveReservation;
use std::{collections::LinkedList, mem::size_of, sync::Arc};

#[derive(Clone, Copy, PartialEq, Eq)]
enum ObjectKind {
    Candidate,
    Publication,
    Finalize,
    OriginalSeal,
    PublicationSeal,
    CommitProof,
    Snapshot,
    ControlAuth,
    StatePage,
    MetadataIndex,
    HistoryManifest,
}

impl ObjectKind {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "candidate" => Some(Self::Candidate),
            "publication" => Some(Self::Publication),
            "finalize" => Some(Self::Finalize),
            "backup-original-seal" => Some(Self::OriginalSeal),
            "backup-publication-seal" => Some(Self::PublicationSeal),
            "commit-proof" => Some(Self::CommitProof),
            "snapshot" => Some(Self::Snapshot),
            "state-control-auth" => Some(Self::ControlAuth),
            "state-page" => Some(Self::StatePage),
            "metadata-index" => Some(Self::MetadataIndex),
            "history-manifest" => Some(Self::HistoryManifest),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Publication => "publication",
            Self::Finalize => "finalize",
            Self::OriginalSeal => "backup-original-seal",
            Self::PublicationSeal => "backup-publication-seal",
            Self::CommitProof => "commit-proof",
            Self::Snapshot => "snapshot",
            Self::ControlAuth => "state-control-auth",
            Self::StatePage => "state-page",
            Self::MetadataIndex => "metadata-index",
            Self::HistoryManifest => "history-manifest",
        }
    }
}

// Constructed only after the real bounded object's address and hash admission.
struct ObjectDigest(Box<str>);

impl ObjectDigest {
    fn as_str(&self) -> &str {
        &self.0
    }
}

enum Step {
    Object { kind: ObjectKind, digest: ObjectDigest, bytes: Box<[u8]> },
    Journal { index: usize, expected_bytes: usize, bytes: Box<[u8]> },
    Account(usize),
}

/// The constructor is private to the successful no-observer verifier below.
/// Its report cannot be used without reading every target-local input again.
pub(in crate::engine::backup_preparation::native_control) struct VerifiedReadTrace {
    manifest: Box<[u8]>,
    steps: LinkedList<Step>,
    report: BackupNativeControlReport,
}

impl VerifiedReadTrace {
    pub(in crate::engine::backup_preparation::native_control) fn initial_charge(
        manifest: &[u8],
    ) -> Option<usize> {
        manifest.len().checked_add(size_of::<Self>() + 2 * size_of::<usize>())
    }

    fn matches(&self, manifest: &[u8]) -> bool {
        self.manifest.as_ref() == manifest
    }
}

struct Capture<'a> {
    reservation: ArchiveReservation<'a>,
    manifest: Box<[u8]>,
    steps: LinkedList<Step>,
}

impl<'a> Capture<'a> {
    fn new(encodings: &Transaction<'a>, manifest: &[u8]) -> Option<Self> {
        let reservation = encodings.reserve_archive(manifest)?;
        Some(Self {
            reservation,
            manifest: Box::from(manifest),
            steps: LinkedList::new(),
        })
    }

    fn reserve_step(&mut self, payload: usize) -> bool {
        // Nodes do not retain spare vector capacity. Allocator overhead, as for
        // the existing memo tree, is separately bounded by MAX_READS.
        self.steps.len() < MAX_READS
            && payload.checked_add(size_of::<Step>() + 2 * size_of::<usize>())
                .is_some_and(|bytes| self.reservation.grow(bytes))
    }

    fn object(&mut self, kind: &'static str, hash: &str, bytes: &[u8]) -> bool {
        let Some(kind) = ObjectKind::parse(kind) else {
            return false;
        };
        if !hash_valid(hash)
            || !bytes.len().checked_add(hash.len())
                .is_some_and(|payload| self.reserve_step(payload))
        {
            return false;
        }
        self.steps.push_back(Step::Object {
            kind,
            digest: ObjectDigest(Box::from(hash)),
            bytes: Box::from(bytes),
        });
        true
    }

    fn journal(&mut self, index: usize, expected_bytes: usize, bytes: &[u8]) -> bool {
        if !self.reserve_step(bytes.len()) {
            return false;
        }
        self.steps.push_back(Step::Journal {
            index,
            expected_bytes,
            bytes: Box::from(bytes),
        });
        true
    }

    fn account(&mut self, bytes: usize) -> bool {
        if !self.reserve_step(0) {
            return false;
        }
        self.steps.push_back(Step::Account(bytes));
        true
    }

    fn finish(mut self, report: &BackupNativeControlReport) -> Option<(
        ArchiveReservation<'a>, Arc<VerifiedReadTrace>,
    )> {
        let payload = report.archive_hash.len()
            .checked_add(report.state_hash.len())?
            .checked_add(report.snapshot_hash.len())?;
        if !self.reservation.grow(payload) {
            return None;
        }
        let trace = VerifiedReadTrace {
            manifest: self.manifest,
            steps: self.steps,
            report: report.clone(),
        };
        Some((self.reservation, Arc::new(trace)))
    }
}

struct Recording<'root, 'memo> {
    target: Target<'root, 'memo>,
    capture: Option<Capture<'memo>>,
}

impl Source for Recording<'_, '_> {
    fn object(&mut self, kind: &'static str, hash: &str) -> Result<AuthenticatedMetadataObject> {
        let bytes = self.target.read_object(kind, hash)?;
        let object = self.target.authenticate_object(kind, hash, &bytes)?;
        if self.capture.as_mut().is_some_and(|capture| !capture.object(kind, hash, &bytes)) {
            self.capture = None;
        }
        Ok(object)
    }

    fn journal(&mut self, index: usize, expected: &OriginalMember) -> Result<Vec<u8>> {
        let bytes = self.target.read_journal(index, expected.journal_bytes)?;
        if self.capture.as_mut()
            .is_some_and(|capture| !capture.journal(index, expected.journal_bytes, &bytes))
        {
            self.capture = None;
        }
        Ok(bytes)
    }

    fn budget(&self) -> &Budget {
        &self.target.budget
    }

    fn account(&mut self, bytes: usize) -> Result<()> {
        self.target.budget.account(bytes)?;
        if self.capture.as_mut().is_some_and(|capture| !capture.account(bytes)) {
            self.capture = None;
        }
        Ok(())
    }
}

/// Only bytes read during this invocation may be replayed to the full verifier.
/// Equal bytes borrow the trace; the first differing read owns its actual bytes.
struct Prefix<'a> {
    steps: std::collections::linked_list::Iter<'a, Step>,
    reads: usize,
    changed: Option<Vec<u8>>,
}

enum ReadBytes<'a> {
    Equal(&'a [u8]),
    Changed(Vec<u8>),
}

impl AsRef<[u8]> for ReadBytes<'_> {
    fn as_ref(&self) -> &[u8] {
        match self {
            Self::Equal(bytes) => bytes,
            Self::Changed(bytes) => bytes,
        }
    }
}

impl<'a> Prefix<'a> {
    fn take(&mut self, matches: impl FnOnce(&Step) -> bool) -> Option<ReadBytes<'a>> {
        if self.reads == 0 {
            return None;
        }
        let step = self.steps.find(|step| !matches!(step, Step::Account(_)))?;
        if !matches(step) {
            // An optional observation is never a replacement for a request.
            // With the same manifest, prior equal bytes and verifier this cannot
            // precede the changed input; a changed input exhausts this prefix.
            self.reads = 0;
            self.changed = None;
            return None;
        }
        self.reads -= 1;
        if self.reads == 0 {
            return self.changed.take().map(ReadBytes::Changed);
        }
        match step {
            Step::Object { bytes, .. } | Step::Journal { bytes, .. } => {
                Some(ReadBytes::Equal(bytes))
            }
            Step::Account(_) => None,
        }
    }
}

struct Replay<'trace, 'root, 'memo> {
    target: Target<'root, 'memo>,
    prefix: Prefix<'trace>,
}

impl Source for Replay<'_, '_, '_> {
    fn object(&mut self, kind: &'static str, hash: &str) -> Result<AuthenticatedMetadataObject> {
        match self.prefix.take(|step| matches!(step,
            Step::Object { kind: saved, digest, .. }
                if saved.as_str() == kind && digest.as_str() == hash))
        {
            Some(bytes) => {
                self.target.budget.account(bytes.as_ref().len())?;
                // The real read and its cancellation/root checks already ran.
                self.target.authenticate_object(kind, hash, bytes.as_ref())
            }
            None => self.target.object(kind, hash),
        }
    }

    fn journal(&mut self, index: usize, expected: &OriginalMember) -> Result<Vec<u8>> {
        match self.prefix.take(|step| matches!(step,
            Step::Journal { index: saved, expected_bytes, .. }
                if *saved == index && *expected_bytes == expected.journal_bytes))
        {
            Some(bytes) => {
                self.target.budget.account(bytes.as_ref().len())?;
                match bytes {
                    ReadBytes::Equal(bytes) => Ok(bytes.to_vec()),
                    ReadBytes::Changed(bytes) => Ok(bytes),
                }
            }
            None => self.target.journal(index, expected),
        }
    }

    fn budget(&self) -> &Budget {
        &self.target.budget
    }

    fn account(&mut self, bytes: usize) -> Result<()> {
        self.target.budget.account(bytes)
    }
}

fn replay<'a>(
    trace: &'a VerifiedReadTrace,
    target: &mut Target<'_, '_>,
) -> Result<Option<Prefix<'a>>> {
    let mut reads = 0;
    for step in &trace.steps {
        let (actual, expected) = match step {
            Step::Object { kind, digest, bytes } => {
                (target.read_object(kind.as_str(), digest.as_str())?, bytes.as_ref())
            }
            Step::Journal { index, expected_bytes, bytes } => {
                (target.read_journal(*index, *expected_bytes)?, bytes.as_ref())
            }
            Step::Account(bytes) => {
                target.budget.account(*bytes)?;
                continue;
            }
        };
        reads += 1;
        if actual.as_slice() != expected {
            return Ok(Some(Prefix {
                steps: trace.steps.iter(),
                reads,
                changed: Some(actual),
            }));
        }
    }
    Ok(None)
}

pub(super) fn read(
    root: &DiskRoot,
    job: &Job,
    task: &NativeTask,
    stop: &AtomicBool,
    encodings: &mut Transaction<'_>,
) -> Result<BackupNativeControlReport> {
    let bytes = root.read_bytes(&manifest_path(root.path()), MAX_MANIFEST)?;
    let manifest: Manifest = decode_proof(&bytes)?;
    if object_hash(FORMAT, &manifest)? != task.archive_hash {
        return Err(invalid("native archive differs from the immutable task"));
    }
    if let Some(trace) = encodings.archive(&bytes).filter(|trace| trace.matches(&bytes)) {
        // The task expectation and configuration are current inputs, not part
        // of a trusted lookup key. Keep the original header gate before reads.
        verify_manifest(&manifest, job)?;
        let mut target = Target {
            root, stop, budget: Budget::default(),
            encodings: Some(Encodings::Pending(encodings)),
        };
        let Some(prefix) = replay(&trace, &mut target)? else {
            return Ok(trace.report.clone());
        };
        target.budget = Budget::default();
        let mut source = Replay { target, prefix };
        return verify(&manifest, &bytes, job, &mut source).map(|(report, _)| report);
    }
    let capture = Capture::new(encodings, &bytes);
    let mut source = Recording {
        target: Target {
            root, stop, budget: Budget::default(),
            encodings: Some(Encodings::Pending(encodings)),
        },
        capture,
    };
    let (report, _) = verify(&manifest, &bytes, job, &mut source)?;
    let retained = source.capture.take().and_then(|capture| capture.finish(&report));
    drop(source);
    if let Some((reservation, trace)) = retained {
        encodings.retain_archive(reservation, trace);
    }
    Ok(report)
}

#[cfg(test)]
#[path = "../../../../../../../test/engine/unit/backup_native_read_trace_tests.rs"]
mod tests;
