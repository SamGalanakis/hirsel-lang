//! Private keyed adapters are checked against independent instance roles.
use crate::rc_support as support;
use hirsel_lir::{
    ir::*,
    stage::Value,
    validate::{self, Inputs, ViolationKind as K},
};

const SOURCE: &str = r#"
fn ordinary(x:u64)->u64 { 10u64/x }
pub fn main()->Result[(),()] {
 let mut s:Set[String]=set.empty()
 let insert:fn(mut Set[String],take String)->()=set.insert
 let _=insert(mut s,"key")
 let f=ordinary
 let _=f(2u64)
 Ok(())
}"#;

#[test]
fn keyed_adapter_custody_and_edge_adversaries() {
    hirsel_engine::with_compiler_stack(|| {
        let (program, lowered) = support::lower(SOURCE);
        let registry = lowered.operation_registry(&program);
        assert_eq!(registry.pretransfer_adapters.len(), 1);
        let manifest = support::manifest(&program);
        let target = support::target();
        let inputs = Inputs {
            memo: Default::default(),
            discharge: None,
            operations: &registry,
            target: &target,
            layouts: &lowered.table,
            manifest: &manifest,
        };
        for unit in &lowered.units {
            let errors = validate::check(unit, &inputs);
            assert!(
                errors.is_empty(),
                "{}: {errors:?}\n{}",
                unit.header.definition,
                support::dump(unit)
            );
        }
        let caller = lowered
            .units
            .iter()
            .find(|u| u.header.definition == support::path("main"))
            .unwrap();
        let mut missing = caller.clone();
        let gate = missing
            .tables
            .calls
            .iter_mut()
            .find(|c| c.pretransfer_failure.is_some())
            .expect("actual materialized adapter dispatch");
        gate.pretransfer_failure = None;
        let missing = hirsel_rc::insert::Editor::new(missing).finish();
        let errors = validate::check(&missing, &inputs);
        assert!(
            errors
                .iter()
                .any(|e| matches!(&e.kind,K::Schema(s) if s.contains("missing pretransfer edge"))),
            "{errors:?}"
        );

        let mut extra = caller.clone();
        let (call,failure)=extra.terminators.iter().find_map(|t|match *t {
            Terminator::Call {call,failure,..} if matches!(extra.tables.calls[call.index()].callee,Callee::Direct(s) if extra.tables.symbols[s.index()].definition==support::path("ordinary")) =>Some((call,failure)),_=>None,
        }).expect("known ordinary callable devirtualized with unchanged protocol");
        assert!(
            extra.tables.calls[call.index()]
                .pretransfer_failure
                .is_none()
        );
        let mut row = extra.tables.failures[failure.index()].clone();
        let edge = extra.tables.edges[row.cleanup.index()].clone();
        row.cleanup = EdgeId(extra.tables.edges.len() as u32);
        extra.tables.edges.push(edge);
        let failure = FailureId(extra.tables.failures.len() as u32);
        extra.tables.failures.push(row);
        extra.tables.calls[call.index()].pretransfer_failure = Some(failure);
        let errors = validate::check(&extra, &inputs);
        assert!(
            errors
                .iter()
                .any(|e| matches!(&e.kind,K::Schema(s) if s.contains("extra pretransfer edge"))),
            "{errors:?}"
        );

        let adapter = lowered
            .units
            .iter()
            .find(|u| hirsel_lir::pretransfer::primitive(&u.header.definition).is_some())
            .unwrap();
        let mut early = adapter.clone();
        let hash = early
            .instructions
            .iter()
            .position(|i| matches!(i, Inst::Hash { .. }))
            .unwrap();
        let take = early
            .instructions
            .iter()
            .position(|i| {
                matches!(
                    i,
                    Inst::Read {
                        mode: ReadMode::Take,
                        ..
                    }
                )
            })
            .unwrap();
        early.instructions.swap(hash, take);
        let error = hirsel_lir::pretransfer::check_adapter(
            &early,
            &lowered.table,
            &registry.pretransfer_adapters,
        )
        .unwrap_err();
        assert!(error.contains("before key admission"), "{error}");
        let errors = validate::check(&early, &inputs);
        assert!(
            errors
                .iter()
                .any(|e| matches!(&e.kind,K::Schema(s) if s.contains("before key admission"))),
            "{errors:?}"
        );

        let ordinary = lowered
            .units
            .iter()
            .find(|u| u.header.definition == support::path("ordinary"))
            .unwrap();
        let mut forged: LirUnit<Value> = ordinary.clone();
        let term = forged
            .terminators
            .iter_mut()
            .find(|t| **t == Terminator::Propagate)
            .unwrap();
        *term = Terminator::RejectTransfer;
        let errors = validate::check(&forged, &inputs);
        assert!(
            errors
                .iter()
                .any(|e| e.kind == K::Vocabulary(hirsel_lir::effect::Opcode::RejectTransfer)),
            "{errors:?}"
        );
    });
}

const CALLBACK_SOURCE: &str =
    "pub fn main()->Result[(),()] {arena.with(fn(a:Arena)->() {()});Ok(())}";

fn erase_callback<S: hirsel_lir::stage::Stage>(unit: LirUnit<S>) -> LirUnit<S> {
    let mut ed = hirsel_rc::insert::Editor::new(unit);
    for b in 0..ed.code.len() {
        ed.code[b].retain(|i| !matches!(i, Inst::FrameEnter { .. } | Inst::FrameLeave));
        if let Terminator::Call {
            call,
            normal: Some(normal),
            ..
        } = ed.unit.terminators[b]
        {
            if let Some(result) = ed.unit.tables.calls[call.index()].result {
                let ty = ed.unit.values[result.index()].ty;
                let constant = ConstantId(ed.unit.tables.constants.len() as u32);
                ed.unit.tables.constants.push(ConstantData {
                    ty,
                    value: ConstantValue::Unit,
                });
                ed.code[b].push(Inst::Const { result, constant });
            }
            ed.unit.terminators[b] = Terminator::Jump(normal);
        }
    }
    ed.finish()
}

#[test]
fn callback_recipe_c_rejects_dispatch_and_both_events_erased() {
    hirsel_engine::with_compiler_stack(|| {
        let (program, lowered) = support::lower(CALLBACK_SOURCE);
        let registry = lowered.operation_registry(&program);
        let manifest = support::manifest(&program);
        let target = support::target();
        let inputs = Inputs {
            memo: Default::default(),
            discharge: None,
            operations: &registry,
            target: &target,
            layouts: &lowered.table,
            manifest: &manifest,
        };
        let unit = lowered
            .units
            .iter()
            .find(|u| u.header.definition.is_core("arena.with"))
            .unwrap();
        let errors = validate::check(unit, &inputs);
        assert!(errors.is_empty(), "{errors:?}");
        let erased = erase_callback(unit.clone());
        assert!(erased.tables.calls.is_empty());
        let errors = validate::check(&erased, &inputs);
        assert!(errors.iter().any(|e|matches!(&e.kind,K::Schema(s) if s.contains("callback recipe invocation coverage"))),"{errors:?}\n{}",support::dump(&erased));
    });
}

#[test]
fn callback_recipe_h_rejects_dispatch_and_both_events_erased() {
    hirsel_engine::with_compiler_stack(|| {
        let program = support::compile(CALLBACK_SOURCE);
        let ready = hirsel_driver::native::build_program(
            &program,
            support::manifest(&program),
            &hirsel_driver::native::Options::new(support::target()),
        )
        .unwrap();
        let mut heap = hirsel_heapcheck::Program {
            target: ready.target.clone(),
            layouts: ready.table.clone(),
            units: ready.raw_units(),
            statics: ready.statics.clone(),
            invocation_bodies: ready.operations.invocation_bodies.clone(),
            invocations: ready.operations.invocations.clone(),
            pretransfer_adapters: ready.operations.pretransfer_adapters.clone(),
        };
        let accepted = hirsel_heapcheck::run(
            &heap,
            &support::path("main"),
            &[],
            &mut hirsel_heapcheck::NoOperations,
            &hirsel_heapcheck::Config::default(),
        );
        assert!(accepted.result.is_ok(), "{:?}", accepted.result);
        let index = heap
            .units
            .iter()
            .position(|u| u.header.definition.is_core("arena.with"))
            .unwrap();
        heap.units[index] = erase_callback(heap.units[index].clone());
        assert!(heap.units[index].tables.calls.is_empty());
        let rejected = hirsel_heapcheck::run(
            &heap,
            &support::path("main"),
            &[],
            &mut hirsel_heapcheck::NoOperations,
            &hirsel_heapcheck::Config::default(),
        );
        let error = rejected.result.unwrap_err();
        assert_eq!(error.kind, hirsel_heapcheck::ViolationKind::FrameImbalance);
        assert!(
            error
                .to_string()
                .contains("callback recipe invocation coverage"),
            "{error}"
        );
    });
}

#[test]
fn source_evidence_factory_31_uses_original_source_and_planned_instances() {
    hirsel_engine::with_compiler_stack(|| {
        let (program, mut lowered) = support::lower(SOURCE);
        let mut signatures = hirsel_validate::SignatureMap::new(program.types.clone());
        signatures.extend(
            program
                .signatures
                .iter()
                .map(|(path, signature)| (path, (**signature).clone())),
        );
        let id = lowered
            .instances
            .ids()
            .find(|id| lowered.instances.view(*id).key.definition().0.name == "main")
            .unwrap();
        let view = lowered.instances.view(id);
        let symbol = Symbol {
            kind: SymbolKind::Function,
            definition: view.key.definition().clone(),
            instance: Some(view.key.digest()),
        };
        let source = &program.definitions[&symbol.definition];
        let derive = |symbol: &Symbol, body| {
            hirsel_rc::invocations::source_instance(
                &program.types,
                source,
                &signatures,
                &program.operations,
                symbol,
                body,
            )
        };
        let evidence = derive(&symbol, view.body).unwrap();
        assert!(
            evidence
                .invocations
                .iter()
                .any(|row| row.primitive.is_some())
        );
        assert!(evidence.invocations.iter().any(|row| row.callee.is_none()));
        // Replacing candidate body selectors cannot change independent evidence.
        for unit in &mut lowered.units {
            unit.tables.source_anchors.clear();
        }
        assert_eq!(derive(&symbol, view.body).unwrap(), evidence);
        assert!(derive(&symbol, hirsel_tir::BodyId(source.bodies.len() as u32)).is_err());
        let mut unbound = symbol.clone();
        unbound.instance = None;
        assert!(derive(&unbound, view.body).is_err());
        let registry = lowered.operation_registry(&program);
        let expected = &registry.pretransfer_adapters[0];
        let adapter_id = lowered
            .instances
            .ids()
            .find(|id| lowered.instances.view(*id).key.definition() == &expected.adapter.definition)
            .unwrap();
        let primitive_id = lowered
            .instances
            .ids()
            .find(|id| {
                lowered.instances.view(*id).key.definition() == &expected.primitive.definition
            })
            .unwrap();
        let adapter = lowered.instances.view(adapter_id);
        let primitive = lowered.instances.view(primitive_id);
        let actuals = &lowered.instances.actuals[adapter_id.index()].arguments;
        let primitive_actuals = &lowered.instances.actuals[primitive_id.index()].arguments;
        let abi = lowered.instances.abis[adapter_id.index()];
        assert_eq!(
            &hirsel_rc::invocations::keyed_adapter(
                adapter,
                actuals,
                primitive,
                primitive_actuals,
                abi
            )
            .unwrap(),
            expected
        );
        assert!(
            hirsel_rc::invocations::keyed_adapter(adapter, actuals, primitive, &[], abi).is_err()
        );
        assert!(
            hirsel_rc::invocations::keyed_adapter(adapter, actuals, view, actuals, abi).is_err()
        );
    });
}

#[test]
fn cached_pretransfer_inventory_rederives_canonical_abis_from_independent_plans() {
    use hirsel_lir::stage::StageKind;
    use hirsel_lir::validate::{NativeCache, NativeEnvironment};
    use std::sync::Arc;
    hirsel_engine::with_compiler_stack(|| {
        let (program, lowered) = support::lower(SOURCE);
        let registry = lowered.operation_registry(&program);
        assert_eq!(registry.pretransfer_adapters.len(), 1);
        assert_eq!(registry.pretransfer_actuals.len(), 1);
        let manifest = support::manifest(&program);
        let target = support::target();
        let caller = lowered
            .units
            .iter()
            .find(|u| u.header.definition == support::path("main"))
            .unwrap();
        let inputs = Inputs {
            memo: Default::default(),
            target: &target,
            layouts: &lowered.table,
            manifest: &manifest,
            operations: &registry,
            discharge: None,
        };
        let store = Arc::new(hirsel_artifact_store::Memory::default());
        let cache = NativeCache::new(store, None);
        let subject = hirsel_syntax::ContentHash::of_bytes(b"pretransfer independent input");
        let policy = hirsel_syntax::ContentHash::of_bytes(b"pretransfer canonical ABI");
        let (_, output) = cache
            .validate_and_publish(
                subject,
                policy,
                caller.clone(),
                &inputs,
                &[],
                b"checked targets",
            )
            .unwrap();
        assert!(
            output.is_some(),
            "actual strict C can publish a canonical nonempty target inventory"
        );
        let accepts = |operations: &hirsel_layout::operation::OperationRegistry| {
            cache
                .lookup(
                    subject,
                    policy,
                    StageKind::Value,
                    &NativeEnvironment {
                        target: &target,
                        manifest: &manifest,
                        operations,
                        discharge: None,
                    },
                )
                .is_some()
        };
        let trace = hirsel_lir::construction::ValueTrace::default();
        let observe = trace.enter();
        assert!(accepts(&registry));
        let mut missing = registry.clone();
        missing.pretransfer_adapters.clear();
        assert!(!accepts(&missing));
        let mut missing = registry.clone();
        missing.pretransfer_actuals.clear();
        assert!(!accepts(&missing), "a bare ABI index cannot accept a hit");
        let mut crossed = registry.clone();
        crossed.pretransfer_adapters[0].abi = hirsel_lir::FnAbiId(u32::MAX);
        assert!(
            !accepts(&crossed),
            "adapter index must match its independent plan binding"
        );
        let mut duplicate = registry.clone();
        duplicate
            .pretransfer_actuals
            .push(duplicate.pretransfer_actuals[0].clone());
        assert!(!accepts(&duplicate));
        let mut changed = registry.clone();
        changed.pretransfer_actuals[0].3.clear();
        assert!(
            !accepts(&changed),
            "typed actuals must independently rederive the ABI"
        );
        let mut rebound_table = lowered.table.clone();
        let old = registry.pretransfer_adapters[0].abi;
        let rebound_id = hirsel_lir::FnAbiId(rebound_table.fn_abis.len() as u32);
        rebound_table
            .fn_abis
            .push(rebound_table.fn_abis[old.0 as usize].clone());
        let mut rebound = registry.clone();
        rebound.pretransfer_adapters[0].abi = rebound_id;
        rebound.pretransfer_actuals[0].1 = rebound_id;
        assert_eq!(
            hirsel_lir::LayoutKeys::new(&rebound_table)
                .fn_abi(&rebound_table.fn_abis[rebound_id.0 as usize]),
            hirsel_lir::LayoutKeys::new(&lowered.table)
                .fn_abi(&lowered.table.fn_abis[old.0 as usize])
        );
        assert!(
            accepts(&rebound),
            "a consistent independent table reindex leaves canonical ABI identity unchanged"
        );
        assert_eq!(
            trace.totals(),
            Default::default(),
            "hit admission opens no Value"
        );
        drop(observe);
        let (fresh, fresh_lowered) = support::lower(SOURCE);
        let independent = fresh_lowered.operation_registry(&fresh);
        assert!(
            accepts(&independent),
            "current independent plan with another type interner agrees"
        );
    });
}

#[test]
fn keyed_map_resource_actual_keeps_gate_custody_through_ready_cleanup() {
    hirsel_engine::with_compiler_stack(|| {
        let source = r#"
resource type Payload {}
drop(x) {}
pub fn main()->Result[(),()] {
 let mut target:Map[(u64,),Payload]=map.empty()
 let insert:fn(mut Map[(u64,),Payload],take (u64,),take Payload)->()=map.insert
 insert(mut target,take (1u64,),take Payload())
 Ok(())
}"#;
        let program = support::compile(source);
        let ready = hirsel_driver::native::build_program(
            &program,
            support::manifest(&program),
            &hirsel_driver::native::Options::new(support::target()),
        )
        .unwrap();
        assert!(
            ready
                .raw_units()
                .iter()
                .any(|unit| unit.header.definition.is_core("$native.keyed_map_insert"))
        );
    });
}
