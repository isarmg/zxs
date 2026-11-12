//! Differential admission of real native archive objects, not new archive authority.
//! The fixture supplies Genesis candidates only; lifecycle/registry envelopes and
//! degraded publications are not admitted by this archive and are not claimed here.
use super::*;
use crate::{BackupPreparationPhase, CommitRequest, OperationId, RepositoryName};
use std::{fs, io::Write, os::unix::fs::DirBuilderExt};

fn operation() -> OperationId {
    OperationId::generate().unwrap()
}

struct Fixture {
    _temp: tempfile::TempDir,
    _engine: Engine,
    target: PathBuf,
}

impl Fixture {
    fn prepared() -> Self {
        let temp = tempfile::Builder::new()
            .permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .tempdir()
            .unwrap();
        let config =
            Config::development((0..3).map(|index| temp.path().join(format!("disk-{index}"))))
                .unwrap();
        let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
        engine.create_disk_root("P01", "admin", operation()).unwrap();
        let repository = engine
            .create_repository(
                &RepositoryName::parse("ArchiveDecode").unwrap(),
                "P01",
                3,
                "admin",
                operation(),
            )
            .unwrap();
        let workspace = engine
            .create_workspace(&repository.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "file", b"native format body")
            .unwrap();
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: operation(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "native format fixture".into(),
                request_digest: None,
            })
            .unwrap();
        assert_eq!(engine.state().versions.len(), 1);
        let target = temp.path().join("selected-target");
        fs::DirBuilder::new().mode(0o700).create(&target).unwrap();
        let operation = operation();
        let request = BackupPreparationRequest {
            operation_id: operation,
            principal: "admin".into(),
            source_slot_id: "B01".into(),
            target_paths: vec![target.clone()],
            expected_sequence: engine.state().sequence,
        };
        engine
            .start_backup_preparation(request, fs::File::open(&target).unwrap())
            .unwrap();
        assert_eq!(
            engine.wait_backup_preparation().unwrap().phase,
            BackupPreparationPhase::HistoryPrepared
        );
        engine
            .start_backup_native_preparation(operation, "admin")
            .unwrap();
        assert_eq!(
            engine.wait_backup_preparation().unwrap().phase,
            BackupPreparationPhase::NativeControlPrepared
        );
        Self {
            _temp: temp,
            _engine: engine,
            target,
        }
    }

}

// The previous owned algorithm is a test reference only. Each branch retains its
// own real bounded read/authentication, not a fabricated successful Source.
fn owned<T: DeserializeOwned + Serialize>(
    source: &mut impl Source,
    kind: &'static str,
    hash: &str,
) -> Result<T> {
    let value = source.object(kind, hash)?.into_value();
    let parsed: T = json_codec::from_value(value.clone())?;
    if json_codec::to_value(&parsed)? != value {
        return Err(invalid(
            "native proof projection differs from its authenticated bytes",
        ));
    }
    Ok(parsed)
}

fn error_signature(error: &Error) -> (ErrorCode, Vec<String>) {
    let mut chain = Vec::new();
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(source) = current {
        chain.push(source.to_string());
        if let Some(json) = source.downcast_ref::<json_codec::Error>() {
            chain.push(format!(
                "{:?}:{}:{}",
                json.classify(),
                json.line(),
                json.column(),
            ));
        }
        current = source.source();
    }
    (error.code(), chain)
}

fn target<'a>(root: &'a DiskRoot, stop: &'a AtomicBool) -> Target<'a, 'a> {
    Target { root, stop, budget: Budget::default(), encodings: None }
}

#[test]
fn real_retained_encoding_keeps_full_native_reports_and_target_local_failures() {
    use crate::engine::backup_preparation::native_control::databases::scoped::encoding_memo;

    let fixture = Fixture::prepared();
    let root = DiskRoot::open(&fixture.target).unwrap();
    let system = DiskRoot::open(&fixture._temp.path().join("system")).unwrap();
    let task = NativeTask::read(&system).unwrap();
    let stop = AtomicBool::new(false);
    let manifest_bytes = root.read_bytes(&manifest_path(root.path()), MAX_MANIFEST).unwrap();
    let manifest: Manifest = decode_proof(&manifest_bytes).unwrap();
    let original_files = fixture_file_bytes(fixture._temp.path());
    let expected = super::super::verify_completed(&task, &[&root], &stop).unwrap();
    let memo = encoding_memo::ScopeEncodingMemo::new();
    let binding = |_: &AtomicBool| -> Result<Option<[u8; 32]>> {
        panic!("native canonical encoding must not evaluate the SQLite binding")
    };
    let mut first = Transaction::new(Some(&memo), &binding);
    let actual = super::super::verify_completed_with_encodings(
        &task, &[&root], &stop, &mut first,
    ).unwrap();
    assert_eq!(canonical_json(&actual).unwrap(), canonical_json(&expected).unwrap());
    // Isolated native-reader comparison only. Production commit remains at the
    // enclosing registry verification's original final binding/cancellation gate.
    first.commit();

    let compare = || {
        let mut transaction = Transaction::new(Some(&memo), &binding);
        let mut baseline = target(&root, &stop);
        let mut retained = Target {
            root: &root,
            stop: &stop,
            budget: Budget::default(),
            encodings: Some(&mut transaction),
        };
        let before = verify_observed(
            &manifest, &manifest_bytes, task.selection.first(), &mut baseline,
            &mut |_, _, _, _| Ok(()),
        );
        let after = verify_observed(
            &manifest, &manifest_bytes, task.selection.first(), &mut retained,
            &mut |_, _, _, _| Ok(()),
        );
        assert_eq!(retained.budget.bytes, baseline.budget.bytes);
        assert_eq!(retained.budget.reads, baseline.budget.reads);
        match (before, after) {
            (Ok(before), Ok(after)) => {
                assert_eq!(before.0, after.0);
                assert_eq!(canonical_json(before.1.state()).unwrap(),
                    canonical_json(after.1.state()).unwrap());
                None
            }
            (Err(before), Err(after)) => {
                assert_eq!(error_signature(&after), error_signature(&before));
                Some(after.code())
            }
            _ => panic!("retained encoding changed full archive admission"),
        }
    };
    assert!(compare().is_none());
    let path = object_path(root.path(), "candidate", &manifest.publications[0].candidate).unwrap();
    let original = fs::read(&path).unwrap();
    for bad in [b"{}".as_slice(), b"{\"x\":1,\"x\":2}", b"not json"] {
        fs::write(&path, bad).unwrap();
        assert!(compare().is_some());
        assert_eq!(fs::read(&path).unwrap(), bad);
    }
    fs::write(&path, &original).unwrap();
    fs::remove_file(&path).unwrap();
    assert!(compare().is_some());
    assert!(!path.exists());
    fs::write(&path, &original).unwrap();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(compare(), Some(ErrorCode::OperationAborted));
    stop.store(false, std::sync::atomic::Ordering::Relaxed);
    assert!(compare().is_none());
    assert_eq!(fixture_file_bytes(fixture._temp.path()), original_files);
}

#[test]
fn retained_native_objects_still_charge_every_real_read_and_byte_before_authentication() {
    use crate::engine::backup_preparation::native_control::databases::scoped::encoding_memo;

    let fixture = Fixture::prepared();
    let root = DiskRoot::open(&fixture.target).unwrap();
    let stop = AtomicBool::new(false);
    let bytes = root.read_bytes(&manifest_path(root.path()), MAX_MANIFEST).unwrap();
    let manifest: Manifest = decode_proof(&bytes).unwrap();
    let hash = &manifest.publications[0].candidate;
    let memo = encoding_memo::ScopeEncodingMemo::new();
    let binding = |_: &AtomicBool| Ok(None);
    let mut first = Transaction::new(Some(&memo), &binding);
    let mut source = Target {
        root: &root,
        stop: &stop,
        budget: Budget::default(),
        encodings: Some(&mut first),
    };
    source.object("candidate", hash).unwrap();
    drop(source);
    first.commit();
    for (reads, bytes) in [(MAX_READS, 0), (0, MAX_BYTES)] {
        let mut transaction = Transaction::new(Some(&memo), &binding);
        let mut baseline = target(&root, &stop);
        baseline.budget = Budget { reads, bytes };
        let mut retained = Target {
            root: &root,
            stop: &stop,
            budget: Budget { reads, bytes },
            encodings: Some(&mut transaction),
        };
        let before = baseline.object("candidate", hash).err().unwrap();
        let after = retained.object("candidate", hash).err().unwrap();
        assert_eq!(error_signature(&after), error_signature(&before));
        assert_eq!(retained.budget.bytes, baseline.budget.bytes);
        assert_eq!(retained.budget.reads, baseline.budget.reads);
    }
}

fn compare<T: DeserializeOwned + Serialize>(
    root: &DiskRoot,
    kind: &'static str,
    hash: &str,
    stop: &AtomicBool,
) -> bool {
    let mut before = Target {
        root,
        stop,
        budget: Budget::default(),
        encodings: None,
    };
    let mut after = Target {
        root,
        stop,
        budget: Budget::default(),
        encodings: None,
    };
    let expected =
        owned::<T>(&mut before, kind, hash).and_then(|value| Ok(canonical_json(&value)?));
    let actual =
        typed::<T>(&mut after, kind, hash).and_then(|value| Ok(canonical_json(&value)?));
    assert_eq!(before.budget.bytes, after.budget.bytes);
    assert_eq!(before.budget.reads, after.budget.reads);
    match (expected, actual) {
        (Ok(expected), Ok(actual)) => {
            assert_eq!(actual, expected);
            true
        }
        (Err(expected), Err(actual)) => {
            assert_eq!(error_signature(&actual), error_signature(&expected));
            false
        }
        (expected, actual) => panic!("owned/borrowed admission differs: {expected:?} / {actual:?}"),
    }
}

fn changed<T: DeserializeOwned + Serialize>(
    root: &DiskRoot,
    kind: &'static str,
    value: &json_codec::Value,
    accepted: bool,
) {
    let hash = storage_contract::object_hash_value(kind, value).unwrap();
    let path = object_path(root.path(), kind, &hash).unwrap();
    let bytes = canonical_json(value).unwrap();
    // A deliberately changed, freshly authenticated test object. The immutable
    // archive manifest still points only to the original real fixture objects.
    if path.exists() {
        assert_eq!(fs::read(&path).unwrap(), bytes);
    } else {
        fs::write(&path, &bytes).unwrap();
    }
    assert_eq!(
        compare::<T>(root, kind, &hash, &AtomicBool::new(false)),
        accepted,
    );
    assert_eq!(fs::read(path).unwrap(), bytes);
}

fn exercise<T: DeserializeOwned + Serialize>(
    root: &DiskRoot,
    kind: &'static str,
    required: &str,
) -> json_codec::Value {
    let sample = object_path(root.path(), kind, &"0".repeat(64)).unwrap();
    let mut paths = fs::read_dir(sample.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    paths.sort();
    assert!(!paths.is_empty(), "real fixture must contain every measured object kind");
    let mut first = None;
    let mut checked_content_hash = false;
    for path in paths {
        let bytes = fs::read(&path).unwrap();
        let value = storage_contract::strict_json(&bytes).unwrap();
        let hash = storage_contract::object_hash_value(kind, &value).unwrap();
        assert_eq!(path, object_path(root.path(), kind, &hash).unwrap());
        assert!(compare::<T>(root, kind, &hash, &AtomicBool::new(false)));
        assert!(!compare::<T>(root, kind, &hash, &AtomicBool::new(true)));
        assert_eq!(fs::read(&path).unwrap(), bytes);
        if kind == "candidate" && !value["versions"].as_array().unwrap().is_empty() {
            let mut invalid_hash = value.clone();
            invalid_hash["versions"][0]["version"]["content_hash"] = "not-a-content-hash".into();
            changed::<T>(root, kind, &invalid_hash, false);
            checked_content_hash = true;
        }
        if first.is_none() {
            first = Some(value);
        }
    }
    if kind == "candidate" {
        assert!(checked_content_hash, "the real file publication must exercise ContentHash");
    }
    let original = first.unwrap();
    let mut unknown = original.clone();
    unknown
        .as_object_mut()
        .unwrap()
        .insert("unexpected_decode_field".into(), true.into());
    changed::<T>(root, kind, &unknown, false);
    let mut missing = original.clone();
    assert!(missing.as_object_mut().unwrap().remove(required).is_some());
    changed::<T>(root, kind, &missing, false);
    let mut null = original.clone();
    null[required] = json_codec::Value::Null;
    changed::<T>(root, kind, &null, false);
    original
}

#[test]
fn real_native_objects_keep_owned_decode_results_errors_and_projection_rejection() {
    let fixture = Fixture::prepared();
    let root = DiskRoot::open(&fixture.target).unwrap();
    let system = DiskRoot::open(&fixture._temp.path().join("system")).unwrap();
    let task = NativeTask::read(&system).unwrap();
    let report = read(
        &root,
        task.selection.first(),
        &task.archive_hash,
        &AtomicBool::new(false),
    )
    .unwrap();
    let mut candidate = exercise::<Candidate>(&root, "candidate", "operation_id");
    candidate["operation_id"] = "not-an-operation".into();
    changed::<Candidate>(&root, "candidate", &candidate, false);
    let mut candidate = candidate.clone();
    candidate["operation_id"] = operation().to_string().into();
    candidate["sequence"] = "01".into();
    changed::<Candidate>(&root, "candidate", &candidate, false);

    let publication = exercise::<Publication>(&root, "publication", "candidate_hash");
    let mut enumeration = publication.clone();
    enumeration["result"]["commit_state"] = "unknown-state".into();
    changed::<Publication>(&root, "publication", &enumeration, false);
    let mut nested = publication.clone();
    nested["finalize_receipts"][0]["record"]["record_type"] = "UNKNOWN".into();
    changed::<Publication>(&root, "publication", &nested, false);
    let mut optional = publication.clone();
    optional["result"]["commit_id"] = json_codec::Value::Null;
    changed::<Publication>(&root, "publication", &optional, true);
    let mut omitted = publication.clone();
    omitted.as_object_mut().unwrap().remove("recovery");
    changed::<Publication>(&root, "publication", &omitted, true);
    omitted["recovery"] = json_codec::Value::Null;
    changed::<Publication>(&root, "publication", &omitted, false);
    // Bare digest strings are not parsed by typed(): semantic/hash binding stays
    // in the archive verifier. Do not turn an existing accepted decode into a
    // supposed rejection merely because the test changed its hash text.
    let mut raw_hash = publication;
    raw_hash["candidate_hash"] = "not-a-digest".into();
    changed::<Publication>(&root, "publication", &raw_hash, true);

    exercise::<PublicationSeal>(&root, "backup-publication-seal", "publication_hash");
    let mut seal = exercise::<DiskSeal>(&root, "backup-original-seal", "disk_id");
    seal["format"] = "old-managed-directory".into();
    changed::<DiskSeal>(&root, "backup-original-seal", &seal, false);
    let mut finalize = exercise::<crate::journal::Finalize>(&root, "finalize", "candidate_hash");
    finalize.as_object_mut().unwrap().remove("recovery_id");
    changed::<crate::journal::Finalize>(&root, "finalize", &finalize, true);
    finalize["recovery_id"] = json_codec::Value::Null;
    changed::<crate::journal::Finalize>(&root, "finalize", &finalize, false);
    let mut snapshot = exercise::<Snapshot>(&root, "snapshot", "format");
    snapshot["format"] = "old-radix-map-roots".into();
    changed::<Snapshot>(&root, "snapshot", &snapshot, false);
    exercise::<CommitProof>(&root, "commit-proof", "version_hash");
    assert_eq!(
        read(
            &root,
            task.selection.first(),
            &task.archive_hash,
            &AtomicBool::new(false),
        )
        .unwrap(),
        report,
    );
}

struct ObservedSource<'a> {
    target: Target<'a, 'a>,
    reads: std::time::Duration,
    parses: std::time::Duration,
    authentications: std::time::Duration,
    journals: std::time::Duration,
    objects: usize,
    object_bytes: usize,
    journal_calls: usize,
}

impl<'a> ObservedSource<'a> {
    fn new(root: &'a DiskRoot, stop: &'a AtomicBool) -> Self {
        Self {
            target: target(root, stop),
            reads: std::time::Duration::ZERO,
            parses: std::time::Duration::ZERO,
            authentications: std::time::Duration::ZERO,
            journals: std::time::Duration::ZERO,
            objects: 0,
            object_bytes: 0,
            journal_calls: 0,
        }
    }
}

// This test-side decomposition is not a production function profiler.
// Keep Target's read, accounting and failure order; no extra cancellation point.
impl Source for ObservedSource<'_> {
    fn object(&mut self, kind: &'static str, hash: &str) -> Result<AuthenticatedMetadataObject> {
        crate::history_package::preparation_checkpoint(self.target.stop)?;
        self.objects += 1;
        let started = std::time::Instant::now();
        let read = (|| {
            self.target.root.read_bytes(
                &object_path(self.target.root.path(), kind, hash)?,
                MAX_BYTES - self.target.budget.bytes,
            )
        })();
        self.reads += started.elapsed();
        let bytes = read?;
        self.object_bytes += bytes.len();
        self.target.budget.account(bytes.len())?;

        let started = std::time::Instant::now();
        let parsed = storage_contract::strict_json(&bytes);
        self.parses += started.elapsed();
        let value = parsed?;

        let started = std::time::Instant::now();
        let authenticated = AuthenticatedMetadataObject::authenticate(kind, hash, value);
        self.authentications += started.elapsed();
        authenticated
    }

    fn journal(&mut self, index: usize, expected: &OriginalMember) -> Result<Vec<u8>> {
        self.journal_calls += 1;
        let started = std::time::Instant::now();
        let result = self.target.journal(index, expected);
        self.journals += started.elapsed();
        result
    }

    fn budget(&self) -> &Budget {
        self.target.budget()
    }

    fn account(&mut self, bytes: usize) -> Result<()> {
        self.target.account(bytes)
    }
}

// Whole calls are delegated unchanged; this is the no-copy comparison.
struct DelegatedSource<'a> {
    target: Target<'a, 'a>,
    objects: std::time::Duration,
    journals: std::time::Duration,
    object_calls: usize,
    journal_calls: usize,
}

impl Source for DelegatedSource<'_> {
    fn object(&mut self, kind: &'static str, hash: &str) -> Result<AuthenticatedMetadataObject> {
        self.object_calls += 1;
        let started = std::time::Instant::now();
        let result = self.target.object(kind, hash);
        self.objects += started.elapsed();
        result
    }

    fn journal(&mut self, index: usize, expected: &OriginalMember) -> Result<Vec<u8>> {
        self.journal_calls += 1;
        let started = std::time::Instant::now();
        let result = self.target.journal(index, expected);
        self.journals += started.elapsed();
        result
    }

    fn budget(&self) -> &Budget {
        self.target.budget()
    }

    fn account(&mut self, bytes: usize) -> Result<()> {
        self.target.account(bytes)
    }
}

#[derive(Clone, Copy)]
enum ObjectFailure {
    Kind,
    Hash,
    Missing,
    Digest,
    Reads,
    Bytes,
}

fn compare_object_failures(root: &DiskRoot, manifest: &Manifest) {
    let stop = AtomicBool::new(false);
    let original_hash = &manifest.publications[0].candidate;
    let path = object_path(root.path(), "candidate", original_hash).unwrap();
    let original = fs::read(&path).unwrap();
    let missing_hash = "0".repeat(64);
    assert!(!object_path(root.path(), "candidate", &missing_hash).unwrap().exists());
    for case in [
        ObjectFailure::Kind, ObjectFailure::Hash, ObjectFailure::Missing,
        ObjectFailure::Digest, ObjectFailure::Reads, ObjectFailure::Bytes,
    ] {
        let mut production = target(root, &stop);
        let mut observed = ObservedSource::new(root, &stop);
        match case {
            ObjectFailure::Reads => {
                production.budget.reads = MAX_READS;
                observed.target.budget.reads = MAX_READS;
            }
            ObjectFailure::Bytes => {
                production.budget.bytes = MAX_BYTES;
                observed.target.budget.bytes = MAX_BYTES;
            }
            ObjectFailure::Digest => fs::write(&path, b"{}").unwrap(),
            ObjectFailure::Kind | ObjectFailure::Hash | ObjectFailure::Missing => {}
        }
        let kind = match case {
            ObjectFailure::Kind => "not-an-archive-kind",
            ObjectFailure::Hash | ObjectFailure::Missing | ObjectFailure::Digest
            | ObjectFailure::Reads | ObjectFailure::Bytes => "candidate",
        };
        let hash = match case {
            ObjectFailure::Hash => "not-a-hash",
            ObjectFailure::Missing => missing_hash.as_str(),
            ObjectFailure::Kind | ObjectFailure::Digest | ObjectFailure::Reads
            | ObjectFailure::Bytes => original_hash.as_str(),
        };
        let expected = production.object(kind, hash).err().unwrap();
        let actual = observed.object(kind, hash).err().unwrap();
        assert_eq!(error_signature(&actual), error_signature(&expected));
        assert_eq!(observed.budget().bytes, production.budget().bytes);
        assert_eq!(observed.budget().reads, production.budget().reads);
        if matches!(case, ObjectFailure::Digest) {
            assert_eq!(fs::read(&path).unwrap(), b"{}");
            fs::write(&path, &original).unwrap();
        }
        assert_eq!(fs::read(&path).unwrap(), original);
    }
}

#[test]
fn real_archive_observed_source_matches_production_and_reports_bounded_costs() {
    let fixture = Fixture::prepared();
    let root = DiskRoot::open(&fixture.target).unwrap();
    let system = DiskRoot::open(&fixture._temp.path().join("system")).unwrap();
    let task = NativeTask::read(&system).unwrap();
    let stop = AtomicBool::new(false);
    let manifest_bytes = root.read_bytes(&manifest_path(root.path()), MAX_MANIFEST).unwrap();
    let manifest: Manifest = decode_proof(&manifest_bytes).unwrap();
    assert_eq!(object_hash(FORMAT, &manifest).unwrap(), task.archive_hash);

    let before = read_observed(
        &root, task.selection.first(), &task.archive_hash, &stop,
        &mut |_, _, _, _| Ok(()),
    ).unwrap();
    let mut source = ObservedSource::new(&root, &stop);
    let started = std::time::Instant::now();
    let observed = verify_observed(
        &manifest, &manifest_bytes, task.selection.first(), &mut source,
        &mut |_, _, _, _| Ok(()),
    ).unwrap();
    let total = started.elapsed();
    let after = read_observed(
        &root, task.selection.first(), &task.archive_hash, &stop,
        &mut |_, _, _, _| Ok(()),
    ).unwrap();
    assert_eq!(observed.0, before.0);
    assert_eq!(after.0, before.0);
    assert_eq!(
        canonical_json(observed.1.state()).unwrap(),
        canonical_json(before.1.state()).unwrap(),
    );
    assert_eq!(
        canonical_json(after.1.state()).unwrap(),
        canonical_json(before.1.state()).unwrap(),
    );
    assert_eq!(source.budget().reads, before.0.metadata_read_steps);
    assert_eq!(source.budget().bytes, before.0.verified_metadata_bytes);
    assert!(source.objects > 0);
    assert!(source.journal_calls > 0);
    // Fixed fields only. Total includes these buckets and all downstream validation;
    // the remainder also includes timing/accounting overhead, not a named function.
    // Diagnostic write failure must not replace the behavioral result.
    let _ = writeln!(
        std::io::stderr(),
        concat!(
            "[backup-archive-test-copy-us] verify_inclusive={} ",
            "object_read_inclusive={} strict_parse={} authenticate_inclusive={} ",
            "journal_inclusive={} objects={} object_bytes={} journals={} ",
            "budget_reads={} budget_bytes={}"
        ),
        total.as_micros(), source.reads.as_micros(), source.parses.as_micros(),
        source.authentications.as_micros(), source.journals.as_micros(),
        source.objects, source.object_bytes, source.journal_calls,
        source.budget().reads, source.budget().bytes,
    );

    let mut delegated = DelegatedSource {
        target: target(&root, &stop),
        objects: std::time::Duration::ZERO,
        journals: std::time::Duration::ZERO,
        object_calls: 0,
        journal_calls: 0,
    };
    let started = std::time::Instant::now();
    let direct = verify_observed(
        &manifest, &manifest_bytes, task.selection.first(), &mut delegated,
        &mut |_, _, _, _| Ok(()),
    ).unwrap();
    let direct_total = started.elapsed();
    assert_eq!(direct.0, before.0);
    assert_eq!(
        canonical_json(direct.1.state()).unwrap(),
        canonical_json(before.1.state()).unwrap(),
    );
    assert_eq!(delegated.budget().reads, before.0.metadata_read_steps);
    assert_eq!(delegated.budget().bytes, before.0.verified_metadata_bytes);
    assert_eq!(delegated.object_calls, source.objects);
    assert_eq!(delegated.journal_calls, source.journal_calls);
    // Diagnostic write failure must not replace the behavioral result.
    let _ = writeln!(
        std::io::stderr(),
        concat!(
            "[backup-archive-delegated-us] verify_inclusive={} object_inclusive={} ",
            "journal_inclusive={} objects={} journals={} budget_reads={} budget_bytes={}"
        ),
        direct_total.as_micros(), delegated.objects.as_micros(),
        delegated.journals.as_micros(), delegated.object_calls, delegated.journal_calls,
        delegated.budget().reads, delegated.budget().bytes,
    );

    let mut stage_source = DelegatedSource {
        target: target(&root, &stop),
        objects: std::time::Duration::ZERO,
        journals: std::time::Duration::ZERO,
        object_calls: 0,
        journal_calls: 0,
    };
    let mut costs = ArchiveStageCosts::default();
    let started = std::time::Instant::now();
    let staged = decomposed_verify_observed(
        &manifest, &manifest_bytes, task.selection.first(), &mut stage_source, &mut costs,
    ).unwrap();
    let stage_total = started.elapsed();
    assert_eq!(staged.0, before.0);
    assert_eq!(
        canonical_json(staged.1.state()).unwrap(),
        canonical_json(before.1.state()).unwrap(),
    );
    assert_eq!(stage_source.budget().reads, before.0.metadata_read_steps);
    assert_eq!(stage_source.budget().bytes, before.0.verified_metadata_bytes);
    assert_eq!(stage_source.object_calls, delegated.object_calls);
    assert_eq!(stage_source.journal_calls, delegated.journal_calls);
    // Stages are inclusive. Their Source subset is timed in this same execution;
    // subtraction still includes harness overhead and is not production profiling.
    costs.report(stage_total);

    let mut wrong: Manifest = decode_proof(&manifest_bytes).unwrap();
    wrong.through_snapshot.push('x');
    let wrong_bytes = canonical_json(&wrong).unwrap();
    let wrong_hash = object_hash(FORMAT, &wrong).unwrap();
    let manifest_file = manifest_path(root.path());
    fs::write(&manifest_file, &wrong_bytes).unwrap();
    let damaged_files = fixture_file_bytes(fixture._temp.path());
    // The modified manifest authenticates against its own digest, but not the
    // frozen Job. This reaches an actual outer gate, not a Source read failure.
    let expected = read_observed(
        &root, task.selection.first(), &wrong_hash, &stop,
        &mut |_, _, _, _| Ok(()),
    ).err().unwrap();
    let mut failed_source = DelegatedSource {
        target: target(&root, &stop),
        objects: std::time::Duration::ZERO,
        journals: std::time::Duration::ZERO,
        object_calls: 0,
        journal_calls: 0,
    };
    let actual = decomposed_verify_observed(
        &wrong, &wrong_bytes, task.selection.first(), &mut failed_source,
        &mut ArchiveStageCosts::default(),
    ).err().unwrap();
    assert_eq!(error_signature(&actual), error_signature(&expected));
    assert_eq!(failed_source.budget().reads, 0);
    assert_eq!(failed_source.budget().bytes, 0);
    assert_eq!(fixture_file_bytes(fixture._temp.path()), damaged_files);
    fs::write(&manifest_file, &manifest_bytes).unwrap();

    compare_object_failures(&root, &manifest);

    // Both Sources must preserve cancellation and actual corrupt-byte failures.
    let path = object_path(root.path(), "candidate", &manifest.publications[0].candidate).unwrap();
    let original = fs::read(&path).unwrap();
    for cancelled in [true, false] {
        if !cancelled {
            fs::write(&path, b"{").unwrap();
        }
        let stop = AtomicBool::new(cancelled);
        let mut production = target(&root, &stop);
        let mut observed = ObservedSource::new(&root, &stop);
        let expected = verify_observed(
            &manifest, &manifest_bytes, task.selection.first(), &mut production,
            &mut |_, _, _, _| Ok(()),
        ).err().unwrap();
        let actual = verify_observed(
            &manifest, &manifest_bytes, task.selection.first(), &mut observed,
            &mut |_, _, _, _| Ok(()),
        ).err().unwrap();
        let mut staged_source = DelegatedSource {
            target: target(&root, &stop),
            objects: std::time::Duration::ZERO,
            journals: std::time::Duration::ZERO,
            object_calls: 0,
            journal_calls: 0,
        };
        let staged_error = decomposed_verify_observed(
            &manifest, &manifest_bytes, task.selection.first(), &mut staged_source,
            &mut ArchiveStageCosts::default(),
        ).err().unwrap();
        assert_eq!(error_signature(&staged_error), error_signature(&expected));
        assert_eq!(staged_source.budget().bytes, production.budget().bytes);
        assert_eq!(staged_source.budget().reads, production.budget().reads);
        assert_eq!(error_signature(&actual), error_signature(&expected));
        assert_eq!(observed.budget().bytes, production.budget().bytes);
        assert_eq!(observed.budget().reads, production.budget().reads);
        if cancelled {
            assert_eq!(actual.code(), ErrorCode::OperationAborted);
            assert_eq!(fs::read(&path).unwrap(), original);
        } else {
            assert_eq!(fs::read(&path).unwrap(), b"{");
        }
        assert_eq!(fs::read(manifest_path(root.path())).unwrap(), manifest_bytes);
    }
    // Repair only this fixture's deliberate corruption, then use production again.
    fs::write(&path, original).unwrap();
    let restored = read_observed(
        &root, task.selection.first(), &task.archive_hash, &stop,
        &mut |_, _, _, _| Ok(()),
    ).unwrap();
    assert_eq!(restored.0, before.0);
    assert_eq!(
        canonical_json(restored.1.state()).unwrap(),
        canonical_json(before.1.state()).unwrap(),
    );
}

fn assert_finalize_encoding_equality(
    left: &crate::journal::Finalize,
    right: &crate::journal::Finalize,
) {
    assert_eq!(
        left == right,
        canonical_json(left).unwrap() == canonical_json(right).unwrap(),
    );
}

#[derive(Clone, Copy)]
enum FinalizeChange {
    Candidate, Disk, Slot, Frame, RecordKind, Sequence, RecordCandidate,
    RecordObject, Previous, Order, Duplicate, Remove, RecoveryNone,
    RecoveryEmpty, RecoverySome,
}

fn exercise_finalize_equality(original: &crate::journal::Finalize) {
    assert!(original.accepts.len() >= 2);
    assert_finalize_encoding_equality(original, original);
    for change in [
        FinalizeChange::Candidate, FinalizeChange::Disk, FinalizeChange::Slot,
        FinalizeChange::Frame, FinalizeChange::RecordKind, FinalizeChange::Sequence,
        FinalizeChange::RecordCandidate, FinalizeChange::RecordObject,
        FinalizeChange::Previous, FinalizeChange::Order, FinalizeChange::Duplicate,
        FinalizeChange::Remove, FinalizeChange::RecoveryNone, FinalizeChange::RecoveryEmpty,
        FinalizeChange::RecoverySome,
    ] {
        let mut changed = original.clone();
        match change {
            FinalizeChange::Candidate => changed.candidate_hash.push('x'),
            FinalizeChange::Disk => changed.accepts[0].disk_id.push('x'),
            FinalizeChange::Slot => changed.accepts[0].slot_id.push('x'),
            FinalizeChange::Frame => changed.accepts[0].frame_hash.push('x'),
            FinalizeChange::RecordKind => {
                assert_eq!(changed.accepts[0].record.record_type, RecordType::Accept);
                changed.accepts[0].record.record_type = RecordType::Finalize;
            }
            FinalizeChange::Sequence => changed.accepts[0].record.sequence = u64::MAX,
            FinalizeChange::RecordCandidate => changed.accepts[0].record.candidate_hash.push('x'),
            FinalizeChange::RecordObject => changed.accepts[0].record.object_hash.push('x'),
            FinalizeChange::Previous => changed.accepts[0].record.previous_frame_hash.push('x'),
            FinalizeChange::Order => changed.accepts.swap(0, 1),
            FinalizeChange::Duplicate => changed.accepts.push(changed.accepts[0].clone()),
            FinalizeChange::Remove => {
                changed.accepts.remove(0);
            }
            FinalizeChange::RecoveryNone => changed.recovery_id = None,
            FinalizeChange::RecoveryEmpty => changed.recovery_id = Some(String::new()),
            FinalizeChange::RecoverySome => changed.recovery_id = Some("fixture-recovery".into()),
        }
        if !matches!(change, FinalizeChange::RecoveryNone) {
            assert_ne!(original, &changed);
        }
        assert_finalize_encoding_equality(original, &changed);
        assert_finalize_encoding_equality(&changed, &changed);
    }
    let mut absent = original.clone();
    absent.recovery_id = None;
    let mut empty = absent.clone();
    empty.recovery_id = Some(String::new());
    assert_ne!(absent, empty);
    assert_finalize_encoding_equality(&absent, &empty);
}

// Retain the old payload algorithm only as an independent differential oracle.
fn encoded_reference_verify_payloads(
    config: &Config,
    candidate: &Candidate,
    publication: &Publication,
    source: &mut impl Source,
) -> Result<()> {
    let mut expected = BTreeSet::new();
    for evidence in &candidate.versions {
        let mut route = BTreeSet::new();
        if evidence.placement.version_id != evidence.version.version_id
            || evidence.placement.route_slot_ids.is_empty()
        {
            return Err(invalid("native payload has no complete route"));
        }
        for slot in &evidence.placement.route_slot_ids {
            if !route.insert(slot) {
                return Err(invalid("native payload repeats a route slot"));
            }
            let disk = members_config_disk(config, slot)?;
            let key = (
                evidence.version.version_id.as_str(),
                disk.disk_id.as_str(),
                slot.as_str(),
            );
            if !expected.insert(key) {
                return Err(invalid("native candidate repeats payload evidence"));
            }
            let receipt = publication
                .payload_receipts
                .iter()
                .find(|r| r.version_id == key.0 && r.disk_id == key.1 && r.slot_id == key.2)
                .ok_or_else(|| invalid("original payload receipt missing"))?;
            let proof: CommitProof = typed(source, "commit-proof", &receipt.proof_hash)?;
            if proof.candidate_hash != publication.candidate_hash
                || proof.disk_id != disk.disk_id
                || proof.slot_id != *slot
                || proof.version_hash != object_hash("version", &evidence.version)?
                || proof.route_slot_ids != evidence.placement.route_slot_ids
                || canonical_json(&proof.finalize)? != canonical_json(&publication.finalize)?
            {
                return Err(invalid(
                    "original payload proof differs from certified evidence",
                ));
            }
        }
    }
    let actual = publication
        .payload_receipts
        .iter()
        .map(|r| {
            (
                r.version_id.as_str(),
                r.disk_id.as_str(),
                r.slot_id.as_str(),
            )
        })
        .collect::<BTreeSet<_>>();
    if actual != expected || actual.len() != publication.payload_receipts.len() {
        return Err(invalid(
            "publication has extra or duplicate payload receipts",
        ));
    }
    Ok(())
}

fn fixture_file_bytes(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, output: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() {
                visit(root, &path, output);
            } else {
                assert!(metadata.is_file());
                output.insert(path.strip_prefix(root).unwrap().to_owned(), fs::read(path).unwrap());
            }
        }
    }
    let mut output = BTreeMap::new();
    visit(root, root, &mut output);
    output
}

#[test]
fn real_finalize_structural_equality_preserves_payload_rejection_and_bytes() {
    let fixture = Fixture::prepared();
    let root = DiskRoot::open(&fixture.target).unwrap();
    let system = DiskRoot::open(&fixture._temp.path().join("system")).unwrap();
    let task = NativeTask::read(&system).unwrap();
    let stop = AtomicBool::new(false);
    let manifest_bytes = root.read_bytes(&manifest_path(root.path()), MAX_MANIFEST).unwrap();
    let manifest: Manifest = decode_proof(&manifest_bytes).unwrap();
    let baseline = read_observed(
        &root, task.selection.first(), &task.archive_hash, &stop,
        &mut |_, _, _, _| Ok(()),
    ).unwrap();
    let mut source = target(&root, &stop);
    let mut exercised = false;
    let original_files = fixture_file_bytes(fixture._temp.path());
    for published in &manifest.publications {
        let candidate: Candidate = typed(&mut source, "candidate", &published.candidate).unwrap();
        let publication: Publication =
            typed(&mut source, "publication", &published.publication).unwrap();
        exercise_finalize_equality(&publication.finalize);
        if publication.payload_receipts.is_empty() {
            continue;
        }
        let original_path = object_path(
            root.path(), "commit-proof", &publication.payload_receipts[0].proof_hash,
        ).unwrap();
        let original_bytes = fs::read(&original_path).unwrap();
        let mut proof: CommitProof = typed(
            &mut source, "commit-proof", &publication.payload_receipts[0].proof_hash,
        ).unwrap();
        proof.finalize.recovery_id = Some("fixture-only-mismatch".into());
        let changed_bytes = canonical_json(&proof).unwrap();
        let changed_hash = object_hash("commit-proof", &proof).unwrap();
        let changed_path = object_path(root.path(), "commit-proof", &changed_hash).unwrap();
        assert!(!changed_path.exists());
        fs::write(&changed_path, &changed_bytes).unwrap();
        let changed_files = fixture_file_bytes(fixture._temp.path());
        // This locally changed reference tests the payload boundary, not a forged
        // complete Publication/seal. The full archive still names only original bytes.
        let mut changed_publication = publication.clone();
        changed_publication.payload_receipts[0].proof_hash = changed_hash;
        for selected in [&publication, &changed_publication] {
            let mut old = target(&root, &stop);
            let mut new = target(&root, &stop);
            let expected = encoded_reference_verify_payloads(
                &manifest.config, &candidate, selected, &mut old,
            );
            let actual = verify_payloads(&manifest.config, &candidate, selected, &mut new);
            assert_eq!(old.budget.reads, new.budget.reads);
            assert_eq!(old.budget.bytes, new.budget.bytes);
            assert!(new.budget.reads > 0);
            if std::ptr::eq(selected, &publication) {
                expected.unwrap();
                actual.unwrap();
            } else {
                let expected = expected.unwrap_err();
                let actual = actual.unwrap_err();
                assert_eq!(actual.code(), ErrorCode::PublicationInvalid);
                assert_eq!(error_signature(&actual), error_signature(&expected));
            }
            assert_eq!(fixture_file_bytes(fixture._temp.path()), changed_files);
            assert_eq!(fs::read(&original_path).unwrap(), original_bytes);
            assert_eq!(fs::read(&changed_path).unwrap(), changed_bytes);
            assert_eq!(fs::read(manifest_path(root.path())).unwrap(), manifest_bytes);
        }
        fs::remove_file(changed_path).unwrap();
        assert_eq!(fixture_file_bytes(fixture._temp.path()), original_files);
        exercised = true;
        break;
    }
    assert!(exercised);
    let restored = read_observed(
        &root, task.selection.first(), &task.archive_hash, &stop,
        &mut |_, _, _, _| Ok(()),
    ).unwrap();
    assert_eq!(restored.0, baseline.0);
    assert_eq!(fixture_file_bytes(fixture._temp.path()), original_files);
    assert_eq!(
        canonical_json(restored.1.state()).unwrap(),
        canonical_json(baseline.1.state()).unwrap(),
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ArchiveStage {
    Configuration, Journals, Candidate, Publication, Seal, PublicationSeal,
    JournalReceipts, Finalize, Snapshot, SnapshotAdmission, ReceiptPrefix,
    HistoryBinding, Payloads, Observer, ReportArchive, ReportState,
}

#[derive(Default)]
struct ArchiveStageCost {
    calls: usize,
    elapsed: std::time::Duration,
    source_elapsed: std::time::Duration,
    objects: usize,
    journals: usize,
    reads: usize,
    bytes: usize,
}

#[derive(Default)]
struct ArchiveStageCosts(BTreeMap<ArchiveStage, ArchiveStageCost>);

impl ArchiveStageCosts {
    fn measure<'a, T>(
        &mut self,
        stage: ArchiveStage,
        source: &mut DelegatedSource<'a>,
        action: impl FnOnce(&mut DelegatedSource<'a>) -> Result<T>,
    ) -> Result<T> {
        let source_elapsed = source.objects + source.journals;
        let objects = source.object_calls;
        let journals = source.journal_calls;
        let reads = source.budget().reads;
        let bytes = source.budget().bytes;
        let started = std::time::Instant::now();
        let result = action(source);
        let elapsed = started.elapsed();
        let cost = self.0.entry(stage).or_default();
        cost.calls += 1;
        cost.elapsed += elapsed;
        cost.source_elapsed += source.objects + source.journals - source_elapsed;
        cost.objects += source.object_calls - objects;
        cost.journals += source.journal_calls - journals;
        cost.reads += source.budget().reads - reads;
        cost.bytes += source.budget().bytes - bytes;
        result
    }

    fn report(&self, total: std::time::Duration) {
        // Diagnostic I/O must not replace behavior; all labels are fixed enum variants.
        let _ = writeln!(
            std::io::stderr(),
            "[backup-archive-stages-us] whole_inclusive={}",
            total.as_micros(),
        );
        for (stage, cost) in &self.0 {
            let _ = writeln!(
                std::io::stderr(),
                concat!(
                    "[backup-archive-stage-us] stage={:?} calls={} inclusive={} ",
                    "source_inclusive={} objects={} journals={} reads={} bytes={}"
                ),
                stage, cost.calls, cost.elapsed.as_micros(), cost.source_elapsed.as_micros(),
                cost.objects, cost.journals, cost.reads, cost.bytes,
            );
        }
    }
}

// Bounded test-only orchestration; actual stage algorithms remain production functions.
fn decomposed_verify_observed(
    manifest: &Manifest,
    bytes: &[u8],
    job: &Job,
    source: &mut DelegatedSource<'_>,
    costs: &mut ArchiveStageCosts,
) -> Result<(BackupNativeControlReport, AdmittedState)> {
    if manifest.format != FORMAT
        || manifest.publications.is_empty()
        || manifest.publications.len() > MAX_PUBLICATIONS
        || manifest.through_sequence != job.request.expected_sequence
        || manifest.through_snapshot != job.snapshot
        || manifest.config.pool_id != job.pool_id
        || object_hash("backup-preparation-config", &manifest.config)? != job.configuration_hash
    {
        return Err(invalid(
            "native prefix does not bind the complete frozen task",
        ));
    }
    let config = costs.measure(ArchiveStage::Configuration, source, |_| {
        crate::ValidatedConfig::try_from(manifest.config.clone())
    })?;
    source.account(bytes.len())?;
    let (journals, journal_frames) = costs.measure(ArchiveStage::Journals, source, |source| {
        original_journals(manifest, source)
    })?;
    let mut previous: Option<AdmittedState> = None;
    let mut snapshot: Option<String> = None;
    for (index, published) in manifest.publications.iter().enumerate() {
        let candidate: Candidate = costs.measure(ArchiveStage::Candidate, source, |source| {
            typed(source, "candidate", &published.candidate)
        })?;
        config.verify_original_candidate_membership(&candidate)?;
        if candidate.generation().is_some()
            || candidate.registry().is_some()
            || candidate.sequence != index as u64 + 1
            || candidate.pool_id != job.pool_id
            || candidate.previous_snapshot != snapshot
        {
            return Err(invalid(
                "native prefix does not extend the exact genesis publication chain",
            ));
        }
        let publication: Publication = costs.measure(ArchiveStage::Publication, source, |source| {
            typed(source, "publication", &published.publication)
        })?;
        let seal: PublicationSeal = costs.measure(ArchiveStage::Seal, source, |source| {
            typed(source, "backup-publication-seal", &published.seal)
        })?;
        if publication.recovery.is_some() {
            return Err(invalid(
                "degraded native publication is not supported by this control archive",
            ));
        }
        costs.measure(ArchiveStage::PublicationSeal, source, |_| {
            verify_publication_seal(&candidate, &publication, &seal)
        })?;
        costs.measure(ArchiveStage::JournalReceipts, source, |_| {
            encoded_reference_verify_journal_receipts(
                &candidate, &publication, &seal, published, &manifest.members, &journals,
            )
        })?;
        let finalize_hash = object_hash("finalize", &publication.finalize)?;
        let finalize: crate::journal::Finalize =
            costs.measure(ArchiveStage::Finalize, source, |source| {
                typed(source, "finalize", &finalize_hash)
            })?;
        if finalize != publication.finalize {
            return Err(invalid("original FINALIZE differs"));
        }
        let descriptor: Snapshot = costs.measure(ArchiveStage::Snapshot, source, |source| {
            typed(source, "snapshot", &candidate.snapshot_hash)
        })?;
        let admitted = costs.measure(ArchiveStage::SnapshotAdmission, source, |source| {
            resolve_admitted_snapshot(descriptor, |kind, hash| source.object(kind, hash))
        })?;
        let state = admitted.state();
        if state.sequence != candidate.sequence
            || state.operations.get(&candidate.operation_id) != Some(&publication.result)
            || !state
                .operation_principals
                .contains_key(&candidate.operation_id)
        {
            return Err(invalid(
                "native publication omits or changes its exact original receipt",
            ));
        }
        costs.measure(ArchiveStage::ReceiptPrefix, source, |_| {
            verify_receipt_prefix(previous.as_ref().map(AdmittedState::state), state, &candidate)
        })?;
        costs.measure(ArchiveStage::HistoryBinding, source, |source| {
            bound_history_objects_with(&candidate, Some(&admitted), |kind, hash| {
                source.object(kind, hash)
            })
        })?;
        costs.measure(ArchiveStage::Payloads, source, |source| {
            verify_payloads(&manifest.config, &candidate, &publication, source)
        })?;
        // The observer can build bounded in-memory derived material. It must
        // not install/authorize anything until this entire prefix returns.
        costs.measure(ArchiveStage::Observer, source, |_| {
            // The compared production calls use the same no-op observer.
            Ok(())
        })?;
        snapshot = Some(candidate.snapshot_hash.clone());
        previous = Some(admitted);
    }
    let state = previous.expect("invariant: admitted nonempty native prefix");
    if state.state().sequence != manifest.through_sequence
        || snapshot.as_deref() != Some(manifest.through_snapshot.as_str())
    {
        return Err(invalid("native archive stops before the frozen boundary"));
    }
    let report = BackupNativeControlReport {
        archive_hash: costs.measure(ArchiveStage::ReportArchive, source, |_| {
            Ok(object_hash(FORMAT, manifest)?)
        })?,
        state_hash: costs.measure(ArchiveStage::ReportState, source, |_| {
            Ok(object_hash("backup-prepared-state", state.state())?)
        })?,
        snapshot_hash: manifest.through_snapshot.clone(),
        sequence: manifest.through_sequence,
        publications: manifest.publications.len(),
        original_members: manifest.members.len(),
        journal_frames,
        metadata_read_steps: source.budget().reads,
        verified_metadata_bytes: source.budget().bytes,
    };
    Ok((report, state))
}

// Independent repeated-hash reference retains the receipt gate before the
// second hash in decomposed_verify_observed, including its original error order.
fn encoded_reference_verify_journal_receipts(
    candidate: &Candidate,
    publication: &Publication,
    seal: &PublicationSeal,
    published: &Published,
    members: &[OriginalMember],
    journals: &[JournalSlots],
) -> Result<()> {
    let finalize_hash = object_hash("finalize", &publication.finalize)?;
    for (member, slots) in members.iter().zip(journals) {
        let records = slots
            .get(&candidate.sequence)
            .ok_or_else(|| invalid("original member omits a publication slot"))?;
        for (record, _) in records {
            let expected = match record.record_type {
                RecordType::Prepare => &candidate.snapshot_hash,
                RecordType::Accept => &published.candidate,
                RecordType::Finalize => &finalize_hash,
                RecordType::Publication => &published.publication,
                _ => return Err(invalid("unsupported original journal record")),
            };
            if record.candidate_hash != published.candidate || record.object_hash != *expected {
                return Err(invalid("original journal references another decision"));
            }
        }
        for receipt in publication
            .finalize
            .accepts
            .iter()
            .chain(&publication.finalize_receipts)
            .chain(&seal.receipts)
            .filter(|receipt| receipt.disk_id == member.disk_id)
        {
            if receipt.slot_id != member.slot_id
                || !records
                    .iter()
                    .any(|(record, hash)| record == &receipt.record && hash == &receipt.frame_hash)
            {
                return Err(invalid(
                    "original receipt is absent from its owner's journal",
                ));
            }
        }
        if records
            .iter()
            .filter(|(record, _)| record.record_type == RecordType::Publication)
            .count()
            != 1
        {
            return Err(invalid("original journal repeats its publication slot"));
        }
    }
    Ok(())
}

#[test]
fn real_finalize_digest_reuse_preserves_archive_failures_and_read_budgets() {
    let fixture = Fixture::prepared();
    let root = DiskRoot::open(&fixture.target).unwrap();
    let system = DiskRoot::open(&fixture._temp.path().join("system")).unwrap();
    let task = NativeTask::read(&system).unwrap();
    let stop = AtomicBool::new(false);
    let manifest_bytes = root.read_bytes(&manifest_path(root.path()), MAX_MANIFEST).unwrap();
    let manifest: Manifest = decode_proof(&manifest_bytes).unwrap();
    assert!(manifest.publications.len() > 1);
    let baseline = read_observed(
        &root,
        task.selection.first(),
        &task.archive_hash,
        &stop,
        &mut |_, _, _, _| Ok(()),
    )
    .unwrap();
    let original_files = fixture_file_bytes(fixture._temp.path());
    let mut source = target(&root, &stop);
    let publication: Publication =
        typed(&mut source, "publication", &manifest.publications[0].publication).unwrap();
    let finalize_hash = object_hash("finalize", &publication.finalize).unwrap();
    let finalize_path = object_path(root.path(), "finalize", &finalize_hash).unwrap();
    let finalize_bytes = fs::read(&finalize_path).unwrap();
    let journal_path = journal_path(root.path(), 0);
    let journal_bytes = fs::read(&journal_path).unwrap();

    let compare = |succeeds: bool| {
        let files = fixture_file_bytes(fixture._temp.path());
        let mut actual_source = target(&root, &stop);
        let mut reference_source = DelegatedSource {
            target: target(&root, &stop),
            objects: std::time::Duration::ZERO,
            journals: std::time::Duration::ZERO,
            object_calls: 0,
            journal_calls: 0,
        };
        let mut costs = ArchiveStageCosts::default();
        let expected = decomposed_verify_observed(
            &manifest,
            &manifest_bytes,
            task.selection.first(),
            &mut reference_source,
            &mut costs,
        );
        let actual = verify_observed(
            &manifest,
            &manifest_bytes,
            task.selection.first(),
            &mut actual_source,
            &mut |_, _, _, _| Ok(()),
        );
        assert_eq!(actual_source.budget.reads, reference_source.budget().reads);
        assert_eq!(actual_source.budget.bytes, reference_source.budget().bytes);
        assert_eq!(fixture_file_bytes(fixture._temp.path()), files);
        if succeeds {
            let actual = actual.unwrap();
            let expected = expected.unwrap();
            assert_eq!(actual.0, expected.0);
            assert_eq!(actual.0, baseline.0);
            assert_eq!(
                canonical_json(actual.1.state()).unwrap(),
                canonical_json(expected.1.state()).unwrap(),
            );
            assert_eq!(
                canonical_json(actual.1.state()).unwrap(),
                canonical_json(baseline.1.state()).unwrap(),
            );
            None
        } else {
            let actual = match actual {
                Err(error) => error_signature(&error),
                Ok(_) => panic!("damaged archive unexpectedly admitted"),
            };
            let expected = match expected {
                Err(error) => error_signature(&error),
                Ok(_) => panic!("reference unexpectedly admitted damaged archive"),
            };
            assert_eq!(actual, expected);
            Some(actual)
        }
    };
    assert!(compare(true).is_none());

    // Exercise the changed receipt gate itself with real admitted journal records.
    // This local record mutation is not claimed to be a reauthenticated archive.
    let published = &manifest.publications[0];
    let candidate: Candidate = typed(&mut source, "candidate", &published.candidate).unwrap();
    let seal: PublicationSeal =
        typed(&mut source, "backup-publication-seal", &published.seal).unwrap();
    let (journals, _) = original_journals(&manifest, &mut source).unwrap();
    let digest = verify_journal_receipts(
        &candidate, &publication, &seal, published, &manifest.members, &journals,
    )
    .unwrap();
    assert_eq!(digest.as_str(), finalize_hash);
    let mut changed_journals = journals.clone();
    let record = &mut changed_journals[0].get_mut(&candidate.sequence).unwrap()[0].0;
    record.candidate_hash = manifest.publications[1].candidate.clone();
    assert_ne!(record.candidate_hash, published.candidate);
    fs::write(&finalize_path, b"invalid standalone FINALIZE").unwrap();
    let gate_files = fixture_file_bytes(fixture._temp.path());
    let mut gate_source = target(&root, &stop);
    let actual = verify_journal_receipts(
        &candidate, &publication, &seal, published, &manifest.members, &changed_journals,
    )
    .and_then(|digest| typed::<crate::journal::Finalize>(
        &mut gate_source, "finalize", digest.as_str(),
    ));
    let expected = encoded_reference_verify_journal_receipts(
        &candidate, &publication, &seal, published, &manifest.members, &changed_journals,
    );
    let actual = actual.err().expect("changed journal reference must fail before FINALIZE read");
    let expected = expected.unwrap_err();
    assert_eq!(actual.code(), ErrorCode::PublicationInvalid);
    assert_eq!(error_signature(&actual), error_signature(&expected));
    assert_eq!(gate_source.budget.reads, 0);
    assert_eq!(gate_source.budget.bytes, 0);
    assert_eq!(fixture_file_bytes(fixture._temp.path()), gate_files);
    fs::write(&finalize_path, &finalize_bytes).unwrap();
    assert_eq!(fixture_file_bytes(fixture._temp.path()), original_files);

    // Alter only our fixture's retained bytes, without changing any authenticated
    // reference. Both algorithms must actually reread and reject the damaged file.
    fs::write(&finalize_path, b"invalid standalone FINALIZE").unwrap();
    let finalize_error = compare(false).unwrap();
    fs::write(&finalize_path, &finalize_bytes).unwrap();
    assert_eq!(fixture_file_bytes(fixture._temp.path()), original_files);

    fs::write(&journal_path, b"invalid original journal").unwrap();
    let journal_error = compare(false).unwrap();
    fs::write(&finalize_path, b"invalid standalone FINALIZE").unwrap();
    let both_error = compare(false).unwrap();
    assert_eq!(both_error, journal_error);
    assert_ne!(finalize_error, journal_error);
    fs::write(&journal_path, &journal_bytes).unwrap();
    fs::write(&finalize_path, &finalize_bytes).unwrap();
    assert_eq!(fixture_file_bytes(fixture._temp.path()), original_files);

    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    compare(false).unwrap();
    stop.store(false, std::sync::atomic::Ordering::Relaxed);
    assert!(compare(true).is_none());
    let restored = read_observed(
        &root,
        task.selection.first(),
        &task.archive_hash,
        &stop,
        &mut |_, _, _, _| Ok(()),
    )
    .unwrap();
    assert_eq!(restored.0, baseline.0);
    assert_eq!(
        canonical_json(restored.1.state()).unwrap(),
        canonical_json(baseline.1.state()).unwrap(),
    );
    assert_eq!(fixture_file_bytes(fixture._temp.path()), original_files);
}
