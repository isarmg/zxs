use super::*;

pub(super) type Directory = json_codec::Map<String, json_codec::Value>;

#[derive(Clone, Default)]
pub(super) struct Graph {
    pub events: Directory,
    pub operations: Directory,
    pub locators: Directory,
    pub objects: BTreeMap<String, json_codec::Value>,
}

pub(super) fn receipt(id: u64, sequence: u64) -> OperationRecordEntry {
    OperationRecordEntry {
        principal: "owner".into(),
        result: OperationResult {
            operation_id: format!("op_{id:032x}").parse().unwrap(),
            kind: "metadata-adversarial".into(),
            commit_id: None,
            history_tree_id: None,
            branch_id: None,
            commit_state: storage_contract::CommitState::Published,
            protection_at_publish: storage_contract::ProtectionAtPublish::NotApplicable,
            current_health: storage_contract::CurrentHealth::Healthy,
            request_digest: "fixture-digest".into(),
            sequence,
            result: json_codec::json!({}),
        },
    }
}

pub(super) fn event(sequence: u64) -> Event {
    Event {
        sequence,
        operation_id: receipt(sequence, sequence).result.operation_id,
        kind: "metadata-adversarial".into(),
        object_id: "fixture".into(),
    }
}

impl Graph {
    pub fn object(&mut self, kind: &'static str, value: json_codec::Value) -> String {
        // Encoding and hashing are shared helpers; expected validity never
        // calls the production reader or semantic validator.
        let hash = object_hash(kind, &value).unwrap();
        self.objects.insert(format!("{kind}/{hash}"), value);
        hash
    }

    pub fn events(&mut self, ordinal: u64, sequences: &[u64]) {
        let segment = EventSegment {
            format: Default::default(),
            events: sequences.iter().copied().map(event).collect(),
        };
        let hash = self.object(EVENT_SEGMENT_KIND, json_codec::to_value(&segment).unwrap());
        self.events.insert(
            format!("{ordinal:020}"),
            json_codec::to_value(EventDirectoryEntry {
                kind: Default::default(),
                hash,
                first_sequence: sequences.first().copied().unwrap_or(0),
            })
            .unwrap(),
        );
    }

    pub fn receipts(
        &mut self,
        ordinal: u64,
        first_ordinal: u64,
        records: Vec<OperationRecordEntry>,
    ) {
        for (offset, record) in records.iter().enumerate() {
            self.locators.insert(
                record.result.operation_id.to_string(),
                (first_ordinal + offset as u64).to_string().into(),
            );
        }
        let first_sequence = records
            .first()
            .map(|record| record.result.sequence)
            .unwrap_or(0);
        let count = records.len() as u64;
        let segment = OperationSegment {
            format: Default::default(),
            records,
        };
        let hash = self.object(
            OPERATION_SEGMENT_KIND,
            json_codec::to_value(&segment).unwrap(),
        );
        self.operations.insert(
            format!("{ordinal:020}"),
            json_codec::to_value(OperationDirectoryEntry {
                kind: Default::default(),
                hash,
                first_sequence,
                first_ordinal,
                records: count,
            })
            .unwrap(),
        );
    }

    pub fn snapshot(&self, sequence: u64) -> (Snapshot, BTreeMap<String, json_codec::Value>) {
        let base = prepare_snapshot(&State::default(), MAX_METADATA).unwrap();
        let mut descriptor = base.descriptor;
        descriptor.sequence = sequence;
        let mut encoder = crate::metadata_map::Encoder::new(MAX_METADATA);
        descriptor.events = encoder.map(self.events.clone()).unwrap();
        descriptor.maps.insert(
            "operation_log".into(),
            encoder.map(self.operations.clone()).unwrap(),
        );
        descriptor.maps.insert(
            "operation_locators".into(),
            encoder.map(self.locators.clone()).unwrap(),
        );
        let mut objects = self.objects.clone();
        for object in base.objects.into_iter().chain(encoder.finish()) {
            objects.insert(
                format!("{}/{}", object.kind, object.hash),
                json_codec::from_slice(&object.bytes).unwrap(),
            );
        }
        (descriptor, objects)
    }

    pub fn resolve(&self, sequence: u64) -> Result<State> {
        let (snapshot, objects) = self.snapshot(sequence);
        resolve_snapshot(snapshot, |kind, hash| {
            let address = format!("{kind}/{hash}");
            let value = objects
                .get(&address)
                .unwrap_or_else(|| panic!("missing fixture {address}"));
            AuthenticatedMetadataObject::authenticate(kind, hash, value.clone())
        })
    }
}

pub(super) fn expect_error(graph: &Graph, sequence: u64, code: ErrorCode, message: &str) {
    let error = graph.resolve(sequence).unwrap_err();
    assert_eq!(error.code(), code);
    assert_eq!(error.message(), message);
    assert!(!error.is_retryable());
}

pub(super) fn full_receipts(start_id: u64, sequence: u64) -> Vec<OperationRecordEntry> {
    (0..OPERATION_SEGMENT_RECORDS as u64)
        .map(|offset| receipt(start_id + offset, sequence))
        .collect()
}
