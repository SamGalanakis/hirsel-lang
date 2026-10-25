//! Hand-built physical nested contracts; no source lowering or code discovery.
use crate::support::*;
use hirsel_core_contract::Passing;
use hirsel_lir::{abi_types::*, ir::*, stage::Rc};
fn nested(mutation: u8) -> Result<(), String> {
    let mut t = Lir::<Rc>::new(
        &[(Passing::Borrow, l::ERASED), (Passing::Borrow, l::ERASED)],
        None,
    );
    let leaf = t.abi_shaped(CallableShape::Closure, &[], None);
    let leaf_identity =
        hirsel_lir::abi::scoped_entry_identity(&t.table.fn_abis[leaf.0 as usize], &t.table);
    let invoke = t.abi_shaped(
        CallableShape::Closure,
        &[(Passing::Borrow, l::ERASED)],
        None,
    );
    let abi = &mut t.table.fn_abis[invoke.0 as usize];
    abi.nonretaining = vec![ParameterPath {
        parameter: 0,
        fields: vec![],
    }];
    abi.scoped_contract = hirsel_lir::abi::scoped_contract_digest(&abi.nonretaining, &[]);
    abi.scoped_entries = vec![ScopedEntry {
        parameter: ParameterPath {
            parameter: 0,
            fields: vec![],
        },
        identity: leaf_identity,
    }];
    let invoke_identity =
        hirsel_lir::abi::scoped_entry_identity(&t.table.fn_abis[invoke.0 as usize], &t.table);
    let own = &mut t.table.fn_abis[t.builder.unit.header.abi.0 as usize];
    own.nonretaining = vec![
        ParameterPath {
            parameter: 0,
            fields: vec![],
        },
        ParameterPath {
            parameter: 1,
            fields: vec![],
        },
    ];
    own.scoped_contract = hirsel_lir::abi::scoped_contract_digest(&own.nonretaining, &[]);
    own.scoped_entries = vec![
        ScopedEntry {
            parameter: ParameterPath {
                parameter: 0,
                fields: vec![],
            },
            identity: invoke_identity,
        },
        ScopedEntry {
            parameter: ParameterPath {
                parameter: 1,
                fields: vec![],
            },
            identity: leaf_identity,
        },
    ];
    match mutation {
        1 => own.scoped_entries.clear(),
        2 => own.scoped_entries[0].identity = hash("forged"),
        3 => own.scoped_entries[1].identity = invoke_identity,
        _ => {}
    }
    let mut views = Vec::new();
    let mut loans = Vec::new();
    for parameter in 0..2 {
        let slot = t.slot(l::ERASED, SlotKind::Parameter(parameter), ROOT);
        let p = t.version_of(slot);
        t.param(p);
        let undo: Vec<_> = loans.iter().copied().rev().map(Undo::End).collect();
        let address = t.at(ENTRY, slot, &[], &undo);
        let loan = t.loan(address, false);
        t.begin(ENTRY, loan);
        views.push(t.read(ENTRY, address, p, ReadMode::Borrow, Some(loan)));
        loans.push(loan);
    }
    let next = t.builder.block(BlockKind::Body, &[]);
    let undo: Vec<_> = loans.iter().copied().rev().map(Undo::End).collect();
    let failed = t.cleanup(&undo);
    t.call_to(
        ENTRY,
        Callee::Indirect(views[usize::from(mutation == 4)]),
        invoke,
        &[(Passing::Borrow, l::ERASED)],
        None,
        &[ArgumentValue::Value(views[1])],
        (next, &[]),
        failed,
    );
    for loan in loans.into_iter().rev() {
        t.end(next, loan);
    }
    t.builder.ret(next, None);
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
    let errors = hirsel_lir::validate::check(&unit, &inputs);
    assert!(errors.is_empty(), "{errors:?}");
    let mut memo =
        hirsel_lir::validate::ScopedQualification::new(&target, &manifest, &operations, None);
    memo.qualify(std::slice::from_ref(&unit), &table)?;
    memo.qualify(std::slice::from_ref(&unit), &table)?;
    let mut forged = table.clone();
    forged.fn_abis[unit.header.abi.0 as usize].scoped_entries[0].identity = hash("memo-forged");
    assert!(memo.qualify(std::slice::from_ref(&unit), &forged).is_err());
    let mut changed = unit.clone();
    changed.terminators[ENTRY.index()] = Terminator::Return(Some(views[0]));
    assert!(memo.qualify(&[changed], &table).is_err());
    memo.qualify(&[unit], &table)
}
#[test]
fn nested_contract_identity_is_serialized_and_caches_by_content() {
    let mut t = Lir::<Rc>::new(&[(Passing::Borrow, l::ERASED)], None);
    let a = t.table.fn_abis[t.builder.unit.header.abi.0 as usize].clone();
    let mut b = a.clone();
    b.scoped_entries.push(ScopedEntry {
        parameter: ParameterPath {
            parameter: 0,
            fields: vec![],
        },
        identity: hash("entry"),
    });
    let text = hirsel_codec::to_text(&b);
    assert_eq!(hirsel_codec::from_text::<FnAbi>(&text).unwrap(), b);
    assert_ne!(
        hirsel_lir::abi::abi_digest(&a),
        hirsel_lir::abi::abi_digest(&b)
    );
    let keys = hirsel_lir::layout_keys::LayoutKeys::new(&t.table);
    assert_ne!(keys.fn_abi(&a), keys.fn_abi(&b));
    let old = hirsel_lir::abi::scoped_entry_identity(&a, &t.table);
    t.table.layouts.swap(l::I64.0 as usize, l::U64.0 as usize);
    assert_eq!(old, hirsel_lir::abi::scoped_entry_identity(&a, &t.table));
}
