//! Published-scope context binds every native boundary and physical target.
use super::*;
use crate::{DiskRegistrationIdentity, DiskRegistryCommitment};
use std::sync::atomic::AtomicBool;

fn identity() -> DiskRegistrationIdentity {
    DiskRegistrationIdentity {
        pool_id: format!("pool_{}", "1".repeat(32)),
        disk_id: format!("disk_{}", "2".repeat(32)),
        slot_id: "C02".into(),
        binding_generation: 7,
    }
}

fn registry(root: char, generation: u64) -> DiskRegistryCommitment {
    json_codec::from_str(&format!(
        "{{\"root_hash\":\"{}\",\"view_generation\":\"{generation}\"}}",
        root.to_string().repeat(64)
    )).unwrap()
}

fn context<'a>(
    target: &'a DiskRegistrationIdentity,
    registry: &'a DiskRegistryCommitment,
) -> Context<'a> {
    Context {
        target,
        device: 11,
        inode: 13,
        operation: "op_00000000000000000000000000000001".parse().unwrap(),
        source_fence: "source-fence",
        archive: "archive",
        candidate: "candidate",
        decision: "decision",
        publication: "publication",
        authority: "authority",
        snapshot: "snapshot",
        first_sequence: 17,
        registry,
        configuration: "configuration",
    }
}

#[test]
fn every_context_identity_binding_and_authenticated_boundary_is_in_the_key() {
    let stop = AtomicBool::new(false);
    let target = identity();
    let original_registry = registry('a', 1);
    let original = context(&target, &original_registry).digest(&stop).unwrap();
    macro_rules! changed {
        ($field:ident, $value:expr) => {{
            let mut changed = context(&target, &original_registry);
            changed.$field = $value;
            assert_ne!(changed.digest(&stop).unwrap(), original, stringify!($field));
        }};
    }
    changed!(device, 19);
    changed!(inode, 23);
    changed!(operation, "op_00000000000000000000000000000002".parse().unwrap());
    changed!(source_fence, "other");
    changed!(archive, "other");
    changed!(candidate, "other");
    changed!(decision, "other");
    changed!(publication, "other");
    changed!(authority, "other");
    changed!(snapshot, "other");
    changed!(first_sequence, 29);
    changed!(configuration, "other");
    for field in 0..4 {
        let mut altered = identity();
        match field {
            0 => altered.pool_id = format!("pool_{}", "3".repeat(32)),
            1 => altered.disk_id = format!("disk_{}", "4".repeat(32)),
            2 => altered.slot_id = "C03".into(),
            _ => altered.binding_generation += 1,
        }
        assert_ne!(context(&altered, &original_registry).digest(&stop).unwrap(), original);
    }
    for other in [registry('b', 1), registry('a', 2)] {
        assert_ne!(context(&target, &other).digest(&stop).unwrap(), original);
    }
}

