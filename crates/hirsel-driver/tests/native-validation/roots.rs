//! Root lifetime anchors (S-14, lane R7) over driver-derived programs: content that
//! reaches a `Shared` root keeps its drop at its TIR cleanup anchor through placement,
//! insertion and elision, and the stage validator's RC-ROOT rule refuses the mutations
//! that would end such a holder early: a release at its last use, and a copy cancelled
//! into a move across the thaw that observes the holders.

use super::*;
use hirsel_layout::operation::OperationRegistry;
use hirsel_lir::ir::{AddressBase, BlockKind, Inst, LirUnit, ReadMode};
use hirsel_lir::layout_types::LayoutTable;
use hirsel_lir::stage::{Rc, Stage};
use hirsel_lir::validate::{Inputs, ViolationKind};
use hirsel_rc::insert::{Editor, Proofs};
use hirsel_rc::instance::Program;
use hirsel_rc::placement::Options;
fn target() -> TargetSpec {
    TargetSpec::x86_64_unknown_linux_gnu()
}
fn manifest(program: &Program) -> hirsel_core_contract::Manifest {
    hirsel_core_contract::Manifest::skeleton(program.operations.values().cloned())
}
fn lower_valid(source: &str) -> (Program, hirsel_rc::import::Lowered) {
    hirsel_engine::with_compiler_stack(|| {
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            crate::compiler_adapters(),
        );
        let file = engine.sources.add("root-proof.hir", source.into()).unwrap();
        let output =
            hirsel_engine::check_progress(&mut engine, file, &hirsel_engine::default_context())
                .unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let program = super::program(&validated);
        let lowered = hirsel_rc::import::lower(&program, &target()).unwrap();
        (program, lowered)
    })
}

/// `s` is unused after its copy, but holds its root until its scope's cleanup: the
/// thaw of the copy finds two holders (the interpreter's `Err`).
const COPY_THEN_THAW: &str = r#"
fn probe() -> bool {
  let built: String = "value"
  let s: Shared[String] = shared.freeze(take built)
  let t = copy s
  match shared.thaw(take t) {
    Ok(text) => true,
    Error(root) => false,
  }
}
"#;

/// `probe` through placement with regions, insertion and elision, with the
/// operations its lowering binds: each source unit's independent body selection.
fn rc_probe(source: &str) -> (Program, LayoutTable, LirUnit<Rc>, OperationRegistry) {
    let (program, lowered) = lower_valid(source);
    let table = lowered.table.clone();
    let operations = lowered.operation_registry(&program);
    let (i, unit) = lowered
        .units
        .iter()
        .enumerate()
        .find(|(_, u)| u.header.definition.0.name == "probe")
        .expect("probe");
    let id = lowered.unit_origins[i];
    let placed = hirsel_rc::placement::place(
        unit.clone(),
        &table,
        &Options::for_instance(true, &lowered.instances, id),
    )
    .expect("placement");
    let proofs = Proofs::for_instance(&lowered.instances, id);
    let rc = hirsel_rc::insert::insert(placed, &table, &proofs).expect("insertion");
    let rc = hirsel_rc::elide::elide(rc, &table).expect("elision");
    (program, table, rc, operations)
}

fn check<S: Stage>(
    program: &Program,
    operations: &OperationRegistry,
    table: &LayoutTable,
    unit: &LirUnit<S>,
) -> Vec<ViolationKind> {
    let manifest = manifest(program);
    let target = target();
    let inputs = Inputs {
        memo: Default::default(),
        discharge: None,
        operations,
        target: &target,
        layouts: table,
        manifest: &manifest,
    };
    hirsel_lir::validate::check(unit, &inputs)
        .into_iter()
        .map(|v| v.kind)
        .collect()
}

/// The copy of `s` in COPY_THEN_THAW: its block and index, and the normal-exit cleanup
/// drop of the slot it reads (block and index).
fn copy_and_anchor(ed: &Editor<Rc>) -> ((usize, usize), (usize, usize)) {
    let copies: Vec<_> = (0..ed.code.len())
        .flat_map(|b| (0..ed.code[b].len()).map(move |i| (b, i)))
        .filter(|&(b, i)| matches!(ed.code[b][i], Inst::Copy { .. }))
        .collect();
    assert_eq!(copies.len(), 1, "one copy of the root");
    let (b, i) = copies[0];
    let Inst::Copy { address, .. } = ed.code[b][i] else {
        unreachable!()
    };
    let AddressBase::Slot(slot) = ed.unit.addresses[address.index()].base else {
        panic!("the copy reads a slot");
    };
    let anchor = (0..ed.code.len())
        .filter(|&c| ed.unit.blocks[c].kind == BlockKind::Cleanup)
        .flat_map(|c| (0..ed.code[c].len()).map(move |j| (c, j)))
        .find(|&(c, j)| match ed.code[c][j] {
            Inst::Drop { address, .. } => {
                ed.unit.addresses[address.index()].base == AddressBase::Slot(slot)
            }
            _ => false,
        })
        .expect("the slot's drop at its cleanup anchor");
    ((b, i), anchor)
}

#[test]
fn ready_only_rejects_existing_rc_root_faults() {
    use hirsel_lir::stage::Ready;
    let mut missed = vec![];
    for fault in 0..3 {
        let (program, table, rc, operations) = rc_probe(COPY_THEN_THAW);
        let finalizers = hirsel_rc::cleanup::Finalizers::planned(
            &lower_valid(COPY_THEN_THAW).1.instances,
            &table,
        );
        let mut ed = Editor::new(rc);
        let ((b, i), (c, j)) = copy_and_anchor(&ed);
        if fault == 1 {
            let drop = ed.code[c].remove(j);
            ed.code[b].insert(i + 1, drop);
        } else if fault == 2 {
            let Inst::Copy {
                result,
                address,
                reaching,
                ..
            } = ed.code[b][i]
            else {
                unreachable!()
            };
            ed.code[b][i] = Inst::Read {
                result,
                address,
                reaching,
                mode: ReadMode::Take,
            };
            ed.code[c].remove(j);
        }
        let mutant = ed.finish();
        let ladder = check(&program, &operations, &table, &mutant);
        let reused =
            hirsel_rc::reuse::reuse(mutant, &table, &hirsel_rc::reuse::Options::default()).unwrap();
        let ready: LirUnit<Ready> = hirsel_rc::ready::ready(reused, &table, &finalizers).unwrap();
        let manifest = manifest(&program);
        let target = target();
        let errors = hirsel_lir::validate::check(
            &ready,
            &Inputs {
                memo: Default::default(),
                discharge: None,
                operations: &operations,
                target: &target,
                layouts: &table,
                manifest: &manifest,
            },
        );
        eprintln!("fault {fault}: ladder={ladder:?}, Ready={errors:?}");
        if fault == 0 {
            assert!(errors.is_empty(), "{errors:?}")
        } else {
            assert!(!ladder.is_empty());
            if errors.is_empty() {
                missed.push((fault, ladder));
            }
        }
    }
    assert!(
        missed.is_empty(),
        "Ready accepted RC-ROOT faults: {missed:?}"
    );
}

#[test]
fn ready_only_rejects_a_resource_released_before_its_cleanup_anchor() {
    let source = "resource type R { text: String }\ndrop(x) { let _ = 1u8 / 0u8 }\nfn probe() { let r = box R(text: \"value\") }\n";
    rejects_early_resource_release(source);
}

#[test]
fn dynamic_resource_list_replacement_still_rejects_early_owner_release() {
    rejects_early_resource_release(
        "resource type R {}\ndrop(x) {}\nfn probe() {\n  let mut xs: List[R] = [R()]\n  let i: u64 = 0\n  xs[i] = R()\n}\n",
    );
}

fn rejects_early_resource_release(source: &str) {
    use hirsel_lir::ir::{Holding, MaskRef};
    let (program, table, rc, operations) = rc_probe(source);
    let instances = lower_valid(source).1.instances;
    let finalizers = hirsel_rc::cleanup::Finalizers::planned(&instances, &table);
    let control = hirsel_rc::ready::ready(rc.clone(), &table, &finalizers).unwrap();
    let errors = check(&program, &operations, &table, &control);
    assert!(errors.is_empty(), "valid resource cleanup: {errors:?}");
    let mut ed = Editor::new(rc);
    let (b, i) = ed
        .code
        .iter()
        .enumerate()
        .flat_map(|(b, code)| code.iter().enumerate().map(move |(i, inst)| (b, i, inst)))
        .find_map(|(b, i, inst)| {
            // End the original owner before its lexical cleanup, not a
            // replacement's already displaced reference at its instruction.
            (ed.unit.blocks[b].kind == BlockKind::Cleanup && matches!(inst, Inst::Drop { .. }))
                .then_some((b, i))
        })
        .expect("resource drop");
    let Inst::Drop {
        address,
        anchor,
        failure,
        ..
    } = ed.code[b][i]
    else {
        unreachable!()
    };
    let AddressBase::Slot(slot) = ed.unit.addresses[address.index()].base else {
        panic!("slot")
    };
    let ty = ed.unit.addresses[address.index()].ty;
    let layout = ed.unit.types[ty.index()].layout;
    // The slot's current version is the last Store of this address on this acyclic path.
    let (store_address, reaching) = ed
        .code
        .iter()
        .flatten()
        .filter_map(|inst| match inst {
            Inst::Store {
                address: a,
                version,
                ..
            } if ed.unit.addresses[a.index()].base == AddressBase::Slot(slot) => {
                Some((*a, *version))
            }
            _ => None,
        })
        .last()
        .expect("resource store");
    let value = ed.value(ty, Holding::Owned);
    ed.code[b].splice(
        i..i + 1,
        [
            Inst::Read {
                result: value,
                address: store_address,
                reaching,
                mode: ReadMode::Take,
            },
            Inst::Release {
                value,
                layout: match table.layouts[layout.0 as usize].kind {
                    hirsel_lir::layout_types::LayoutKind::Box { payload }
                    | hirsel_lir::layout_types::LayoutKind::Block { payload } => payload,
                    _ => panic!("boxed resource"),
                },
                live: MaskRef::All,
                anchor,
                failure,
            },
        ],
    );
    let released: Vec<_> = ed.code[b].drain(i..i + 2).collect();
    let (body, stored) = ed
        .code
        .iter()
        .enumerate()
        .find_map(|(body, code)| {
            code.iter()
                .position(
                    |inst| matches!(inst, Inst::Store { version, .. } if *version == reaching),
                )
                .map(|at| (body, at))
        })
        .unwrap();
    ed.code[body].splice(stored + 1..stored + 1, released);
    let mutant = ed.finish();
    let ladder = check(&program, &operations, &table, &mutant);
    assert!(
        ladder.contains(&ViolationKind::ResourceOutsideAnchor(value)),
        "{ladder:?}"
    );
    let ready = hirsel_rc::ready::ready(mutant, &table, &finalizers).unwrap();
    let rows = manifest(&program);
    let target = target();
    let errors = hirsel_lir::validate::check(
        &ready,
        &Inputs {
            memo: Default::default(),
            target: &target,
            layouts: &table,
            manifest: &rows,
            operations: &operations,
            discharge: None,
        },
    );
    assert!(
        errors
            .iter()
            .any(|v| v.kind == ViolationKind::ResourceOutsideAnchor(value)),
        "Ready did not reject the resource lifetime fault: {errors:?}; ladder: {ladder:?}"
    );
}

#[test]
fn dynamic_resource_list_replacement_keeps_its_cleanup_anchors() {
    for index in [0, 7] {
        let source = format!(
            "resource type R {{}}\ndrop(x) {{}}\nfn probe() {{\n  let mut xs: List[R] = [R()]\n  let i: u64 = {index}\n  xs[i] = R()\n}}\n"
        );
        let (program, table, rc, operations) = rc_probe(&source);
        assert!(check(&program, &operations, &table, &rc).is_empty());
        let instances = lower_valid(&source).1.instances;
        let finalizers = hirsel_rc::cleanup::Finalizers::planned(&instances, &table);
        let ready = hirsel_rc::ready::ready(rc, &table, &finalizers).unwrap();
        let errors = check(&program, &operations, &table, &ready);
        assert!(errors.is_empty(), "index {index}: {errors:?}");
    }
}
