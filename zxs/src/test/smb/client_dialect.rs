//! No credentials. A GUID/dialect pin is metadata belonging to the server
//! run, never proof that two connections or their users are interchangeable.
use super::*;
use crate::smb::connection::sessions::tests::{connected,running,try_connected};

#[test]
fn original_negotiated_transports_pin_one_client_and_retire_only_after_the_last_disconnect() {
    let run=running([3;16]);let a=connected(&run);let b=connected(&run);
    assert_eq!(run.run.index.lock().unwrap().transports.len(),2);
    assert!(matches!(AcceptSeat::reserve(&run.run).unwrap().promote(&a),Err(Fault::SessionState)));
    assert!(run.run.index.lock().unwrap().clients.is_empty());
    a.pin_client_dialect().unwrap();b.pin_client_dialect().unwrap();
    {let mut g=run.run.index.lock().unwrap();assert_eq!(g.clients.len(),1);assert_eq!(g.clients[0].selected,0x0311);
        g.clients[0].selected=0x0302;} // hypothetical conflicting entry, never a supported production dialect
    assert!(matches!(b.pin_client_dialect(),Err(Fault::ClientDialect)));
    drop(a);
    {let mut g=run.run.index.lock().unwrap();g.prune_transports();assert_eq!(g.transports.len(),1);assert_eq!(g.clients.len(),1);}
    drop(b);
    {let mut g=run.run.index.lock().unwrap();g.prune_transports();assert!(g.transports.is_empty());assert!(g.clients.is_empty());}
    let new=connected(&run);new.pin_client_dialect().unwrap();
}
#[test]
fn negotiated_origin_registry_is_bounded_without_leaking_abandoned_transports() {
    let run=running([8;16]);
    let mut live:Vec<_>=(0..TRANSPORT_CAP).map(|_|connected(&run)).collect();
    assert!(matches!(try_connected(&run),Err(Fault::SessionLimit)));
    assert_eq!(run.run.index.lock().unwrap().transports.len(),TRANSPORT_CAP);
    drop(live.pop());let _replacement=connected(&run);
    assert_eq!(run.run.index.lock().unwrap().transports.len(),TRANSPORT_CAP);
}
