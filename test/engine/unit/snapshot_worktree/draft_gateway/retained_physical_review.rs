//! Only the physical operand is under test; a snapshot in this fixture is
//! deliberately *not* a permit to hide its selected FILE.
use super::{Fixture, write_draft};
use crate::container_storage::retained_file::HeldRetainedEndpoints;
use storage_contract::{OperationId,Principal};

#[test]
fn exact_original_born_file_yields_no_writes_and_refuses_an_occupied_or_stale_second_parent() {
    let (fixture,mut engine)=Fixture::new();
    let target=&fixture.targets[0];
    let actor:Principal="admin".parse().unwrap();
    let op=OperationId::generate().unwrap();
    let before=fixture.media();
    let review=HeldRetainedEndpoints::review(&engine,target,&actor,op).unwrap();
    let site=review.facts();
    assert_eq!(&site.target,target);
    assert_eq!(site.operation,op);
    assert_eq!(site.allocation.name.original_name(),target.path.file_name());
    assert_eq!(site.actor,actor);
    assert_eq!(site.base,engine.file_draft_base(target).unwrap());
    assert_eq!(site.replica,engine.state().file_containers[&target.file].replicas[&site.owner]);
    assert_eq!(fixture.media(),before);
    assert!(!engine.state().operations.contains_key(&op));
    let disk=engine.config().disks.iter().find(|d|d.disk_id==site.owner.to_string()).unwrap();
    let occupied=disk.root.join(crate::persist::layout::DELETED_DATA_ROOT.as_path())
        .join(site.allocation.name.as_str().to_ascii_uppercase());
    std::fs::create_dir(&occupied).unwrap();
    let sentinel=occupied.join("not-owned");
    std::fs::write(&sentinel,b"foreign material").unwrap();
    assert!(review.require_unmoved(&engine).is_err());
    assert_eq!(std::fs::read(&sentinel).unwrap(),b"foreign material");
    assert!(!engine.state().operations.contains_key(&op));
    // A later actual save on this FILE can never reuse that old review.
    write_draft(&mut engine,target,b"durable draft");
    assert!(review.require_unmoved(&engine).is_err());
    assert!(HeldRetainedEndpoints::review(&engine,target,&actor,OperationId::generate().unwrap()).is_err());
}
