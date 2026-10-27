//! Strict C binds borrowed data companions to original ports and live slot versions.
use crate::support::{self, ENTRY, ROOT};
use hirsel_core_contract::{Manifest, OperationId, Passing};
use hirsel_layout::{
    layout::{LayoutBuilder, Owners},
    operation::OperationRegistry,
};
use hirsel_lir::{
    ir::*,
    layout_types::*,
    operation::*,
    sealed_payload_source::*,
    stage::Rc,
    validate::{self, Inputs},
};
use hirsel_syntax::{ContentHash, DefPath};
use hirsel_types::{Primitive as P, Type, TypeKind};

struct Declarations {
    ordinary: OperationRegistry,
    sources: Vec<ReceiptSource>,
}
impl OperationDeclarations for Declarations {
    fn pretransfer_adapters(&self) -> &[hirsel_lir::pretransfer::AdapterReference] {
        &self.ordinary.pretransfer_adapters
    }
    fn receipt_source(&self, definition: &DefPath, instance: ContentHash) -> Option<ReceiptSource> {
        self.sources
            .iter()
            .find(|s| &s.original.definition == definition && s.instance == instance)
            .cloned()
    }
    fn derive(
        &self,
        op: &OperationId,
        args: &[OperationArgument],
        target: &TargetSpec,
    ) -> Result<(hirsel_lir::FnAbi, LayoutTable), hirsel_lir::layout::LayoutError> {
        self.ordinary.derive(op, args, target)
    }
    fn seal_descriptor_for_source(
        &self,
        _: &DefPath,
        _: ContentHash,
        root: &NativeType,
        table: &LayoutTable,
        target: &TargetSpec,
    ) -> Result<hirsel_lir::sealed::SealDescriptor, hirsel_lir::layout::LayoutError> {
        self.ordinary.seal_descriptor(root, table, target)
    }
}

fn fixture(call: bool) -> (LirUnit<Rc>, LayoutTable, Manifest, Declarations) {
    fixture_mode(call, false)
}

fn fixture_mode(call: bool, take: bool) -> (LirUnit<Rc>, LayoutTable, Manifest, Declarations) {
    let types = support::session();
    let scalar = types.intern(Type::with_kind(TypeKind::Primitive(P::U64)));
    let boxed = types.intern(Type::with_kind(TypeKind::Builtin {
        kind: hirsel_types::BuiltinType::List,
        arguments: vec![hirsel_types::GenericArgument::Type(scalar)].into(),
    }));
    let root = NativeType::Builtin(
        hirsel_lir::operation::BuiltinType::List,
        vec![OperationArgument::Type(NativeType::Primitive(
            Primitive::U64,
        ))],
    );
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let owners = Owners::default();
    let mut layouts = LayoutBuilder::new(&target, &owners);
    let ordinary = layouts.layout(&types, boxed).unwrap();
    let meta = layouts.payload_receipt(&root);
    let mut t = support::Lir::<Rc>::new(&[], None);
    let offset = t.table.layouts.len() as u32;
    for mut row in layouts.finish().layouts {
        row.for_each_reference_mut(&mut |id| id.0 += offset);
        t.table.layouts.push(row);
    }
    let ordinary = LayoutId(ordinary.0 + offset);
    let meta = LayoutId(meta.0 + offset);
    let passing = if take { Passing::Take } else { Passing::Borrow };
    let params = [(passing, ordinary), (passing, ordinary)];
    t.builder.unit.header.abi = t.abi(&params, None);
    for _ in 0..2 {
        let value = t.value_as(
            ValueRepr::Block,
            ordinary,
            if take { Holding::Owned } else { Holding::Plain },
        );
        t.param(value);
    }
    let profile = ClosedStorageProfile {
        schema: ClosedStorageProfile::SCHEMA,
        root: ClosedStorageType {
            ty: root,
            source_type: support::hash("List[u64]"),
        },
        functions: vec![],
    };
    let ty = ReceiptType {
        source_type: profile.root.source_type,
        profile: profile.content().unwrap(),
    };
    let contract = ReceiptContract::new(
        (0..2)
            .map(|source| ReceiptParameter {
                source,
                ty: ty.clone(),
                mode: if take {
                    ReceiptMode::Take
                } else {
                    ReceiptMode::Borrow
                },
            })
            .collect(),
        None,
        vec![],
        vec![],
    )
    .unwrap();
    let mut bindings = vec![];
    for source in 0..2 {
        let ty = t.ty(ValueRepr::PayloadReceipt, meta);
        let slot = SlotId(t.builder.unit.slots.len() as u32);
        t.builder.unit.slots.push(SlotInfo {
            ty,
            scope: ROOT,
            kind: SlotKind::ReceiptParameter(source),
        });
        let version = t.value_as(ValueRepr::PayloadReceipt, meta, Holding::Plain);
        bindings.push(ReceiptEntrySlot {
            source,
            slot,
            version,
        });
    }
    t.builder.unit.tables.receipt_entry = Some(ReceiptEntry {
        contract: contract.content().unwrap(),
        parameters: bindings.clone(),
        captures: vec![],
        result: None,
        environment: None,
    });
    let own = t.builder.unit.header.abi;
    t.table.fn_abis[own.0 as usize].key.receipt_contract = Some(contract.content().unwrap());
    t.table.fn_abis[own.0 as usize].receipts = Some(contract.clone());
    let length = t.value_as(
        ValueRepr::Scalar(ScalarKind::U64),
        support::l::U64,
        Holding::Plain,
    );
    t.builder.push(
        ENTRY,
        Inst::SeqLen {
            result: length,
            value: t.builder.unit.parameters[0],
        },
    );
    let mut sources = vec![];
    let own_source = ReceiptSource {
        original: hirsel_lir::invocations::ReferenceBody {
            definition: t.builder.unit.header.definition.clone(),
            instance: support::hash("original"),
            body: 0,
        },
        instance: t.builder.unit.header.instance,
        source: support::hash("V observation"),
        implementation_revision: t.builder.unit.header.implementation_revision,
        contract: contract.clone(),
        profiles: (0..2)
            .map(|source| ReceiptProfile {
                port: ReceiptPort::Parameter(source),
                profile: profile.clone(),
            })
            .collect(),
    };
    sources.push(own_source.clone());
    if call {
        let mut receipts = vec![];
        let mut loans = vec![];
        let mut addresses = vec![];
        for binding in &bindings {
            let ty = t.builder.unit.slots[binding.slot.index()].ty;
            let address = AddressId(t.builder.unit.addresses.len() as u32);
            t.builder.unit.addresses.push(AddressInfo {
                source_place: None,
                unchecked: vec![],
                base: AddressBase::Slot(binding.slot),
                projection: PoolRange::EMPTY,
                ty,
            });
            if take {
                // A whole metadata-slot address has no fallible projection.
                let fault = t.builder.block(BlockKind::FailureCleanup, &[]);
                t.builder.term(
                    fault,
                    Terminator::InvariantFault {
                        reason: "whole metadata-slot address failed".into(),
                    },
                );
                let failure = t.failure(fault);
                t.builder.push(ENTRY, Inst::Address { address, failure });
            } else {
                t.prepare(ENTRY, address, &[]);
            }
            addresses.push(address);
        }
        for (binding, address) in bindings.iter().zip(addresses) {
            let holding = if take {
                Holding::Owned
            } else {
                let loan = t.loan(address, false);
                t.begin(ENTRY, loan);
                loans.push(loan);
                Holding::Borrowed(loan)
            };
            let value = t.value_as(ValueRepr::PayloadReceipt, meta, holding);
            t.builder.push(
                ENTRY,
                Inst::Read {
                    result: value,
                    address,
                    reaching: binding.version,
                    mode: if take {
                        ReadMode::Take
                    } else {
                        ReadMode::Borrow
                    },
                },
            );
            receipts.push(if take {
                ReceiptArgument::Take {
                    parameter: binding.source,
                    receipt: value,
                }
            } else {
                ReceiptArgument::Borrow {
                    parameter: binding.source,
                    receipt: value,
                }
            });
        }
        let next = t.builder.block(BlockKind::Body, &[]);
        let fail = t.builder.block(BlockKind::FailureCleanup, &[]);
        for &loan in loans.iter().rev() {
            t.end(fail, loan);
            t.end(next, loan);
        }
        t.builder.term(fail, Terminator::Propagate);
        let args = t
            .builder
            .unit
            .parameters
            .iter()
            .map(|v| ArgumentValue::Value(*v))
            .collect::<Vec<_>>();
        t.call(ENTRY, &params, None, &args, (next, &[]), fail);
        let call = &mut t.builder.unit.tables.calls[0];
        let id = call.abi;
        t.table.fn_abis[id.0 as usize].key.receipt_contract = Some(contract.content().unwrap());
        t.table.fn_abis[id.0 as usize].receipts = Some(contract.clone());
        call.receipts = Some(ReceiptCall {
            contract: contract.content().unwrap(),
            arguments: receipts,
            result: None,
        });
        let Callee::Direct(symbol) = call.callee else {
            unreachable!()
        };
        let symbol = &mut t.builder.unit.tables.symbols[symbol.index()];
        symbol.instance = Some(support::hash("selected callee"));
        sources.push(ReceiptSource {
            original: hirsel_lir::invocations::ReferenceBody {
                definition: symbol.definition.clone(),
                instance: support::hash("original callee"),
                body: 0,
            },
            instance: symbol.instance.unwrap(),
            ..own_source
        });
        if take {
            let mut pending = t.builder.unit.tables.calls[0]
                .receipts
                .as_ref()
                .unwrap()
                .inputs()
                .collect::<Vec<_>>();
            pending.reverse();
            pending.extend(t.builder.unit.parameters.iter().copied());
            let rejected = t.releasing(&pending);
            let failure = t.failure(rejected);
            t.builder.unit.tables.calls[0].pretransfer_failure = Some(failure);
            let Callee::Direct(symbol) = t.builder.unit.tables.calls[0].callee else {
                unreachable!()
            };
            t.operations
                .pretransfer_adapters
                .push(hirsel_lir::pretransfer::AdapterReference {
                    adapter: t.builder.unit.tables.symbols[symbol.index()].clone(),
                    primitive: Symbol {
                        kind: SymbolKind::Function,
                        definition: DefPath::core("map.insert"),
                        instance: Some(support::hash("independent primitive")),
                    },
                    abi: id,
                });
        }
        t.builder.ret(next, None);
    } else {
        t.builder.ret(ENTRY, None);
    }
    let declarations = Declarations {
        ordinary: t.operations.clone(),
        sources,
    };
    let (unit, table, manifest) = t.finish();
    (unit, table, manifest, declarations)
}
fn check(
    unit: &LirUnit<Rc>,
    table: &LayoutTable,
    manifest: &Manifest,
    declarations: &Declarations,
) -> Vec<validate::Violation> {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let inputs = Inputs {
        target: &target,
        layouts: table,
        manifest,
        operations: declarations,
        discharge: None,
        memo: Default::default(),
    };
    let errors = validate::check(unit, &inputs);
    assert_eq!(errors, validate::check_scoped_transport(unit, &inputs));
    errors
}
#[test]
fn borrowed_data_entry_and_nested_call_have_initialized_versioned_companions() {
    for call in [false, true] {
        let (unit, table, manifest, declarations) = fixture(call);
        let errors = check(&unit, &table, &manifest, &declarations);
        assert!(errors.is_empty(), "{errors:#?}");
    }
}
#[test]
fn borrowed_data_entry_refuses_missing_source_wrong_profile_and_extra_entry_slots() {
    let (unit, table, manifest, declarations) = fixture(false);
    assert!(check(&unit, &table, &manifest, &declarations).is_empty());
    for mutation in 0..5 {
        let mut unit = unit.clone();
        let mut table = table.clone();
        let mut sources = declarations.sources.clone();
        match mutation {
            0 => sources.clear(),
            1 => sources[0].implementation_revision = support::hash("other body"),
            2 => {
                let slot = unit.tables.receipt_entry.as_ref().unwrap().parameters[0].slot;
                let layout = unit.types[unit.slots[slot.index()].ty.index()].layout;
                let LayoutKind::PayloadReceipt { root } =
                    &mut table.layouts[layout.0 as usize].kind
                else {
                    unreachable!()
                };
                *root = NativeType::Primitive(Primitive::Bytes);
            }
            3 => unit.slots.push(unit.slots[0].clone()),
            4 => unit.tables.receipt_entry = None,
            _ => unreachable!(),
        }
        let declarations = Declarations {
            ordinary: declarations.ordinary.clone(),
            sources,
        };
        assert!(
            !check(&unit, &table, &manifest, &declarations).is_empty(),
            "mutation {mutation}"
        );
    }
}
#[test]
fn borrowed_data_call_refuses_crossed_snapshots_missing_callee_and_closed_loans() {
    let (unit, table, manifest, declarations) = fixture(true);
    assert!(check(&unit, &table, &manifest, &declarations).is_empty());
    for mutation in 0..4 {
        let mut unit = unit.clone();
        let mut sources = declarations.sources.clone();
        match mutation {
            0 => {
                let receipts = unit.tables.calls[0].receipts.as_mut().unwrap();
                let [
                    ReceiptArgument::Borrow { receipt: a, .. },
                    ReceiptArgument::Borrow { receipt: b, .. },
                ] = receipts.arguments.as_mut_slice()
                else {
                    unreachable!()
                };
                std::mem::swap(a, b);
            }
            1 => {
                sources.pop();
            }
            2 => {
                let begin = unit
                    .instructions
                    .iter_mut()
                    .find(|i| {
                        matches!(
                            i,
                            Inst::BeginLoan {
                                loan: LoanId(0),
                                ..
                            }
                        )
                    })
                    .unwrap();
                *begin = Inst::EndLoan {
                    loan: LoanId(0),
                    if_active: true,
                };
            }
            3 => {
                unit.tables.calls[0].receipts.as_mut().unwrap().arguments[0] =
                    ReceiptArgument::Borrow {
                        parameter: 0,
                        receipt: unit.tables.receipt_entry.as_ref().unwrap().parameters[0].version,
                    }
            }
            _ => unreachable!(),
        }
        let declarations = Declarations {
            ordinary: declarations.ordinary.clone(),
            sources,
        };
        assert!(
            !check(&unit, &table, &manifest, &declarations).is_empty(),
            "mutation {mutation}"
        );
    }
}

#[test]
fn borrowed_receipt_entries_refuse_take_mutable_loans_and_storage_reinitialization() {
    let (unit, table, manifest, declarations) = fixture(true);
    let errors = check(&unit, &table, &manifest, &declarations);
    assert!(errors.is_empty(), "{errors:#?}");
    for mutation in 0..3 {
        let mut unit = unit.clone();
        match mutation {
            0 => {
                let read = unit
                    .instructions
                    .iter_mut()
                    .find(|i| matches!(i, Inst::Read { .. }))
                    .unwrap();
                let Inst::Read { mode, result, .. } = read else {
                    unreachable!()
                };
                *mode = ReadMode::Take;
                unit.values[result.index()].holding = Holding::Owned;
            }
            1 => {
                let begin = unit
                    .instructions
                    .iter_mut()
                    .find(|i| matches!(i, Inst::BeginLoan { .. }))
                    .unwrap();
                let Inst::BeginLoan { mutable, loan, .. } = begin else {
                    unreachable!()
                };
                *mutable = true;
                unit.tables.loans[loan.index()].mutable = true;
            }
            2 => {
                let binding = &unit.tables.receipt_entry.as_ref().unwrap().parameters[0];
                let begin = unit
                    .instructions
                    .iter_mut()
                    .find(|i| matches!(i, Inst::BeginLoan { .. }))
                    .unwrap();
                *begin = Inst::StorageLive { slot: binding.slot };
            }
            _ => unreachable!(),
        }
        let errors = check(&unit, &table, &manifest, &declarations);
        assert!(
            errors.iter().any(|v| match (&v.kind, mutation) {
                (validate::ViolationKind::BorrowedStorage(_), 0) => true,
                (validate::ViolationKind::Sealed(text), 1) => text.contains("mutably borrowed"),
                (validate::ViolationKind::Sealed(text), 2) => text.contains("reinitialized"),
                _ => false,
            }),
            "mutation {mutation}: {errors:#?}"
        );
    }
}

#[test]
fn erasing_both_call_companions_and_abi_contract_cannot_hide_original_demand() {
    let (mut unit, mut table, manifest, declarations) = fixture(true);
    let errors = check(&unit, &table, &manifest, &declarations);
    assert!(errors.is_empty(), "{errors:#?}");
    let call = &mut unit.tables.calls[0];
    call.receipts = None;
    let abi = &mut table.fn_abis[call.abi.0 as usize];
    abi.receipts = None;
    abi.key.receipt_contract = None;
    let errors = check(&unit, &table, &manifest, &declarations);
    assert!(
        errors.iter().any(|e| matches!(&e.kind,
        validate::ViolationKind::Sealed(text) if text.contains("complete ABI contract"))),
        "{errors:#?}"
    );
    let ordinary = Declarations {
        ordinary: declarations.ordinary.clone(),
        sources: declarations.sources[..1].to_vec(),
    };
    let errors = check(&unit, &table, &manifest, &ordinary);
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn cached_call_output_replays_absent_and_present_callee_source_without_materializing_ir() {
    use hirsel_lir::stage::StageKind;
    use hirsel_lir::validate::{NativeCache, NativeEnvironment};
    use std::sync::Arc;
    let (mut unit, mut table, manifest, declarations) = fixture(true);
    let call = &mut unit.tables.calls[0];
    call.receipts = None;
    let abi = &mut table.fn_abis[call.abi.0 as usize];
    abi.receipts = None;
    abi.key.receipt_contract = None;
    let ordinary = Declarations {
        ordinary: declarations.ordinary.clone(),
        sources: declarations.sources[..1].to_vec(),
    };
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let inputs = Inputs {
        target: &target,
        layouts: &table,
        manifest: &manifest,
        operations: &ordinary,
        discharge: None,
        memo: Default::default(),
    };
    let environment = NativeEnvironment {
        target: &target,
        manifest: &manifest,
        operations: &ordinary,
        discharge: None,
    };
    let directory = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("callee-receipt-replay-89-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    let store = hirsel_artifact_store::Store::new(directory.clone());
    let subject = support::hash("borrowed callee source replay");
    let policy = support::hash("borrowed callee source policy");
    let cache = NativeCache::new(Arc::new(store.clone()), Some(store.clone()));
    cache
        .validate_and_publish(
            subject,
            policy,
            unit.into_stage::<hirsel_lir::stage::Value>(),
            &inputs,
            &[],
            b"source reads",
        )
        .unwrap()
        .1
        .expect("actual C output");
    drop(cache);
    let cache = NativeCache::new(Arc::new(store.clone()), Some(store));
    let trace = hirsel_lir::construction::ValueTrace::default();
    let _observe = trace.enter();
    assert!(
        cache
            .lookup(subject, policy, StageKind::Value, &environment)
            .is_some()
    );
    let demanded = NativeEnvironment {
        operations: &declarations,
        ..environment
    };
    assert!(
        cache
            .lookup(subject, policy, StageKind::Value, &demanded)
            .is_none()
    );
    assert_eq!(
        trace.totals(),
        hirsel_lir::construction::ValueCounts::default()
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn take_data_entry_and_call_transfer_companions_only_after_admission() {
    let (unit, table, manifest, declarations) = fixture_mode(true, true);
    let errors = check(&unit, &table, &manifest, &declarations);
    assert!(errors.is_empty(), "{errors:#?}");
    let call = &unit.tables.calls[0];
    assert!(call.pretransfer_failure.is_some());
    assert!(
        call.receipts
            .as_ref()
            .unwrap()
            .arguments
            .iter()
            .all(|argument| matches!(argument, ReceiptArgument::Take { .. }))
    );
    // Normal and entered-failure continuations hold no transferred owners.
    for mutation in 0..4 {
        let mut unit = unit.clone();
        match mutation {
            0 => {
                let receipts = unit.tables.calls[0].receipts.as_mut().unwrap();
                let [
                    ReceiptArgument::Take { receipt: a, .. },
                    ReceiptArgument::Take { receipt: b, .. },
                ] = receipts.arguments.as_mut_slice()
                else {
                    unreachable!()
                };
                std::mem::swap(a, b);
            }
            1 => {
                // Rejected admission still owes every pending metadata owner.
                let metadata = unit.tables.calls[0]
                    .receipts
                    .as_ref()
                    .unwrap()
                    .inputs()
                    .next()
                    .unwrap();
                let released = unit
                    .instructions
                    .iter_mut()
                    .find(|i| matches!(i, Inst::Release { value, .. } if *value == metadata))
                    .unwrap();
                let Inst::Release { value, .. } = released else {
                    unreachable!()
                };
                *value = unit.tables.calls[0]
                    .receipts
                    .as_ref()
                    .unwrap()
                    .inputs()
                    .nth(1)
                    .unwrap();
            }
            2 => {
                // A taken entry is a real owner, not a borrowed-slot exemption.
                let binding = &unit.tables.receipt_entry.as_ref().unwrap().parameters[0];
                let read = unit
                    .instructions
                    .iter_mut()
                    .find(|i| {
                        matches!(i,
                    Inst::Read { reaching, .. } if *reaching == binding.version)
                    })
                    .unwrap();
                let Inst::Read { reaching, .. } = read else {
                    unreachable!()
                };
                *reaching = unit.tables.receipt_entry.as_ref().unwrap().parameters[1].version;
            }
            3 => {
                // Entered failure cannot run rejected-admission cleanup again.
                let rejected = unit.tables.calls[0].pretransfer_failure.unwrap();
                let Terminator::Call { failure, .. } = unit.terminators[ENTRY.index()] else {
                    unreachable!()
                };
                unit.tables.failures[failure.index()].cleanup =
                    unit.tables.failures[rejected.index()].cleanup;
            }
            _ => unreachable!(),
        }
        let errors = check(&unit, &table, &manifest, &declarations);
        assert!(!errors.is_empty(), "mutation {mutation}");
        if mutation == 0 {
            assert!(
                errors.iter().any(|e| matches!(&e.kind,
                validate::ViolationKind::Sealed(text) if text.contains("not paired"))),
                "{errors:#?}"
            );
        }
        if mutation == 1 {
            assert!(
                errors
                    .iter()
                    .any(|e| matches!(e.kind, validate::ViolationKind::Leak(_))),
                "{errors:#?}"
            );
        }
    }
}

#[test]
fn taking_an_entry_without_disposing_its_companion_leaks_storage() {
    let (unit, table, manifest, declarations) = fixture_mode(false, true);
    let errors = check(&unit, &table, &manifest, &declarations);
    let entry = unit.tables.receipt_entry.as_ref().unwrap();
    for binding in &entry.parameters {
        assert!(
            errors
                .iter()
                .any(|e| e.kind == validate::ViolationKind::LeakedStorage(binding.slot)),
            "{errors:#?}"
        );
    }
}
