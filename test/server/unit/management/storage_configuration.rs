use super::*;

#[test]
fn configuration_chain_requires_an_exact_latest_tip_and_all_recorded_edges() {
    let mut chain = ConfigurationChain::default();
    chain.edge("initial".into(), "offline".into());
    chain.edge("offline".into(), "offline".into()); // Disabled -> Detached has the same Config.
    chain.edge("offline".into(), "added".into());
    chain.edge("added".into(), "exited-again".into());
    chain.tip("exited-again".into());
    assert!(chain.connects("exited-again", "initial"));
    assert!(!chain.connects("added", "initial"));
    assert!(!chain.connects("invented", "initial"));
    chain.predecessors.remove("offline");
    assert!(!chain.connects("exited-again", "initial"));
}

#[test]
fn configuration_chain_cycles_cannot_invent_an_activation_ancestor() {
    let mut chain = ConfigurationChain::default();
    chain.edge("second".into(), "first".into());
    chain.edge("first".into(), "second".into());
    chain.tip("second".into());
    assert!(!chain.connects("second", "initial"));
}
