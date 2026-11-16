//! Structural inventory tests only. These values never authorize roots or I/O.
use super::*;
use storage_contract::file_container::ContainerManagementAreas;

fn disk(value: u8) -> DiskId {
    format!("disk_{value:032x}").parse().unwrap()
}

fn prefix(source: DiskId, seed: u64) -> ContainerPrefix {
    let entity = |offset| ContainerEntity::new(7, seed + offset);
    ContainerPrefix {
        source_disk: source,
        preparation: format!("directory_{seed:032x}").parse().unwrap(),
        entity: entity(0),
        areas: ContainerAreas {
            common_data: entity(1),
            deleted_data: entity(2),
            zxs_data: ContainerManagementAreas {
                root: entity(3), common_data: entity(4), deleted_data: entity(5),
            },
        },
    }
}

#[test]
fn source_prefix_inventory_preserves_target_parent_and_full_entities() {
    let (a, b, c) = (disk(1), disk(2), disk(3));
    let ba = prefix(a, 10);
    let cb = prefix(b, 20);
    let cba = prefix(a, 30);
    let mut inventory = Prefixes::default();
    collect_chain(b, a, &[ba.clone()], &mut inventory).unwrap();
    collect_chain(c, a, &[cb.clone(), cba.clone()], &mut inventory).unwrap();
    // A second FILE or directory occurrence repeats the same three addresses.
    collect_chain(c, a, &[cb.clone(), cba.clone()], &mut inventory).unwrap();
    assert_eq!(inventory.occurrences.len(), 3);
    assert_eq!(inventory.at(b, None, a).unwrap(), Some(&ba));
    assert_eq!(inventory.at(c, None, b).unwrap(), Some(&cb));
    assert_eq!(inventory.at(c, Some(b), a).unwrap(), Some(&cba));
    inventory.extend_chain(c, &[cb.clone(), cba.clone()]).unwrap();
    let records = inventory.into_records().collect::<BTreeMap<_, _>>();
    assert_eq!(records, BTreeMap::from([((b, a), ba), ((c, b), cb), ((c, a), cba)]));
}

#[test]
fn source_prefix_inventory_rejects_reparenting_even_with_identical_record() {
    let (a, b, c) = (disk(1), disk(2), disk(3));
    let cb = prefix(b, 20);
    let cba = prefix(a, 30);
    let mut inventory = Prefixes::default();
    collect_chain(c, a, &[cb.clone(), cba.clone()], &mut inventory).unwrap();
    // The same source UUID, preparation, prefix and five areas are insufficient
    // to reinterpret C/B/A as C/A. Neither live reuse nor cold feedback may do so.
    let hot = inventory.at(c, None, a).err().unwrap();
    assert!(matches!(hot, crate::Error::FreshLayout(Failure::ContainerIdentity)));
    let cold = inventory.extend_chain(c, &[cba.clone()]).unwrap_err();
    assert!(matches!(cold, crate::Error::FreshLayout(Failure::ContainerIdentity)));
    let shape = collect_chain(c, a, &[cba.clone()], &mut inventory).unwrap_err();
    assert!(matches!(shape, crate::Error::FreshLayout(Failure::ContainerShape)));
    assert_eq!(inventory.at(c, Some(b), a).unwrap(), Some(&cba));
    assert_eq!(inventory.at(c, None, b).unwrap(), Some(&cb));
    assert_eq!(inventory.occurrences.len(), 2);
}

#[test]
fn source_prefix_inventory_rejects_entity_or_preparation_substitution() {
    let (a, b, c) = (disk(1), disk(2), disk(3));
    let cb = prefix(b, 20);
    let cba = prefix(a, 30);
    let mut inventory = Prefixes::default();
    collect_chain(c, a, &[cb.clone(), cba.clone()], &mut inventory).unwrap();
    let mut changed = Vec::new();
    let mut replacement = cba.clone();
    replacement.preparation = prefix(a, 100).preparation;
    changed.push(replacement);
    let mut replacement = cba.clone();
    replacement.entity = ContainerEntity::new(7, 100);
    changed.push(replacement);
    for area in 0..5 {
        let mut replacement = cba.clone();
        let target = match area {
            0 => &mut replacement.areas.common_data,
            1 => &mut replacement.areas.deleted_data,
            2 => &mut replacement.areas.zxs_data.root,
            3 => &mut replacement.areas.zxs_data.common_data,
            _ => &mut replacement.areas.zxs_data.deleted_data,
        };
        *target = ContainerEntity::new(7, 100);
        changed.push(replacement);
    }
    for replacement in changed {
        let error = inventory.extend_chain(c, &[cb.clone(), replacement]).unwrap_err();
        assert!(matches!(error, crate::Error::FreshLayout(Failure::ContainerIdentity)));
        assert_eq!(inventory.at(c, Some(b), a).unwrap(), Some(&cba));
    }
    inventory.extend_chain(c, &[cb, cba]).unwrap();
}
