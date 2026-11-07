use super::*;

fn observed() -> DirectorySource {
    let candidate = super::super::super::tests::candidate("/mnt/independent");
    DirectorySource {
        path: candidate.root_path,
        discovery_identity: candidate.discovery_identity,
        identity: candidate.root_identity,
    }
}
fn preview() -> Value {
    let identity = json!({"origin":{"pool_id":"pool","disk_id":"disk","slot_id":"slot","binding_generation":"1"},"incarnation":"a".repeat(64),"role":"primary"});
    json!({"source":observed(),"topology":{
        "protocol":"independent_source_topology",
        "root":{"identity":identity.clone(),"publication":null,"sequence":"0"},
        "nodes":[{"identity":identity,"pins":[]}],"relations":[]
    }})
}

#[test]
fn independent_preview_binds_exact_source_and_rejects_extra_authority() {
    assert!(read(preview(), &observed()).is_ok());
    for change in 0..4 {
        let mut value = preview();
        match change {
            0 => value["source"]["path"] = "/mnt/other".into(),
            1 => value["source"]["identity"]["inode"] = "999".into(),
            2 => value["topology"]["writable"] = true.into(),
            3 => value["topology"]["root"]["sequence"] = "1".into(),
            _ => unreachable!(),
        }
        assert!(read(value, &observed()).is_err());
    }
}

#[test]
fn independent_preview_is_discarded_on_reselection_or_invalidation() {
    let source = observed();
    let mut model = super::super::super::Model {
        snapshot: Some(super::super::super::Snapshot {
            state: super::super::super::StorageState::Pending,
            _disk_ids: vec![],
            completion_available: false,
            allow_development: false,
            terminal: None,
        }),
        ..Default::default()
    };
    super::super::super::wizard::tests::open_discovered_source(&mut model, source.clone());
    let generation = model.foreign.independent.generation;
    assert!(current(&model, &source, generation));
    model.foreign.invalidate();
    assert!(!current(&model, &source, generation));
    assert!(model.foreign.independent.preview.is_none());
    assert!(!model.foreign.independent.reading);
}
