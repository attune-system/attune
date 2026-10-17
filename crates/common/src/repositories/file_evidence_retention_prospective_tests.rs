//! Pure accounting/identity checks. Repository custody is exercised separately.
use super::*;

fn chain() -> Closure {
    Closure {
        native_bytes: BTreeMap::from([(1, 100), (2, 250), (3, 400)]),
        native_predecessors: BTreeMap::from([(1, None), (2, Some(1)), (3, Some(2))]),
        ..Closure::default()
    }
}

#[test]
fn predecessor_is_a_native_archive_not_a_receipt() {
    use super::super::native_input_snapshot::NativeStorageParent as P;
    let native =
        FileEvidenceRetentionRepository::native_parent(&P::NativeSnapshot { snapshot: 9 }).unwrap();
    let receipt =
        FileEvidenceRetentionRepository::native_parent(&P::Receipt { receipt: 9 }).unwrap();
    assert_eq!(native, Node::new("runtime_native_input_snapshot", 9));
    assert_ne!(native, receipt);
    assert!(
        FileEvidenceRetentionRepository::native_parent(&P::NativeSnapshot { snapshot: 0 }).is_err()
    );
}

#[test]
fn cumulative_receipt_charges_cover_predecessors_before_inventory_visits() {
    let mut c = chain();
    c.charge_receipt(600, &[(3, 400)]).unwrap();
    assert_eq!(c.bytes, 600);
    assert_eq!(c.charge_native(2).unwrap(), 0);
    assert_eq!(c.charge_native(1).unwrap(), 0);
    c.charge_receipt(800, &[(2, 250), (3, 400)]).unwrap();
    assert_eq!(
        c.bytes, 750,
        "keep each receipt's non-native charges, deduct only exact overlapping stored native rows"
    );
    assert_eq!(c.charged_native, BTreeSet::from([1, 2, 3]));
}

#[test]
fn standalone_inventory_then_successor_receipt_charges_each_row_once() {
    let mut c = chain();
    let bytes = c.charge_native(2).unwrap();
    c.charge_bytes(bytes).unwrap();
    assert_eq!(c.bytes, 250);
    c.charge_receipt(600, &[(3, 400)]).unwrap();
    assert_eq!(c.bytes, 600);
    assert_eq!(c.charge_native(3).unwrap(), 0);
}

#[test]
fn shared_predecessor_dedup_does_not_erase_unrelated_receipt_costs() {
    let mut c = chain();
    c.native_bytes.insert(4, 450);
    c.native_predecessors.insert(4, Some(1));
    c.charge_receipt(1050, &[(3, 400), (4, 450)]).unwrap();
    assert_eq!(c.bytes, 950);
    c.charge_bytes(25).unwrap();
    assert_eq!(
        c.bytes, 975,
        "extra physical association material shares the existing bound"
    );
    assert!(c.charge_receipt(399, &[(3, 400)]).is_err());
}

#[test]
fn chain_bound_is_128_and_accounting_never_accepts_an_unverified_subtotal() {
    let mut c = Closure::default();
    for id in 1..=128 {
        c.native_bytes.insert(id, id as usize * 100);
        c.native_predecessors
            .insert(id, if id == 1 { None } else { Some(id - 1) });
    }
    assert_eq!(c.charge_native(128).unwrap(), 12800);
    c.native_bytes.insert(129, 12900);
    c.native_predecessors.insert(129, Some(128));
    assert!(c.charge_native(129).is_err());
    let mut c = chain();
    assert!(c.charge_receipt(600, &[(3, 399)]).is_err());
    c.native_bytes.insert(3, 200);
    assert!(c.charge_native(3).is_err());
}
