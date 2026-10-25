use super::*;

fn hash(name: &str) -> ContentHash {
    ContentHash::of_parts("receipt-contract-test", [name.as_bytes()])
}
fn ty() -> ReceiptType {
    ReceiptType {
        source_type: hash("original-type"),
        profile: hash("closed-profile"),
    }
}
fn parameter(source: u32, mode: ReceiptMode) -> ReceiptParameter {
    ReceiptParameter {
        source,
        ty: ty(),
        mode,
    }
}
fn contract() -> ReceiptContract {
    ReceiptContract::new(
        vec![
            parameter(0, ReceiptMode::Borrow),
            parameter(3, ReceiptMode::MutSlot),
        ],
        Some(ReceiptResult { ty: ty() }),
        vec![ReceiptCapture {
            ordinal: 2,
            ty: ty(),
            mode: ReceiptMode::Take,
        }],
        vec![
            ReceiptGrant::Parameter(1),
            ReceiptGrant::Capture(4),
            ReceiptGrant::CallerDestination,
        ],
    )
    .unwrap()
}

#[test]
fn complete_original_contract_round_trips_without_candidate_ids() {
    let original = contract();
    let text = hirsel_codec::to_text(&original);
    let decoded: ReceiptContract = hirsel_codec::from_text(&text).unwrap();
    assert_eq!(decoded, original);
    decoded.check_structure().unwrap();
    assert_eq!(decoded.content(), original.content());
    assert_eq!(decoded.parameters[1].source, 3);
    assert_eq!(decoded.captures[0].ordinal, 2);
    assert_eq!(decoded.parameters[1].mode, ReceiptMode::MutSlot);
}

#[test]
fn selector_order_duplicates_and_unbound_type_products_are_refused() {
    for parameters in [
        vec![
            parameter(2, ReceiptMode::Borrow),
            parameter(1, ReceiptMode::Take),
        ],
        vec![
            parameter(1, ReceiptMode::Borrow),
            parameter(1, ReceiptMode::MutSlot),
        ],
    ] {
        assert!(ReceiptContract::new(parameters, None, vec![], vec![]).is_err());
    }
    let capture = ReceiptCapture {
        ordinal: 2,
        ty: ty(),
        mode: ReceiptMode::Borrow,
    };
    assert!(ReceiptContract::new(vec![], None, vec![capture.clone(), capture], vec![]).is_err());
    assert!(
        ReceiptContract::new(
            vec![],
            None,
            vec![],
            vec![ReceiptGrant::Capture(0), ReceiptGrant::Parameter(0)]
        )
        .is_err()
    );
    assert!(
        ReceiptContract::new(vec![], None, vec![], vec![ReceiptGrant::Parameter(0); 2]).is_err()
    );
    for field in 0..2 {
        let mut input = parameter(0, ReceiptMode::Take);
        if field == 0 {
            input.ty.source_type = ContentHash([0; 32]);
        } else {
            input.ty.profile = ContentHash([0; 32]);
        }
        assert!(ReceiptContract::new(vec![input], None, vec![], vec![]).is_err());
    }
}

#[test]
fn raw_decoding_does_not_authorize_a_forged_or_stale_demand_hash() {
    let good = contract();
    let mut forged = good.clone();
    forged.source_demand = hash("forged");
    let decoded: ReceiptContract =
        hirsel_codec::from_text(&hirsel_codec::to_text(&forged)).unwrap();
    assert!(decoded.check_structure().is_err());
    assert!(decoded.content().is_err());
    let mut stale = good;
    stale.parameters[1].mode = ReceiptMode::Take;
    assert!(stale.canonical_bytes().is_err());
    stale.schema += 1;
    assert!(stale.check_structure().is_err());
}

#[test]
fn every_transport_axis_changes_the_semantic_output_identity() {
    let good = contract();
    let base = good.content().unwrap();
    for axis in 0..8 {
        let mut changed = good.clone();
        match axis {
            0 => changed.parameters[0].source = 1,
            1 => changed.parameters[0].mode = ReceiptMode::Take,
            2 => changed.parameters[0].ty.source_type = hash("other-type"),
            3 => changed.parameters[0].ty.profile = hash("other-profile"),
            4 => changed.result = None,
            5 => changed.captures[0].ordinal = 3,
            6 => changed.captures[0].mode = ReceiptMode::Borrow,
            7 => changed.inventory.pop().map(|_| ()).unwrap(),
            _ => unreachable!(),
        }
        let rebuilt = ReceiptContract::new(
            changed.parameters,
            changed.result,
            changed.captures,
            changed.inventory,
        )
        .unwrap();
        assert_ne!(rebuilt.content().unwrap(), base, "axis {axis}");
        assert_ne!(rebuilt.source_demand, good.source_demand, "axis {axis}");
    }
    // Repeating an equal semantic output retains its logical identity; the
    // record deliberately carries no source revision or consuming ABI key.
    assert_eq!(contract().content().unwrap(), base);
}
