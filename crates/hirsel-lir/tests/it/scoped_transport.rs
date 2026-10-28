//! Contract-bearing and ordinary owning twins use the identical raw body.
//! These bodies model lowered owning releases at Ready: an erased callable
//! can destroy resources, so an Rc body must retain its semantic Drop instead.
use crate::support::*;
use hirsel_core_contract::Passing;
use hirsel_lir::abi_types::ParameterPath;
use hirsel_lir::ir::*;
use hirsel_lir::stage::{Ready, Value};

fn contract(t: &mut Lir<impl hirsel_lir::stage::Stage>, paths: Vec<ParameterPath>) {
    let abi = &mut t.table.fn_abis[t.builder.unit.header.abi.0 as usize];
    abi.nonretaining = paths;
    abi.scoped_contract =
        hirsel_lir::abi::scoped_contract_digest(&abi.nonretaining, &abi.nonretaining_captures);
}

fn check(t: Lir<impl hirsel_lir::stage::Stage>) -> Vec<K> {
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
    hirsel_lir::validate::check_scoped_transport(&unit, &inputs)
        .into_iter()
        .map(|v| v.kind)
        .collect()
}

#[derive(Clone, Copy, Debug)]
enum Route {
    Direct,
    Local,
    Join,
    Failure,
    MayRetain,
    Erased,
}
fn retain(scoped: bool, route: Route) -> Vec<K> {
    let mut t = Lir::<Ready>::new(&[(Passing::Borrow, l::ERASED)], Some(l::ERASED));
    if scoped {
        contract(
            &mut t,
            vec![ParameterPath {
                parameter: 0,
                fields: vec![],
            }],
        );
    }
    let slot = t.slot(l::ERASED, SlotKind::Parameter(0), ROOT);
    let p = t.version_of(slot);
    t.param(p);
    let a = t.at(ENTRY, slot, &[], &[]);
    let copied = t.owned(l::ERASED);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Copy {
            result: copied,
            address: a,
            reaching: p,
            failure,
        },
    );
    match route {
        Route::Direct => t.builder.ret(ENTRY, Some(copied)),
        Route::Local => {
            let local = t.slot(l::ERASED, SlotKind::Local, ROOT);
            t.builder.push(ENTRY, Inst::StorageLive { slot: local });
            let a = t.at(ENTRY, local, &[], &[Undo::Release(copied)]);
            let version = t.store(ENTRY, a, copied, local);
            let taken = t.read(ENTRY, a, version, ReadMode::Take, None);
            t.builder.push(
                ENTRY,
                Inst::StorageDead {
                    slot: local,
                    if_live: false,
                },
            );
            t.builder.ret(ENTRY, Some(taken));
        }
        Route::Join => {
            let p = t.owned(l::ERASED);
            let next = t.builder.block(BlockKind::Body, &[p]);
            t.builder.jump(ENTRY, next, &[copied]);
            t.builder.ret(next, Some(p));
        }
        Route::MayRetain => {
            let next = t.builder.block(BlockKind::Body, &[]);
            let failed = t.builder.propagate();
            let returned = t
                .call(
                    ENTRY,
                    &[(Passing::Take, l::ERASED)],
                    Some(l::ERASED),
                    &[ArgumentValue::Value(copied)],
                    (next, &[]),
                    failed,
                )
                .unwrap();
            t.builder.ret(next, Some(returned));
        }
        Route::Failure => {
            // A second failing address uses this callback only in its failure
            // cleanup. Copying it there cannot hide behind a normal-edge proof.
            let next = t.builder.block(BlockKind::Body, &[]);
            let failed = t.releasing(&[copied]);
            t.call(ENTRY, &[], None, &[], (next, &[]), failed);
            t.builder.ret(next, Some(copied));
        }
        Route::Erased => {
            let weakened = t.owned(l::ERASED_ONCE);
            t.builder.push(
                ENTRY,
                Inst::Weaken {
                    result: weakened,
                    value: copied,
                },
            );
            let abi = t.abi(&[], Some(l::ERASED_ONCE));
            t.table.fn_abis[t.builder.unit.header.abi.0 as usize].result =
                t.table.fn_abis[abi.0 as usize].result.clone();
            t.builder.ret(ENTRY, Some(weakened));
        }
    }
    check(t)
}

#[test]
fn raw_borrow_owning_copy_return_keeps_its_meaning() {
    assert!(retain(false, Route::Direct).is_empty());
}
#[test]
fn scoped_copy_return_is_independently_refused() {
    let errors = retain(true, Route::Direct);
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
#[test]
fn local_join_erased_failure_and_may_retain_twins() {
    for route in [
        Route::Local,
        Route::Join,
        Route::Erased,
        Route::Failure,
        Route::MayRetain,
    ] {
        let ordinary = retain(false, route);
        assert!(ordinary.is_empty(), "{route:?} ordinary: {ordinary:?}");
        let scoped = retain(true, route);
        assert!(
            scoped
                .iter()
                .any(|k| matches!(k, K::ScopedRetention { .. })),
            "{route:?} scoped: {scoped:?}"
        );
    }
}

#[test]
fn contract_serialization_and_stripping_changes_identity() {
    let mut t = Lir::<Value>::new(&[(Passing::Borrow, l::ERASED)], None);
    let old = t.table.fn_abis[t.builder.unit.header.abi.0 as usize].clone();
    contract(
        &mut t,
        vec![ParameterPath {
            parameter: 0,
            fields: vec![],
        }],
    );
    let scoped = t.table.fn_abis[t.builder.unit.header.abi.0 as usize].clone();
    assert_ne!(
        hirsel_lir::abi::abi_digest(&old),
        hirsel_lir::abi::abi_digest(&scoped)
    );
    let text = hirsel_codec::to_text(&scoped);
    let copy: hirsel_lir::FnAbi = hirsel_codec::from_text(&text).unwrap();
    assert_eq!(copy, scoped);
    t.table.fn_abis[t.builder.unit.header.abi.0 as usize]
        .nonretaining
        .clear();
    assert!(
        hirsel_lir::layout_validate::validate_table(
            &t.table,
            &hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu()
        )
        .is_err()
    );
}
fn carrier(t: &mut Lir<Ready>, borrowed: bool) -> hirsel_lir::LayoutId {
    use hirsel_lir::layout_types::*;
    let mut r = t.table.layouts[l::ERASED.0 as usize].clone();
    r.kind = LayoutKind::Aggregate;
    r.fields = vec![FieldLayout {
        ordinal: 0,
        offset: 0,
        layout: l::ERASED,
        unaligned: false,
    }];
    r.scan = t.table.layouts[l::ERASED.0 as usize].scan.clone();
    r.drop = t.table.layouts[l::ERASED.0 as usize].drop.clone();
    r.copy = CopyRecipe::Fields;
    if borrowed {
        r.scan = hirsel_lir::layout::borrowed_scan(&r, 0, &|id| &t.table.layouts[id.0 as usize])
            .unwrap();
        r.drop.clear();
        r.copy = CopyRecipe::Bits;
    }
    let id = hirsel_lir::LayoutId(t.table.layouts.len() as u32);
    t.table.layouts.push(r);
    id
}

#[derive(Clone, Copy, Debug)]
enum CarrierSink {
    Project,
    Heap,
    Mut,
    OwningCapture,
}
fn carrier_retainer(scoped: bool, sink: CarrierSink) -> Vec<K> {
    let mut t = Lir::<Ready>::new(&[(Passing::Borrow, l::ERASED)], None);
    let container = carrier(&mut t, false);
    let second = if matches!(sink, CarrierSink::Mut) {
        let abi = &mut t.table.fn_abis[t.builder.unit.header.abi.0 as usize];
        // Add a caller-owned mut aggregate in exactly its address convention.
        let mut p = abi.parameters[0].clone();
        p.source = 1;
        p.convention = Passing::Mut;
        p.layout = container;
        p.passing = hirsel_lir::abi_types::ParameterPassing::ExclusiveAddress;
        p.slots.truncate(1);
        abi.parameters.push(p);
        let s = t.slot(container, SlotKind::Parameter(1), ROOT);
        let v = t.version_of(s);
        Some((s, v))
    } else {
        None
    };
    if scoped {
        contract(
            &mut t,
            vec![ParameterPath {
                parameter: 0,
                fields: vec![],
            }],
        );
    }
    let s = t.slot(l::ERASED, SlotKind::Parameter(0), ROOT);
    let p = t.version_of(s);
    t.param(p);
    if let Some((_, v)) = second {
        t.param(v);
    }
    let a = t.at(ENTRY, s, &[], &[]);
    let copied = t.owned(l::ERASED);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Copy {
            result: copied,
            address: a,
            reaching: p,
            failure,
        },
    );
    match sink {
        CarrierSink::Project => {
            let c = t.construct_site(ConstructorKind::Tuple, container, &[(0, copied)], t.heap());
            let v = t.owned(container);
            let failure = t.fails();
            t.builder.push(
                ENTRY,
                Inst::Construct {
                    result: v,
                    construct: c,
                    failure,
                },
            );
            let local = t.slot(container, SlotKind::Local, ROOT);
            t.builder.push(ENTRY, Inst::StorageLive { slot: local });
            let a = t.at(ENTRY, local, &[], &[Undo::Release(v)]);
            let version = t.store(ENTRY, a, v, local);
            // Ready cleanup consumes the whole carrier and releases it; the
            // pre-Ready semantic Drop is not part of this stage's vocabulary.
            let cleanup = t.builder.block(BlockKind::FailureCleanup, &[]);
            let held = t.read(cleanup, a, version, ReadMode::Take, None);
            t.release(cleanup, held, &[]);
            t.builder.term(cleanup, Terminator::Propagate);
            let a = t.address(
                AddressBase::Slot(local),
                &[ProjectionStep::Field(0)],
                l::ERASED,
            );
            let failure = t.failure(cleanup);
            t.builder.push(
                ENTRY,
                Inst::Address {
                    address: a,
                    failure,
                },
            );
            let result = t.read(ENTRY, a, version, ReadMode::Take, None);
            t.builder.push(
                ENTRY,
                Inst::StorageDead {
                    slot: local,
                    if_live: false,
                },
            );
            let mut r = t.table.fn_abis.iter().find_map(|a| a.result.clone());
            // The fixture has no callable result yet: take one from a standard ABI.
            if r.is_none() {
                let id = t.abi(&[], Some(l::ERASED));
                r = t.table.fn_abis[id.0 as usize].result.clone();
            }
            t.table.fn_abis[t.builder.unit.header.abi.0 as usize].result = r;
            t.builder.ret(ENTRY, Some(result));
        }
        CarrierSink::Heap => {
            let mut block = t.table.layouts[l::BLK.0 as usize].clone();
            block.kind = hirsel_lir::layout_types::LayoutKind::Box { payload: container };
            let layout = hirsel_lir::LayoutId(t.table.layouts.len() as u32);
            t.table.layouts.push(block);
            let c = t.construct_site(ConstructorKind::Tuple, container, &[(0, copied)], t.heap());
            let v = t.value_as(ValueRepr::Block, layout, Holding::Owned);
            let failure = t.fails();
            t.builder.push(
                ENTRY,
                Inst::Construct {
                    result: v,
                    construct: c,
                    failure,
                },
            );
            let anchor = t.instruction_anchor();
            let failure = t.fails();
            t.builder.push(
                ENTRY,
                Inst::Release {
                    value: v,
                    layout: container,
                    live: MaskRef::All,
                    anchor,
                    failure,
                },
            );
            t.builder.ret(ENTRY, None);
        }
        CarrierSink::Mut => {
            let (s, _) = second.unwrap();
            let a = t.at(
                ENTRY,
                s,
                &[ProjectionStep::Field(0)],
                &[Undo::Release(copied)],
            );
            let displaced = t.slot(l::ERASED, SlotKind::Displaced, ROOT);
            t.builder.push(ENTRY, Inst::StorageLive { slot: displaced });
            let version = t.version_of(s);
            let displaced_version = t.version_of(displaced);
            // Prepare whole displaced storage while it is still empty, then
            // read its replaced owner without an intervening failing action.
            let displaced_address = t.at(ENTRY, displaced, &[], &[Undo::Release(copied)]);
            let failure = t.fails();
            t.builder.push(
                ENTRY,
                Inst::Replace {
                    address: a,
                    value: copied,
                    displaced,
                    version,
                    displaced_version,
                    if_live: false,
                    failure,
                },
            );
            let old = t.read(
                ENTRY,
                displaced_address,
                displaced_version,
                ReadMode::Take,
                None,
            );
            t.builder.push(
                ENTRY,
                Inst::StorageDead {
                    slot: displaced,
                    if_live: false,
                },
            );
            let anchor = t.instruction_anchor();
            let failure = t.fails();
            t.builder.push(
                ENTRY,
                Inst::Release {
                    value: old,
                    layout: l::ERASED,
                    live: MaskRef::All,
                    anchor,
                    failure,
                },
            );
            t.builder.ret(ENTRY, None);
        }
        CarrierSink::OwningCapture => {
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
            t.builder.unit.tables.closures.push(ClosureSite {
                receipts: None,
                scoped: false,
                code: Some(ClosureCode { symbol, abi }),
                environment: container,
                captures,
                destination: DestinationClass::Heap,
                placement: t.heap(),
                anchor: SourceAnchorId(0),
            });
            let v = t.owned(l::ERASED);
            let failure = t.fails();
            t.builder.push(
                ENTRY,
                Inst::Closure {
                    result: v,
                    closure,
                    failure,
                },
            );
            let anchor = t.instruction_anchor();
            let failure = t.fails();
            t.builder.push(
                ENTRY,
                Inst::Release {
                    value: v,
                    layout: l::ERASED,
                    live: MaskRef::All,
                    anchor,
                    failure,
                },
            );
            t.builder.ret(ENTRY, None);
        }
    }
    check(t)
}

#[test]
fn field_projection_heap_mut_and_nested_owning_capture_keep_provenance() {
    for sink in [
        CarrierSink::Project,
        CarrierSink::Heap,
        CarrierSink::Mut,
        CarrierSink::OwningCapture,
    ] {
        let ordinary = carrier_retainer(false, sink);
        assert!(ordinary.is_empty(), "{sink:?} owning: {ordinary:?}");
        let scoped = carrier_retainer(true, sink);
        let operation = match sink {
            CarrierSink::Project => "return or error payload",
            CarrierSink::Heap => "owning carrier construction",
            CarrierSink::Mut => "persistent or mutable hand-back store",
            CarrierSink::OwningCapture => "owning capture",
        };
        assert!(
            scoped
                .iter()
                .any(|k| matches!(k,K::ScopedRetention {operation:op,..} if *op==operation)),
            "{sink:?} scoped: {scoped:?}"
        );
    }
}

#[test]
fn failure_only_copy_retains_its_incoming_scoped_origin() {
    for scoped in [false, true] {
        let mut t = Lir::<Ready>::new(&[(Passing::Borrow, l::ERASED)], None);
        if scoped {
            contract(
                &mut t,
                vec![ParameterPath {
                    parameter: 0,
                    fields: vec![],
                }],
            );
        }
        let s = t.slot(l::ERASED, SlotKind::Parameter(0), ROOT);
        let p = t.version_of(s);
        t.param(p);
        let normal = t.builder.block(BlockKind::Body, &[]);
        let failed = t.builder.block(BlockKind::FailureCleanup, &[]);
        t.call(ENTRY, &[], None, &[], (normal, &[]), failed);
        t.builder.ret(normal, None);
        let address = t.at(failed, s, &[], &[]);
        let result = t.owned(l::ERASED);
        let failure = t.fails();
        t.builder.push(
            failed,
            Inst::Copy {
                result,
                address,
                reaching: p,
                failure,
            },
        );
        t.release(failed, result, &[]);
        t.builder.term(failed, Terminator::Propagate);
        let errors = check(t);
        if scoped {
            assert!(
                errors
                    .iter()
                    .any(|k| matches!(k, K::ScopedRetention { .. })),
                "{errors:?}"
            );
        } else {
            assert!(errors.is_empty(), "{errors:?}");
        }
    }
}
#[test]
fn two_arm_union_keeps_each_scoped_origin() {
    for scoped in [false, true] {
        let mut t = Lir::<Ready>::new(
            &[(Passing::Borrow, l::ERASED), (Passing::Borrow, l::ERASED)],
            Some(l::ERASED),
        );
        if scoped {
            contract(
                &mut t,
                vec![
                    ParameterPath {
                        parameter: 0,
                        fields: vec![],
                    },
                    ParameterPath {
                        parameter: 1,
                        fields: vec![],
                    },
                ],
            );
        }
        let mut roots = Vec::new();
        let mut addresses = Vec::new();
        for ordinal in 0..2 {
            let s = t.slot(l::ERASED, SlotKind::Parameter(ordinal), ROOT);
            let v = t.version_of(s);
            t.param(v);
            roots.push(v);
            addresses.push(t.at(ENTRY, s, &[], &[]));
        }
        let joined = t.owned(l::ERASED);
        let join = t.builder.block(BlockKind::Body, &[joined]);
        let yes = t.builder.block(BlockKind::Body, &[]);
        let no = t.builder.block(BlockKind::Body, &[]);
        t.branch(ENTRY, (yes, &[]), (no, &[]));
        for (i, block) in [yes, no].into_iter().enumerate() {
            let result = t.owned(l::ERASED);
            let failure = t.fails();
            t.builder.push(
                block,
                Inst::Copy {
                    result,
                    address: addresses[i],
                    reaching: roots[i],
                    failure,
                },
            );
            t.builder.jump(block, join, &[result]);
        }
        t.builder.ret(join, Some(joined));
        let errors = check(t);
        if scoped {
            for origin in roots {
                assert!(errors.iter().any(|k| matches!(k, K::ScopedRetention {origin: o, operation: "return or error payload", ..} if *o == origin)), "{errors:?}");
            }
        } else {
            assert!(errors.is_empty(), "{errors:?}");
        }
    }
}

#[test]
fn linked_body_claims_cover_calls_and_closure_entries() {
    let mut t = Lir::<Value>::new(&[(Passing::Borrow, l::ERASED)], None);
    contract(
        &mut t,
        vec![ParameterPath {
            parameter: 0,
            fields: vec![],
        }],
    );
    let s = t.slot(l::ERASED, SlotKind::Parameter(0), ROOT);
    let p = t.version_of(s);
    t.param(p);
    t.builder.ret(ENTRY, None);
    let (body, mut table, _) = t.finish();
    let claimed = body.header.abi;
    let mut empty = table.fn_abis[claimed.0 as usize].clone();
    empty.nonretaining.clear();
    empty.scoped_contract = hirsel_lir::abi::scoped_contract_digest(&[], &[]);
    let stripped = hirsel_lir::FnAbiId(table.fn_abis.len() as u32);
    table.fn_abis.push(empty);
    // Link checking reads entry declarations independently of the local C
    // check. The body itself above has no retaining instruction.
    for closure in [false, true] {
        let mut caller = body.clone();
        caller.header.definition = path("caller");
        caller.header.instance = hash("caller");
        let symbol = SymbolId(caller.tables.symbols.len() as u32);
        caller.tables.symbols.push(Symbol {
            kind: SymbolKind::Function,
            definition: body.header.definition.clone(),
            instance: Some(body.header.instance),
        });
        if closure {
            caller.tables.closures.push(ClosureSite {
                receipts: None,
                scoped: true,
                code: Some(ClosureCode {
                    symbol,
                    abi: claimed,
                }),
                environment: l::UNIT,
                captures: PoolRange::EMPTY,
                destination: DestinationClass::Heap,
                placement: None,
                anchor: SourceAnchorId(0),
            });
        } else {
            caller.tables.calls.push(CallSite {
                receipts: None,
                pretransfer_failure: None,
                forward_caller: true,
                operation_arguments: vec![],
                callee: Callee::Direct(symbol),
                abi: claimed,
                arguments: PoolRange::EMPTY,
                result: None,
                destination: None,
                declared: hirsel_lir::effect::Effects::ALL,
                anchor: SourceAnchorId(0),
            });
        }
        assert!(
            hirsel_lir::validate::check_scoped_calls(&[caller.clone(), body.clone()], &table)
                .is_ok()
        );
        assert!(hirsel_lir::validate::check_scoped_links(&[caller.clone()], &table).is_ok());
        assert!(hirsel_lir::validate::check_scoped_calls(&[caller.clone()], &table).is_err());
        let mut forged = body.clone();
        forged.header.abi = stripped;
        assert!(
            hirsel_lir::validate::check_scoped_calls(&[caller.clone(), forged], &table).is_err()
        );
        if closure {
            caller
                .tables
                .closures
                .last_mut()
                .unwrap()
                .code
                .as_mut()
                .unwrap()
                .abi = stripped;
        } else {
            caller.tables.calls.last_mut().unwrap().abi = stripped;
        }
        assert!(hirsel_lir::validate::check_scoped_calls(&[caller, body.clone()], &table).is_err());
    }
}

#[test]
fn mut_call_writeback_cannot_launder_the_incoming_contract() {
    for scoped in [false, true] {
        let mut t = Lir::<Ready>::new(&[(Passing::Mut, l::ERASED)], Some(l::ERASED));
        if scoped {
            contract(
                &mut t,
                vec![ParameterPath {
                    parameter: 0,
                    fields: vec![],
                }],
            );
        }
        let slot = t.slot(l::ERASED, SlotKind::Parameter(0), ROOT);
        let p = t.version_of(slot);
        t.param(p);
        let address = t.at(ENTRY, slot, &[], &[]);
        let version = t.version_of(slot);
        let abi = t.abi(&[(Passing::Mut, l::ERASED)], None);
        let declaration = &mut t.table.fn_abis[abi.0 as usize];
        declaration.nonretaining = vec![ParameterPath {
            parameter: 0,
            fields: vec![],
        }];
        declaration.scoped_contract =
            hirsel_lir::abi::scoped_contract_digest(&declaration.nonretaining, &[]);
        let next = t.builder.block(BlockKind::Body, &[]);
        let failed = t.builder.propagate();
        let symbol = t.symbol(SymbolKind::Function);
        t.call_to(
            ENTRY,
            Callee::Direct(symbol),
            abi,
            &[(Passing::Mut, l::ERASED)],
            None,
            &[ArgumentValue::Address { address, version }],
            (next, &[]),
            failed,
        );
        let address = t.at(next, slot, &[], &[]);
        let result = t.owned(l::ERASED);
        let failure = t.fails();
        t.builder.push(
            next,
            Inst::Copy {
                result,
                address,
                reaching: version,
                failure,
            },
        );
        t.builder.ret(next, Some(result));
        let errors = check(t);
        if scoped {
            assert!(errors.iter().any(|k|matches!(k,K::ScopedRetention{origin,operation:"return or error payload",..} if *origin==p)),"{errors:?}");
        } else {
            assert!(errors.is_empty(), "{errors:?}");
        }
    }
}
#[test]
fn scoped_borrow_forwarding_requires_the_parameter_contract() {
    for scoped in [false, true] {
        for promised in [false, true] {
            let mut t = Lir::<Ready>::new(&[(Passing::Borrow, l::ERASED)], None);
            if scoped {
                contract(
                    &mut t,
                    vec![ParameterPath {
                        parameter: 0,
                        fields: vec![],
                    }],
                );
            }
            let slot = t.slot(l::ERASED, SlotKind::Parameter(0), ROOT);
            let p = t.version_of(slot);
            t.param(p);
            let address = t.at(ENTRY, slot, &[], &[]);
            let loan = t.loan(address, false);
            t.begin(ENTRY, loan);
            let view = t.read(ENTRY, address, p, ReadMode::Borrow, Some(loan));
            let abi = t.abi(&[(Passing::Borrow, l::ERASED)], None);
            if promised {
                let declaration = &mut t.table.fn_abis[abi.0 as usize];
                declaration.nonretaining = vec![ParameterPath {
                    parameter: 0,
                    fields: vec![],
                }];
                declaration.scoped_contract =
                    hirsel_lir::abi::scoped_contract_digest(&declaration.nonretaining, &[]);
            }
            let next = t.builder.block(BlockKind::Body, &[]);
            let failed = t.cleanup(&[Undo::End(loan)]);
            let symbol = t.symbol(SymbolKind::Function);
            t.call_to(
                ENTRY,
                Callee::Direct(symbol),
                abi,
                &[(Passing::Borrow, l::ERASED)],
                None,
                &[ArgumentValue::Value(view)],
                (next, &[]),
                failed,
            );
            t.end(next, loan);
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
            let context = hirsel_lir::validate::check_scoped_transport(&unit, &inputs);
            let errors: Vec<_> = context.into_iter().map(|v| v.kind).collect();
            if scoped && !promised {
                assert!(
                    errors.iter().any(|k| matches!(
                        k,
                        K::ScopedRetention {
                            operation: "may-retain call argument",
                            ..
                        }
                    )),
                    "{errors:?}"
                );
            } else {
                assert!(errors.is_empty(), "{errors:?}");
            }
        }
    }
}

#[test]
fn unknown_indirect_entry_does_not_qualify_a_scoped_contract() {
    for promised in [false, true] {
        let mut t = Lir::<Ready>::new(&[(Passing::Borrow, l::ERASED)], None);
        if promised {
            contract(
                &mut t,
                vec![ParameterPath {
                    parameter: 0,
                    fields: vec![],
                }],
            );
        }
        let slot = t.slot(l::ERASED, SlotKind::Parameter(0), ROOT);
        let p = t.version_of(slot);
        t.param(p);
        let address = t.at(ENTRY, slot, &[], &[]);
        let loan = t.loan(address, false);
        t.begin(ENTRY, loan);
        let view = t.read(ENTRY, address, p, ReadMode::Borrow, Some(loan));
        let abi = t.abi_shaped(
            hirsel_lir::abi_types::CallableShape::Closure,
            &[(Passing::Borrow, l::ERASED)],
            None,
        );
        if promised {
            let declaration = &mut t.table.fn_abis[abi.0 as usize];
            declaration.nonretaining = vec![ParameterPath {
                parameter: 0,
                fields: vec![],
            }];
            declaration.scoped_contract =
                hirsel_lir::abi::scoped_contract_digest(&declaration.nonretaining, &[]);
        }
        let next = t.builder.block(BlockKind::Body, &[]);
        let failed = t.cleanup(&[Undo::End(loan)]);
        t.call_to(
            ENTRY,
            Callee::Indirect(view),
            abi,
            &[(Passing::Borrow, l::ERASED)],
            None,
            &[ArgumentValue::Value(view)],
            (next, &[]),
            failed,
        );
        t.end(next, loan);
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
        assert!(hirsel_lir::validate::check(&unit, &inputs).is_empty());
        let qualified = hirsel_lir::validate::qualify_scoped_transport(&[unit], &inputs);
        assert_eq!(qualified.is_err(), promised, "{qualified:?}");
        if promised {
            assert!(
                qualified
                    .unwrap_err()
                    .contains("indirect scoped entry contract is unavailable")
            );
        }
    }
}

#[test]
fn formal_field_selector_is_reconstructed_before_a_copy() {
    for scoped in [false, true] {
        let mut t = Lir::<Ready>::new(&[], Some(l::ERASED));
        let carrier = carrier(&mut t, false);
        let abi = t.abi(&[(Passing::Borrow, carrier)], Some(l::ERASED));
        t.builder.unit.header.abi = abi;
        if scoped {
            contract(
                &mut t,
                vec![ParameterPath {
                    parameter: 0,
                    fields: vec![0],
                }],
            );
        }
        let slot = t.slot(carrier, SlotKind::Parameter(0), ROOT);
        let p = t.version_of(slot);
        t.param(p);
        let address = t.at(ENTRY, slot, &[ProjectionStep::Field(0)], &[]);
        let result = t.owned(l::ERASED);
        let failure = t.fails();
        t.builder.push(
            ENTRY,
            Inst::Copy {
                result,
                address,
                reaching: p,
                failure,
            },
        );
        t.builder.ret(ENTRY, Some(result));
        let errors = check(t);
        if scoped {
            assert!(errors.iter().any(|k|matches!(k,K::ScopedRetention{origin,operation:"return or error payload",..}if *origin==p)),"{errors:?}");
        } else {
            assert!(errors.is_empty(), "{errors:?}");
        }
    }
}
#[test]
fn a_capture_selector_cannot_name_an_absent_capture() {
    for forged in [false, true] {
        let mut t = Lir::<Ready>::new(&[], None);
        if forged {
            let abi = &mut t.table.fn_abis[t.builder.unit.header.abi.0 as usize];
            abi.nonretaining_captures = vec![ParameterPath {
                parameter: 99,
                fields: vec![],
            }];
            abi.scoped_contract =
                hirsel_lir::abi::scoped_contract_digest(&[], &abi.nonretaining_captures);
        }
        t.builder.ret(ENTRY, None);
        let errors = check(t);
        if forged {
            assert!(
                errors
                    .iter()
                    .any(|k| matches!(k, K::Type("scoped contract ordinal"))),
                "{errors:?}"
            );
        } else {
            assert!(errors.is_empty(), "{errors:?}");
        }
    }
}

#[test]
fn shared_174_forwarding_preserves_borrowed_result_origins() {
    for layout in [l::SHARED, l::BLK] {
        for alias in [false, true] {
            let mut t = Lir::<Ready>::new(&[(Passing::Borrow, layout)], alias.then_some(layout));
            contract(
                &mut t,
                vec![ParameterPath {
                    parameter: 0,
                    fields: vec![],
                }],
            );
            let slot = t.slot(layout, SlotKind::Parameter(0), ROOT);
            let p = t.version_of(slot);
            t.param(p);
            let address = t.at(ENTRY, slot, &[], &[]);
            let loan = t.loan(address, false);
            t.begin(ENTRY, loan);
            let view = t.read(ENTRY, address, p, ReadMode::Borrow, Some(loan));
            let abi = t.abi(&[(Passing::Borrow, layout)], alias.then_some(layout));
            if alias {
                t.table.fn_abis[abi.0 as usize]
                    .result
                    .as_mut()
                    .unwrap()
                    .borrowed_parameters = vec![0];
            }
            let next = t.builder.block(BlockKind::Body, &[]);
            let failed = t.cleanup(&[Undo::End(loan)]);
            let symbol = t.symbol(SymbolKind::Function);
            let result = t.call_to(
                ENTRY,
                Callee::Direct(symbol),
                abi,
                &[(Passing::Borrow, layout)],
                alias.then_some(layout),
                &[ArgumentValue::Value(view)],
                (next, &[]),
                failed,
            );
            t.end(next, loan);
            t.builder.ret(next, result);
            let errors = check(t);
            let forwarding = errors.iter().any(|k| {
                matches!(
                    k,
                    K::ScopedRetention {
                        operation: "may-retain call argument",
                        ..
                    }
                )
            });
            assert_eq!(
                forwarding,
                layout == l::BLK,
                "{layout:?} alias={alias}: {errors:?}"
            );
            let escaped = errors.iter().any(|k| {
                matches!(
                    k,
                    K::ScopedRetention {
                        operation: "return or error payload",
                        ..
                    }
                )
            });
            assert_eq!(escaped, alias, "{layout:?} alias={alias}: {errors:?}");
            if layout == l::SHARED && !alias {
                assert!(errors.is_empty(), "{errors:?}");
            }
        }
    }
}

#[test]
fn shared_174_copy_acquires_a_root_but_borrowed_reads_keep_the_view() {
    for layout in [l::SHARED, l::BLK] {
        for copied in [false, true] {
            let mut t = Lir::<Ready>::new(&[(Passing::Borrow, layout)], Some(layout));
            contract(
                &mut t,
                vec![ParameterPath {
                    parameter: 0,
                    fields: vec![],
                }],
            );
            let slot = t.slot(layout, SlotKind::Parameter(0), ROOT);
            let p = t.version_of(slot);
            t.param(p);
            let address = t.at(ENTRY, slot, &[], &[]);
            let result = if copied {
                let result = t.owned(layout);
                let failure = t.fails();
                t.builder.push(
                    ENTRY,
                    Inst::Copy {
                        result,
                        address,
                        reaching: p,
                        failure,
                    },
                );
                result
            } else {
                let loan = t.loan(address, false);
                t.begin(ENTRY, loan);
                t.read(ENTRY, address, p, ReadMode::Borrow, Some(loan))
            };
            t.builder.ret(ENTRY, Some(result));
            let errors = check(t);
            let escaped = errors.iter().any(|k| {
                matches!(
                    k,
                    K::ScopedRetention {
                        operation: "return or error payload",
                        ..
                    }
                )
            });
            assert_eq!(
                escaped,
                !(copied && layout == l::SHARED),
                "{layout:?} copied={copied}: {errors:?}"
            );
            if copied && layout == l::SHARED {
                assert!(errors.is_empty(), "{errors:?}");
            }
        }
    }
}

#[test]
fn invocation_local_data_copies_are_bounded_and_copied_results_cannot_escape() {
    for escape in [false, true] {
        let mut t = Lir::<Ready>::new(&[(Passing::Borrow, l::BLK)], escape.then_some(l::BLK));
        contract(
            &mut t,
            vec![ParameterPath {
                parameter: 0,
                fields: vec![],
            }],
        );
        let slot = t.slot(l::BLK, SlotKind::Parameter(0), ROOT);
        let parameter = t.version_of(slot);
        t.param(parameter);
        let address = t.at(ENTRY, slot, &[], &[]);
        let copied = t.owned(l::BLK);
        let failure = t.fails();
        t.builder.push(
            ENTRY,
            Inst::Copy {
                result: copied,
                address,
                reaching: parameter,
                failure,
            },
        );
        t.builder.push(ENTRY, Inst::Share { value: copied });
        if escape {
            t.builder.ret(ENTRY, Some(copied));
        } else {
            t.release(ENTRY, copied, &[]);
            t.builder.ret(ENTRY, None);
        }
        let errors = check(t);
        if escape {
            assert!(
                errors.iter().any(|error| matches!(
                    error,
                    K::ScopedRetention {
                        operation: "return or error payload",
                        ..
                    }
                )),
                "{errors:?}"
            );
        } else {
            assert!(errors.is_empty(), "{errors:?}");
        }
    }
}
