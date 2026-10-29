use super::*;
use storage_contract::{
    OPERATION_SEGMENT_RECORDS, OPERATION_SEGMENT_TARGET_BYTES, OperationLog, OperationRecordEntry,
};

fn event(sequence: u64) -> Event {
    Event {
        sequence,
        operation_id: format!("op_{sequence:032x}").parse().unwrap(),
        kind: "fixture".into(),
        object_id: "fixture".into(),
    }
}

#[test]
fn incremental_directory_encoding_visits_dirty_leaf_values_not_the_sealed_prefix() {
    // Equal-sequence events are valid; repeated immutable bodies let this
    // fixture exercise a large directory without materializing large history.
    let segment = storage_contract::EventSegment {
        format: Default::default(),
        events: vec![event(0); storage_contract::EVENT_SEGMENT_SPAN],
    };
    let reference = storage_contract::EventSegmentRef {
        hash: object_hash(EVENT_SEGMENT_KIND, &segment).unwrap(),
        first_sequence: 0,
    };
    let events = json_codec::from_value(json_codec::json!({
        "sealed": vec![reference; 512], "tail": [event(0)],
    }))
    .unwrap();
    let receipts = (0..4096u64)
        .map(|sequence| storage_contract::OperationRecordEntry {
            principal: "owner".into(),
            result: OperationResult {
                operation_id: event(sequence).operation_id,
                kind: "fixture".into(),
                commit_id: None,
                history_tree_id: None,
                branch_id: None,
                commit_state: storage_contract::CommitState::Published,
                protection_at_publish: storage_contract::ProtectionAtPublish::NotApplicable,
                current_health: storage_contract::CurrentHealth::Healthy,
                request_digest: "digest".into(),
                sequence,
                result: json_codec::json!({}),
            },
        })
        .collect();
    let before = State {
        sequence: 4096,
        events,
        operations: storage_contract::OperationLog::from_records(receipts).unwrap(),
        ..Default::default()
    };
    let base = prepare(&before, MAX_METADATA).unwrap();
    let mut after = before.clone();
    after.begin_tracking();
    after.sequence += 1;
    after.events.push(event(after.sequence)).unwrap();
    SEGMENT_DIRECTORY_SERIALIZATIONS.with(|counts| counts.borrow_mut().clear());
    let incremental = prepare_incremental(
        &before,
        &after,
        &base.reuse_basis(),
        MAX_METADATA,
        &mut no_reused_sources,
    )
    .unwrap();
    let counts = SEGMENT_DIRECTORY_SERIALIZATIONS.with(|counts| counts.borrow().clone());
    assert_eq!(counts.get("operation_log").copied().unwrap_or(0), 0);
    assert!(counts["events"] > 0 && counts["events"] < 128, "{counts:?}");
    assert_eq!(
        incremental.hash,
        prepare(&after, MAX_METADATA).unwrap().hash
    );
}

// These tests measure warm incremental snapshot serialization and reused locator
// callback loads, not installation, recovery, or whole-request complexity. A
// seal still copies the shared Arc<Vec> directory while the old State is alive;
// missing-baseline/full-oracle encoding still deliberately visits all history.
// Neither allocation volume nor repeated authentication of memoized pages is
// measured here. No historical segment bodies are loaded by this codec path.
const RECEIPT_PREFIXES: [usize; 3] = [4_096, 65_536, 131_072];
type Address = (&'static str, String);
type Sources = BTreeMap<Address, Vec<u8>>;

fn receipt(sequence: u64) -> OperationRecordEntry {
    OperationRecordEntry {
        principal: "owner".into(),
        result: OperationResult {
            operation_id: OperationId::generate(),
            kind: "fixture".into(),
            commit_id: None,
            history_tree_id: None,
            branch_id: None,
            commit_state: storage_contract::CommitState::Published,
            protection_at_publish: storage_contract::ProtectionAtPublish::NotApplicable,
            current_health: storage_contract::CurrentHealth::Healthy,
            request_digest: "digest".into(),
            sequence,
            result: json_codec::json!({}),
        },
    }
}

fn receipt_prefix(count: usize) -> State {
    let state = State {
        sequence: count as u64,
        operations: OperationLog::from_records((1..=count as u64).map(receipt).collect()).unwrap(),
        ..Default::default()
    };
    assert_eq!(state.operations.len(), count as u64);
    assert_eq!(
        state.operations.sealed().len(),
        count / OPERATION_SEGMENT_RECORDS
    );
    assert!(state.operations.tail().is_empty());
    assert_eq!(state.operations.cold_index().unwrap().len(), count);
    state
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ReusedReads {
    calls: BTreeMap<Address, usize>,
    bytes: usize,
}

fn load_source(
    sources: &Sources,
    kind: &'static str,
    hash: &str,
) -> Result<AuthenticatedMetadataObject> {
    let bytes = sources.get(&(kind, hash.to_owned())).ok_or_else(|| {
        Error::new(
            ErrorCode::ObjectCorrupt,
            format!("receipt cost fixture has no source for {kind}/{hash}"),
        )
    })?;
    AuthenticatedMetadataObject::authenticate(kind, hash, json_codec::from_slice(bytes)?)
}

impl ReusedReads {
    fn load(
        &mut self,
        sources: &Sources,
        kind: &'static str,
        hash: &str,
    ) -> Result<AuthenticatedMetadataObject> {
        let object = load_source(sources, kind, hash)?;
        *self.calls.entry((kind, hash.to_owned())).or_default() += 1;
        self.bytes += object.canonical_len();
        Ok(object)
    }
}

struct WarmReceipts {
    before: State,
    basis: SnapshotReuse,
    sources: Sources,
    // Preserved from the legitimate typed construction, never used by the
    // measured incremental encoder. Only the independent full oracle uses it.
    index: BTreeMap<OperationId, u64>,
}

impl WarmReceipts {
    fn new(mut before: State) -> Self {
        let index = before.operations.cold_index().unwrap().clone();
        let prepared = prepare(&before, MAX_METADATA).unwrap();
        let basis = prepared.reuse_basis();
        let sources = prepared
            .objects
            .into_iter()
            .map(|object| ((object.kind, object.hash), object.bytes))
            .collect();
        before.operations.release_cold_index();
        Self {
            before,
            basis,
            sources,
            index,
        }
    }

    fn locator_path(&self, id: OperationId) -> ReusedReads {
        let mut reads = ReusedReads::default();
        let existing = crate::metadata_map::lookup(
            &self.basis.descriptor.maps["operation_locators"],
            &id.to_string(),
            |kind, hash| reads.load(&self.sources, kind, hash),
        )
        .unwrap();
        assert!(existing.is_none());
        reads
    }

    fn next_receipt(&self) -> OperationRecordEntry {
        // Choose a generated, absent ID landing in an occupied old leaf. This
        // exercises untouched co-resident locator rows, not an empty new path.
        loop {
            let entry = receipt(self.before.sequence + 1);
            assert!(!self.index.contains_key(&entry.result.operation_id));
            if self
                .locator_path(entry.result.operation_id)
                .calls
                .keys()
                .any(|(kind, _)| *kind == "state-page")
            {
                return entry;
            }
        }
    }

    fn append(&mut self, entry: OperationRecordEntry, case: &str) {
        let id = entry.result.operation_id;
        let ordinal = self.before.operations.len();
        assert_eq!(entry.result.sequence, self.before.sequence + 1);
        assert!(self.before.operations.cold_index().is_none());
        let expected_reads = self.locator_path(id);
        assert!(
            expected_reads
                .calls
                .keys()
                .any(|(kind, _)| *kind == "state-page")
        );
        let mut after = self.before.clone();
        after.begin_tracking();
        after.sequence += 1;
        after.operations.push(entry).unwrap();
        assert!(after.operations.cold_index().is_none());
        assert_eq!(after.operations.len(), ordinal + 1);
        assert_eq!(
            &after.operations.sealed()[..self.before.operations.sealed().len()],
            self.before.operations.sealed()
        );

        // Fixture construction and the independent path probe are outside the
        // measurement. This counter runs in op_dir_entry, before map encoding,
        // so pre-serializing a full prefix cannot hide behind reused hashes.
        SEGMENT_DIRECTORY_SERIALIZATIONS.with(|counts| counts.borrow_mut().clear());
        take_encoding_counts();
        let mut reads = ReusedReads::default();
        let actual = prepare_incremental(
            &self.before,
            &after,
            &self.basis,
            MAX_METADATA,
            &mut |kind, hash| reads.load(&self.sources, kind, hash),
        )
        .unwrap();
        let directory_counts =
            SEGMENT_DIRECTORY_SERIALIZATIONS.with(|counts| counts.borrow().clone());
        let field_counts = take_encoding_counts();
        assert_incremental_receipt_cost(
            &actual,
            &directory_counts,
            &field_counts,
            &reads,
            &expected_reads,
        );
        assert_eq!(actual.descriptor.events, self.basis.descriptor.events);
        assert_ne!(
            actual.descriptor.maps["operation_log"],
            self.basis.descriptor.maps["operation_log"]
        );
        assert_ne!(
            actual.descriptor.maps["operation_locators"],
            self.basis.descriptor.maps["operation_locators"]
        );
        eprintln!(
            "{case}: receipts={ordinal}, sealed={}, directory_values={}, prepared_bytes={}, locator_callback_loads={}, locator_distinct_addresses={}, locator_bytes={}",
            self.before.operations.sealed().len(),
            directory_counts["operation_log"],
            actual
                .objects
                .iter()
                .map(|object| object.bytes.len())
                .sum::<usize>(),
            reads.calls.values().sum::<usize>(),
            reads.calls.len(),
            reads.bytes,
        );

        // All measured observations are captured before deliberate full work.
        assert!(self.index.insert(id, ordinal).is_none());
        assert_receipt_full_oracle(&actual, &after, &self.index);
        self.basis = actual.reuse_basis();
        self.sources.extend(
            actual
                .objects
                .into_iter()
                .map(|object| ((object.kind, object.hash), object.bytes)),
        );
        let located = crate::metadata_map::lookup(
            &self.basis.descriptor.maps["operation_locators"],
            &id.to_string(),
            |kind, hash| load_source(&self.sources, kind, hash),
        )
        .unwrap();
        assert_eq!(located, Some(json_codec::Value::from(ordinal.to_string())));
        self.before = after;
    }
}

fn assert_incremental_receipt_cost(
    actual: &PreparedSnapshot,
    directory_counts: &BTreeMap<&'static str, usize>,
    field_counts: &BTreeMap<String, usize>,
    reads: &ReusedReads,
    expected_reads: &ReusedReads,
) {
    // update::plan asks for at most the 128 old leaf keys plus one new key;
    // preparing an open tail can encode its directory value once separately.
    assert!(
        (1..=130).contains(&directory_counts["operation_log"]),
        "{directory_counts:?}"
    );
    assert_eq!(directory_counts.get("events").copied().unwrap_or(0), 0);
    assert_eq!(field_counts.get("operation_log").copied().unwrap_or(0), 0);
    assert_eq!(
        field_counts.get("operation_locators").copied().unwrap_or(0),
        0
    );
    assert_eq!(reads, expected_reads);
    assert!(reads.calls.values().all(|calls| *calls == 1), "{reads:?}");
    assert_eq!(
        reads
            .calls
            .keys()
            .filter(|(kind, _)| *kind == "state-page")
            .count(),
        1
    );
    // One memoized root-to-leaf path: at most 64 radix indexes and one leaf.
    assert!(reads.calls.len() <= 65);
    assert!(reads.bytes <= 64 * 1024 + (reads.calls.len() - 1) * 4 * 1024);

    let mut bodies = 0;
    let mut leaf_bytes = 0;
    let mut index_bytes = 0;
    for object in &actual.objects {
        match object.kind {
            OPERATION_SEGMENT_KIND => {
                bodies += 1;
                assert!(object.bytes.len() < MAX_METADATA);
            }
            "state-page" => leaf_bytes += object.bytes.len(),
            "metadata-index" => index_bytes += object.bytes.len(),
            "snapshot" => assert!(object.bytes.len() < 16 * 1024),
            kind => panic!("receipt-only append unexpectedly prepared {kind}"),
        }
    }
    assert_eq!(bodies, 1);
    // Two dirty maps, each with one <=64 KiB old leaf plus one small row.
    // Splitting only adds bounded wrappers; a 16-child index fits 4 KiB.
    assert!(leaf_bytes <= 2 * (64 * 1024 + 512));
    assert!(index_bytes <= 2 * 64 * 4 * 1024);
}

fn assert_receipt_full_oracle(
    actual: &PreparedSnapshot,
    after: &State,
    index: &BTreeMap<OperationId, u64>,
) {
    let mut oracle = after.clone();
    oracle.operations.attach_cold_index(index.clone()).unwrap();
    let full = prepare(&oracle, MAX_METADATA).unwrap();
    assert_eq!(actual.hash, full.hash);
    assert_eq!(
        canonical_json(&actual.descriptor).unwrap(),
        canonical_json(&full.descriptor).unwrap()
    );
    let expected = full
        .objects
        .iter()
        .map(|object| ((object.kind, object.hash.as_str()), &object.bytes))
        .collect::<BTreeMap<_, _>>();
    for object in &actual.objects {
        assert_eq!(
            &object.bytes,
            expected[&(object.kind, object.hash.as_str())]
        );
    }
}

#[test]
fn receipt_append_encodes_dirty_directory_leaf_and_loads_one_locator_path_at_three_scales() {
    for count in RECEIPT_PREFIXES {
        let mut fixture = WarmReceipts::new(receipt_prefix(count));
        fixture.append(fixture.next_receipt(), "nonseal");
        assert_eq!(
            fixture.before.operations.sealed().len(),
            count / OPERATION_SEGMENT_RECORDS
        );
        assert_eq!(fixture.before.operations.tail().len(), 1);
        assert!(fixture.before.operations.fresh_seal().is_none());
    }
}

#[test]
fn receipt_count_seal_and_next_tail_encode_only_dirty_leaves_at_three_scales() {
    for count in RECEIPT_PREFIXES {
        let mut before = receipt_prefix(count);
        for _ in 0..OPERATION_SEGMENT_RECORDS - 1 {
            before.sequence += 1;
            before.operations.push(receipt(before.sequence)).unwrap();
        }
        assert_eq!(before.operations.tail().len(), 255);
        let tail_bytes = before
            .operations
            .tail()
            .iter()
            .map(|entry| canonical_json(entry).unwrap().len())
            .sum::<usize>();
        assert!(tail_bytes < OPERATION_SEGMENT_TARGET_BYTES);
        let mut fixture = WarmReceipts::new(before);
        let entry = fixture.next_receipt();
        assert!(
            tail_bytes + canonical_json(&entry).unwrap().len() < OPERATION_SEGMENT_TARGET_BYTES
        );
        fixture.append(entry, "count-seal");
        assert_eq!(
            fixture.before.operations.sealed().len(),
            count / OPERATION_SEGMENT_RECORDS + 1
        );
        assert!(fixture.before.operations.tail().is_empty());
        assert_eq!(
            fixture
                .before
                .operations
                .fresh_seal()
                .unwrap()
                .1
                .records
                .len(),
            256
        );
        fixture.append(fixture.next_receipt(), "post-count-seal-tail");
        assert_eq!(
            fixture.before.operations.sealed().len(),
            count / OPERATION_SEGMENT_RECORDS + 1
        );
        assert_eq!(fixture.before.operations.tail().len(), 1);
        assert!(fixture.before.operations.fresh_seal().is_none());
    }
}

#[test]
fn receipt_byte_first_hit_and_adjacent_tail_encode_only_dirty_leaves_at_three_scales() {
    for count in RECEIPT_PREFIXES {
        let prefix = receipt_prefix(count);
        for record_bytes in [
            OPERATION_SEGMENT_TARGET_BYTES - 1,
            OPERATION_SEGMENT_TARGET_BYTES,
        ] {
            let mut fixture = WarmReceipts::new(prefix.clone());
            let mut entry = fixture.next_receipt();
            entry.result.result = json_codec::json!({"padding": ""});
            let envelope_bytes = canonical_json(&entry).unwrap().len();
            entry.result.result =
                json_codec::json!({"padding": "x".repeat(record_bytes - envelope_bytes)});
            assert_eq!(canonical_json(&entry).unwrap().len(), record_bytes);
            fixture.append(entry, "byte-first-hit");
            if record_bytes < OPERATION_SEGMENT_TARGET_BYTES {
                assert_eq!(
                    fixture.before.operations.sealed().len(),
                    count / OPERATION_SEGMENT_RECORDS
                );
                assert_eq!(fixture.before.operations.tail().len(), 1);
                assert!(fixture.before.operations.fresh_seal().is_none());
                fixture.append(fixture.next_receipt(), "byte-crossing-open-tail");
                assert_eq!(
                    fixture
                        .before
                        .operations
                        .fresh_seal()
                        .unwrap()
                        .1
                        .records
                        .len(),
                    2
                );
            } else {
                assert_eq!(
                    fixture
                        .before
                        .operations
                        .fresh_seal()
                        .unwrap()
                        .1
                        .records
                        .len(),
                    1
                );
            }
            assert_eq!(
                fixture.before.operations.sealed().len(),
                count / OPERATION_SEGMENT_RECORDS + 1
            );
            assert!(fixture.before.operations.tail().is_empty());
        }
    }
}
