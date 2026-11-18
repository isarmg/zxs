use super::*;
use crate::{Config,Engine,engine::MutationPlan};
use storage_contract::{OperationId,RepositoryName};

#[test]
fn first_real_root_freezes_all_holders_and_original_snapshot_without_creating_a_decision() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = Config::development([
        (name_policy::Name::parse("A").unwrap(),tmp.path().join("a")),
        (name_policy::Name::parse("B").unwrap(),tmp.path().join("b")),
    ]).unwrap();
    let slot = cfg.disks[0].slot_id.clone();
    let engine = Engine::initialize(&tmp.path().join("system"),cfg).unwrap();
    let op = OperationId::generate().unwrap();
    let name = RepositoryName::parse(&format!("ZXS disk {slot}")).unwrap();
    let MutationPlan::Prepared { next, prepared, result } = engine.plan_repository_creation(
        &name,&slot,1,"alice",op,true,
    ).unwrap() else { panic!("expected a real new root request") };
    let detached = engine.prepare_publication(*next,prepared,result,crate::disk::MAX_METADATA).unwrap();
    let input = engine.first_root_scope_input(&detached).unwrap();
    let plain = json_codec::to_value(input.state_descriptor().unwrap()).unwrap();
    let stop = AtomicBool::new(false);
    let graph = FirstRootGraph::capture(input,&stop).unwrap();
    assert_eq!(graph.disks.len(),2);
    let mut full: json_codec::Value = json_codec::from_slice(graph.snapshot_object().bytes()).unwrap();
    let scope = full.as_object_mut().unwrap().remove("scope_closure").unwrap();
    assert_eq!(scope,json_codec::to_value(graph.commitment()).unwrap());
    assert_eq!(full,plain); // same actual native maps/events/protected reference
    assert_ne!(graph.commitment().hash().to_string(),graph.snapshot_object().hash);
    let untouched = graph.disks.values().next_back().unwrap().held.content_root().unwrap().path().join("unregistered");
    std::fs::write(&untouched,b"leave me alone").unwrap();
    assert!(graph.stage_original(&stop).is_err());
    assert_eq!(std::fs::read(&untouched).unwrap(),b"leave me alone");
    std::fs::remove_file(untouched).unwrap();
    graph.stage_original(&stop).unwrap();
    assert_eq!(engine.state().sequence,0);
    assert!(!engine.state().operations.contains_key(&op));
    for disk in graph.disks.values() {
        let root = &disk.held.root;
        let recorded: Closure = super::super::objects::load(root,Domain::Closure,graph.commitment()).unwrap();
        assert_eq!(recorded.counts.inventory_entries,0);
        assert_eq!(recorded.counts.materials,5);
        assert_eq!(recorded.sequence,Sequence::new(1));
        assert!(matches!(recorded.origin, Origin::Publication {operation_id,..} if operation_id == op));
        assert!(matches!(recorded.membership, Membership::Genesis {..}));
    }
}
