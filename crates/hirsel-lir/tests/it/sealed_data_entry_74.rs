//! Data contracts are proved from the body, including forwarding and exits.
use crate::support::*;
use hirsel_core_contract::Passing;
use hirsel_lir::{abi_types::ParameterPath, ir::*, stage::Value};

fn contract(t: &mut Lir<Value>) {
    let abi = &mut t.table.fn_abis[t.builder.unit.header.abi.0 as usize];
    abi.nonretaining = vec![ParameterPath {
        parameter: 0,
        fields: vec![],
    }];
    abi.scoped_contract = hirsel_lir::abi::scoped_contract_digest(&abi.nonretaining, &[]);
}

fn check(t: Lir<Value>) -> Vec<K> {
    let operations = t.operations.clone();
    let (unit, table, manifest) = t.finish();
    let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
    let inputs = hirsel_lir::validate::Inputs {
        target: &target,
        layouts: &table,
        manifest: &manifest,
        operations: &operations,
        discharge: None,
        memo: Default::default(),
    };
    hirsel_lir::validate::check(&unit, &inputs)
        .into_iter()
        .map(|v| v.kind)
        .collect()
}

#[test]
fn a_data_entry_returns_an_observed_length_without_retaining_the_root() {
    let mut t = Lir::<Value>::new(&[(Passing::Borrow, l::SEQBLK)], Some(l::U64));
    contract(&mut t);
    let root = t.plain(l::SEQBLK);
    t.param(root);
    let result = t.plain(l::U64);
    t.builder.push(
        ENTRY,
        Inst::SeqLen {
            result,
            value: root,
        },
    );
    t.builder.ret(ENTRY, Some(result));
    assert_eq!(check(t), []);
}

fn returned_copy(scoped: bool, join: bool) -> Vec<K> {
    let mut t = Lir::<Value>::new(&[(Passing::Borrow, l::SEQBLK)], Some(l::SEQBLK));
    if scoped {
        contract(&mut t);
    }
    let slot = t.slot(l::SEQBLK, SlotKind::Parameter(0), ROOT);
    let root = t.version_of(slot);
    t.param(root);
    let address = t.at(ENTRY, slot, &[], &[]);
    let value = t.owned(l::SEQBLK);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Copy {
            result: value,
            address,
            reaching: root,
            failure,
        },
    );
    if join {
        let result = t.owned(l::SEQBLK);
        let next = t.builder.block(BlockKind::Body, &[result]);
        t.builder.jump(ENTRY, next, &[value]);
        t.builder.ret(next, Some(result));
    } else {
        t.builder.ret(ENTRY, Some(value));
    }
    check(t)
}

#[test]
fn a_data_contract_is_not_permission_to_return_an_owning_copy() {
    for join in [false, true] {
        assert_eq!(returned_copy(false, join), []);
        let errors = returned_copy(true, join);
        assert!(
            errors.iter().any(|k| matches!(
                k,
                K::ScopedRetention {
                    operation: "return or error payload",
                    ..
                }
            )),
            "{errors:?}"
        );
    }
}

#[test]
fn data_contracts_do_not_make_mutable_entries_read_only() {
    let mut t = Lir::<Value>::new(&[(Passing::Mut, l::SEQBLK)], None);
    contract(&mut t);
    let slot = t.slot(l::SEQBLK, SlotKind::Parameter(0), ROOT);
    let root = t.version_of(slot);
    t.param(root);
    t.builder.ret(ENTRY, None);
    assert!(check(t).iter().any(|k| matches!(k, K::Layout(detail) if detail.contains("invalid scoped nonretaining contract"))));
}

#[test]
fn forwarding_data_needs_the_actual_body_contract() {
    let mut t = Lir::<Value>::new(&[(Passing::Borrow, l::SEQBLK)], Some(l::U64));
    contract(&mut t);
    let slot = t.slot(l::SEQBLK, SlotKind::Parameter(0), ROOT);
    let root = t.version_of(slot);
    t.param(root);
    let address = t.at(ENTRY, slot, &[], &[]);
    let loan = t.loan(address, false);
    t.begin(ENTRY, loan);
    let view = t.read(ENTRY, address, root, ReadMode::Borrow, Some(loan));
    let abi = t.builder.unit.header.abi;
    let symbol = t.symbol(SymbolKind::Function);
    let next = t.builder.block(BlockKind::Body, &[]);
    let failed = t.cleanup(&[Undo::End(loan)]);
    let result = t
        .call_to(
            ENTRY,
            Callee::Direct(symbol),
            abi,
            &[(Passing::Borrow, l::SEQBLK)],
            Some(l::U64),
            &[ArgumentValue::Value(view)],
            (next, &[]),
            failed,
        )
        .unwrap();
    t.end(next, loan);
    t.builder.ret(next, Some(result));
    let operations = t.operations.clone();
    let (mut caller, table, manifest) = t.finish();
    let mut reader = Lir::<Value>::new(&[(Passing::Borrow, l::SEQBLK)], Some(l::U64));
    contract(&mut reader);
    let root = reader.plain(l::SEQBLK);
    reader.param(root);
    let length = reader.plain(l::U64);
    reader.builder.push(
        ENTRY,
        Inst::SeqLen {
            result: length,
            value: root,
        },
    );
    reader.builder.ret(ENTRY, Some(length));
    let (mut body, _, _) = reader.finish();
    body.header.definition = path("data_reader");
    body.header.instance = hash("data_reader");
    caller.tables.symbols[symbol.index()].definition = body.header.definition.clone();
    caller.tables.symbols[symbol.index()].instance = Some(body.header.instance);
    let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
    let inputs = hirsel_lir::validate::Inputs {
        target: &target,
        layouts: &table,
        manifest: &manifest,
        operations: &operations,
        discharge: None,
        memo: Default::default(),
    };
    assert!(hirsel_lir::validate::check_scoped_transport(&caller, &inputs).is_empty());
    hirsel_lir::validate::qualify_scoped_transport(&[caller.clone(), body.clone()], &inputs)
        .expect("both data bodies prove nonretention");
    assert!(
        hirsel_lir::validate::check_scoped_calls(&[caller.clone(), body.clone()], &table).is_ok()
    );
    assert!(hirsel_lir::validate::check_scoped_calls(&[caller.clone()], &table).is_err());
    let mut stripped = table.clone();
    let ordinary = hirsel_lir::FnAbiId(stripped.fn_abis.len() as u32);
    let mut header = stripped.fn_abis[abi.0 as usize].clone();
    header.nonretaining.clear();
    header.scoped_contract = hirsel_lir::abi::scoped_contract_digest(&[], &[]);
    stripped.fn_abis.push(header);
    body.header.abi = ordinary;
    assert!(hirsel_lir::validate::check_scoped_calls(&[caller.clone(), body], &stripped).is_err());
    caller.tables.calls[0].abi = ordinary;
    let inputs = hirsel_lir::validate::Inputs {
        layouts: &stripped,
        memo: Default::default(),
        ..inputs
    };
    let errors = hirsel_lir::validate::check_scoped_transport(&caller, &inputs);
    assert!(
        errors.iter().any(|v| matches!(
            v.kind,
            K::ScopedRetention {
                operation: "may-retain call argument",
                ..
            }
        )),
        "{errors:?}"
    );
}
