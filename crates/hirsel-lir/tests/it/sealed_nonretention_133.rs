//! Forged borrowed-data promises must fail independently of source summaries.
use crate::support::*;
use hirsel_core_contract::Passing;
use hirsel_lir::{abi_types::ParameterPath, ir::*, stage::Ready};

fn capture(scoped: bool) -> Vec<K> {
    use hirsel_lir::layout_types::*;
    let mut t = Lir::<Ready>::new(&[(Passing::Borrow, l::SEQBLK)], None);
    if scoped {
        let abi = &mut t.table.fn_abis[t.builder.unit.header.abi.0 as usize];
        abi.nonretaining = vec![ParameterPath {
            parameter: 0,
            fields: vec![],
        }];
        abi.scoped_contract = hirsel_lir::abi::scoped_contract_digest(&abi.nonretaining, &[]);
    }
    let slot = t.slot(l::SEQBLK, SlotKind::Parameter(0), ROOT);
    let root = t.version_of(slot);
    t.param(root);
    let address = t.at(ENTRY, slot, &[], &[]);
    let copied = t.owned(l::SEQBLK);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Copy {
            result: copied,
            address,
            reaching: root,
            failure,
        },
    );
    let mut environment = t.table.layouts[l::SEQBLK.0 as usize].clone();
    environment.kind = LayoutKind::Aggregate;
    environment.fields = vec![FieldLayout {
        ordinal: 0,
        offset: 0,
        layout: l::SEQBLK,
        unaligned: false,
    }];
    environment.copy = CopyRecipe::Fields;
    let layout = hirsel_lir::LayoutId(t.table.layouts.len() as u32);
    t.table.layouts.push(environment);
    let abi = t.abi_shaped(hirsel_lir::abi_types::CallableShape::Closure, &[], None);
    let symbol = t.symbol(SymbolKind::Function);
    let captures = PoolRange::push(
        &mut t.builder.unit.pools.captures,
        [CaptureValue {
            value: copied,
            usage: CaptureUse::Take,
        }],
    );
    let closure = ClosureId(t.builder.unit.tables.closures.len() as u32);
    let placement = t.heap();
    t.builder.unit.tables.closures.push(ClosureSite {
        receipts: None,
        scoped: false,
        code: Some(ClosureCode { symbol, abi }),
        environment: layout,
        captures,
        destination: DestinationClass::Heap,
        placement,
        anchor: SourceAnchorId(0),
    });
    let value = t.owned(l::ERASED);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Closure {
            result: value,
            closure,
            failure,
        },
    );
    let anchor = t.instruction_anchor();
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Release {
            value,
            layout: l::ERASED,
            live: MaskRef::All,
            anchor,
            failure,
        },
    );
    t.builder.ret(ENTRY, None);
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
fn data_nonretention_rejects_an_owning_closure_capture_with_an_ordinary_control() {
    assert_eq!(capture(false), []);
    let errors = capture(true);
    assert!(
        errors.iter().any(|e| matches!(
            e,
            K::ScopedRetention {
                operation: "owning capture",
                ..
            }
        )),
        "{errors:?}"
    );
}
