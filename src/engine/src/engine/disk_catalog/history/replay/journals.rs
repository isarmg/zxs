//! Read real held journals, anchored to the admitted writers. Their in-memory
//! receipt tails are endpoints only, never the source of historical records.
use super::*;
use crate::journal::{JournalFrameReader, RecordType};

const MAX_JOURNAL_BYTES: usize = 16 * 1024 * 1024;
const MAX_JOURNAL_FRAMES: usize = 32_768;

pub(super) fn sources(
    engine: &Engine,
    through: u64,
    checkpoint: &impl Fn() -> Result<()>,
) -> Result<BTreeMap<u64, Source>> {
    let mut sources = BTreeMap::<u64, Source>::new();
    for disk in engine.config.metadata_members() {
        checkpoint()?;
        let writer = engine.journals.get(&disk.disk_id).ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "disk history has no admitted journal writer",
            )
        })?;
        let expected_tail = writer.records().last().map(|(_, hash)| hash.as_str());
        let handle = engine.disk_resources.get(disk)?;
        let root = &handle.root;
        let lease = root.admit_io()?;
        engine._locks.verify_disk(disk, root)?;
        let observe = || {
            checkpoint()?;
            lease.check_running()
        };
        let seal = validate_seal_observed(&engine.config, disk, &mut |_| observe())?;
        validate_stored_seal(&handle.seal, &seal)?;
        let path = crate::engine::membership_transition::pending::activation::log_path(
            &engine.config,
            disk,
        );
        let mut file = root.open_read_regular(&path)?;
        let stamp = crate::file_stamp::FileStamp::from_metadata(&file.metadata()?);
        let mut reader = JournalFrameReader::new(&mut file);
        let mut last = None;
        let mut frames = 0usize;
        let mut bytes = 0usize;
        let mut publication_sequence = None;
        let mut meter = |count: usize| {
            observe()?;
            bytes = bytes.checked_add(count).ok_or_else(budget)?;
            if bytes > MAX_JOURNAL_BYTES {
                return Err(budget());
            }
            Ok(())
        };
        while last.as_deref() != expected_tail {
            if frames == MAX_JOURNAL_FRAMES {
                return Err(budget());
            }
            let Some((record, hash)) = reader.next_record_observed(&mut meter)? else {
                break;
            };
            frames += 1;
            last = Some(hash);
            if record.record_type != RecordType::Publication {
                continue;
            }
            if publication_sequence.is_some_and(|sequence| record.sequence <= sequence) {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "disk history journal repeats a publication",
                ));
            }
            publication_sequence = Some(record.sequence);
            if record.sequence > through {
                continue;
            }
            let source = Source {
                candidate: record.candidate_hash,
                publication: record.object_hash,
            };
            match sources.entry(record.sequence) {
                std::collections::btree_map::Entry::Occupied(old) => {
                    if old.get().candidate != source.candidate
                        || old.get().publication != source.publication
                    {
                        return Err(Error::new(
                            ErrorCode::LogFork,
                            "disk history publication sources conflict",
                        ));
                    }
                }
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(source);
                }
            }
            if sources.len() > MAX_PUBLICATIONS {
                return Err(budget());
            }
        }
        if reader.torn_tail
            || last.as_deref() != expected_tail
            || reader.probe_extra_observed(&mut meter)?
        {
            return Err(Error::new(
                ErrorCode::LogCorrupt,
                "disk history journal differs from its admitted writer endpoint",
            ));
        }
        drop(reader);
        if crate::file_stamp::FileStamp::from_metadata(&file.metadata()?) != stamp
            || crate::file_stamp::FileStamp::from_metadata(
                &root.open_read_regular(&path)?.metadata()?,
            ) != stamp
        {
            return Err(Error::new(
                ErrorCode::LogCorrupt,
                "disk history journal changed during source replay",
            ));
        }
        file.check_running()?;
        let seal = validate_seal_observed(&engine.config, disk, &mut |_| observe())?;
        validate_stored_seal(&handle.seal, &seal)?;
        observe()?;
    }
    Ok(sources)
}
