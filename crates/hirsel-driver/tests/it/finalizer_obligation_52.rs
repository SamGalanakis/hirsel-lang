//! Mutations of real source-built Ready cleanup, checked through public C.
use hirsel_driver::{compiler_adapters, native};
use hirsel_engine::*;
use hirsel_lir::{ir::*, stage::Ready, validate};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

fn program() -> native::ReadyProgram {
    let source = r#"
resource type Leaf {n:u64}
drop(x) {()}
resource type Branch {inner:Leaf}
drop(x) {()}
resource type Top {branch:Branch, sibling:Leaf}
drop(x) {()}
pub fn main()->Result[(),()] {
 let root=Top(branch:Branch(inner:Leaf(n:1u64)),sibling:Leaf(n:2u64))
 Ok(())
}
"#;
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine
        .sources
        .add("finalizer-obligation.hir", source.to_owned())
        .unwrap();
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let input = engine.export_native_input().unwrap();
    let validated = engine.import_native_input(input).unwrap();
    native::build(
        &validated,
        &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
    )
    .unwrap()
}

fn main_unit(program: &native::ReadyProgram) -> LirUnit<Ready> {
    program
        .raw_units()
        .into_iter()
        .find(|unit| unit.header.definition.0.name == "main")
        .unwrap()
}
fn errors(
    program: &native::ReadyProgram,
    unit: &LirUnit<Ready>,
    operations: &hirsel_layout::operation::OperationRegistry,
) -> Vec<validate::Violation> {
    validate::check(
        unit,
        &validate::Inputs {
            memo: Default::default(),
            discharge: None,
            operations,
            target: &program.target,
            layouts: &program.table,
            manifest: &program.manifest,
        },
    )
}
fn path(unit: &LirUnit<Ready>, address: AddressId) -> &[ProjectionStep] {
    unit.addresses[address.index()]
        .projection
        .get(&unit.pools.projections)
        .unwrap()
}
fn replace_code(unit: &mut LirUnit<Ready>, code: Vec<Vec<Inst>>) {
    unit.instructions.clear();
    for (block, code) in unit.blocks.iter_mut().zip(code) {
        block.instructions = PoolRange::push(&mut unit.instructions, code);
    }
}
fn entries(unit: &LirUnit<Ready>) -> Vec<(BlockId, AddressId)> {
    unit.blocks
        .iter()
        .enumerate()
        .filter_map(|(index, block)| {
            if !matches!(unit.terminators[index], Terminator::Call { .. }) {
                return None;
            }
            let code = block.instructions.get(&unit.instructions).unwrap();
            code.iter().find_map(|inst| match *inst {
                Inst::FinalizeBegin { address, .. }
                    if code
                        .iter()
                        .any(|inst| matches!(inst, Inst::FrameEnter { .. })) =>
                {
                    Some((BlockId(index as u32), address))
                }
                _ => None,
            })
        })
        .collect()
}

#[test]
fn finalizer_obligation_52_sibling_is_not_an_owned_descendant() {
    with_compiler_stack(|| {
        let program = program();
        let mut unit = main_unit(&program);
        assert!(errors(&program, &unit, &program.operations).is_empty());
        let rows = entries(&unit);
        let (_, nested) = rows
            .iter()
            .find(|(_, address)| {
                path(&unit, *address) == [ProjectionStep::Field(0), ProjectionStep::Field(0)]
            })
            .unwrap();
        let (_, sibling) = rows
            .iter()
            .find(|(_, address)| path(&unit, *address) == [ProjectionStep::Field(1)])
            .unwrap();
        assert_eq!(
            unit.addresses[nested.index()].ty,
            unit.addresses[sibling.index()].ty
        );
        unit.addresses[nested.index()].projection = unit.addresses[sibling.index()].projection;
        let errors = errors(&program, &unit, &program.operations);
        assert!(errors.iter().any(|e| matches!(&e.kind, validate::ViolationKind::Schema(text) if text.contains("not an owned descendant finalizer"))), "{errors:?}");
    });
}

#[test]
fn finalizer_obligation_52_ordinary_admission_cannot_nest_in_cleanup() {
    with_compiler_stack(|| {
        let program = program();
        let mut unit = main_unit(&program);
        assert!(errors(&program, &unit, &program.operations).is_empty());
        let (block, _) = entries(&unit)
            .into_iter()
            .find(|(_, address)| path(&unit, *address).is_empty())
            .unwrap();
        let Terminator::Call {
            normal: Some(normal),
            ..
        } = unit.terminators[block.index()]
        else {
            panic!("cleanup call")
        };
        let target = unit.tables.edges[normal.index()].target;
        let mut operations = program.operations.clone();
        let source = unit.tables.source_anchors[0].clone();
        let anchor = SourceAnchorId(unit.tables.source_anchors.len() as u32);
        unit.tables.source_anchors.push(SourceAnchor {
            block: u32::MAX - 1,
            statement: 73,
            ..source
        });
        operations
            .invocations
            .get_mut(&unit.header.definition)
            .unwrap()
            .push(hirsel_lir::invocations::ReferenceInvocation {
                body: source.body,
                block: u32::MAX - 1,
                statement: 73,
                restart: false,
                primitive: None,
                callee: None,
            });
        let failure = FailureId(unit.tables.failures.len() as u32);
        let mut failure_row = unit.tables.failures[0].clone();
        let cleanup = EdgeId(unit.tables.edges.len() as u32);
        unit.tables
            .edges
            .push(unit.tables.edges[failure_row.cleanup.index()].clone());
        failure_row.cleanup = cleanup;
        failure_row.anchor = anchor;
        failure_row.rule = Some("LANG-10".into());
        unit.tables.failures.push(failure_row);
        let mut code: Vec<_> = unit
            .blocks
            .iter()
            .map(|block| block.instructions.get(&unit.instructions).unwrap().to_vec())
            .collect();
        code[target.index()].extend([Inst::FrameEnter { failure }, Inst::FrameLeave]);
        replace_code(&mut unit, code);
        let errors = errors(&program, &unit, &operations);
        assert!(errors.iter().any(|e| matches!(&e.kind, validate::ViolationKind::Schema(text) if text.contains("not an owned descendant finalizer"))), "{errors:?}");
    });
}

/// Locate matched retirement instructions only to move them in a valid fixture.
/// The verdict comes from C's independent declarations and ownership walk.
fn retirements(unit: &LirUnit<Ready>) -> BTreeMap<(BlockId, usize), AddressId> {
    let entries: BTreeMap<_, _> = entries(unit).into_iter().collect();
    let mut incoming = BTreeMap::from([(unit.entry, Vec::<Option<AddressId>>::new())]);
    let mut pending = VecDeque::from([unit.entry]);
    let mut retired = BTreeMap::new();
    while let Some(block) = pending.pop_front() {
        let mut stack = incoming[&block].clone();
        let mut outgoing = Vec::new();
        for (index, inst) in unit.blocks[block.index()]
            .instructions
            .get(&unit.instructions)
            .unwrap()
            .iter()
            .enumerate()
        {
            if let Some(failure) = inst.failure() {
                outgoing.push((unit.tables.failures[failure.index()].cleanup, stack.clone()));
            }
            match inst {
                Inst::FrameEnter { .. } => stack.push(entries.get(&block).copied()),
                Inst::FrameLeave => {
                    if let Some(Some(address)) = stack.pop() {
                        retired.insert((block, index), address);
                    }
                }
                _ => (),
            }
        }
        outgoing.extend(
            unit.terminators[block.index()]
                .normal_edges(&unit.pools)
                .into_iter()
                .map(|edge| (edge, stack.clone())),
        );
        outgoing.extend(
            unit.terminators[block.index()]
                .failure_edges(&unit.tables)
                .into_iter()
                .map(|failure| (unit.tables.failures[failure.index()].cleanup, stack.clone())),
        );
        for (edge, state) in outgoing {
            let target = unit.tables.edges[edge.index()].target;
            if let Some(previous) = incoming.get(&target) {
                assert_eq!(previous, &state);
            } else {
                incoming.insert(target, state);
                pending.push_back(target);
            }
        }
    }
    retired
}

#[test]
fn finalizer_obligation_52_retirement_requires_descendants_absent() {
    with_compiler_stack(|| {
        let program = program();
        let original = main_unit(&program);
        for origin in [UnitOrigin::Source, UnitOrigin::Library] {
            let mut unit = original.clone();
            unit.header.origin = origin;
            assert!(errors(&program, &unit, &program.operations).is_empty());
            let selected: BTreeSet<_> = entries(&unit)
                .into_iter()
                .filter(|(_, address)| path(&unit, *address) == [ProjectionStep::Field(0)])
                .map(|(_, address)| address)
                .collect();
            assert!(!selected.is_empty());
            let retired = retirements(&unit);
            let mut early = BTreeSet::new();
            for (block, address) in entries(&unit) {
                if !selected.contains(&address) {
                    continue;
                }
                let Terminator::Call {
                    normal: Some(normal),
                    failure,
                    ..
                } = unit.terminators[block.index()]
                else {
                    panic!("finalizer call")
                };
                early.insert(unit.tables.edges[normal.index()].target);
                early.insert(
                    unit.tables.edges[unit.tables.failures[failure.index()].cleanup.index()].target,
                );
            }
            let mut replacement = Vec::new();
            for index in 0..unit.blocks.len() {
                let block = BlockId(index as u32);
                let mut code: Vec<_> = unit.blocks[index]
                    .instructions
                    .get(&unit.instructions)
                    .unwrap()
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| {
                        retired
                            .get(&(block, *i))
                            .is_none_or(|address| !selected.contains(address))
                    })
                    .map(|(_, inst)| *inst)
                    .collect();
                if early.contains(&block) {
                    let after_phase = code
                        .iter()
                        .position(|inst| matches!(inst, Inst::FinalizeEnd { .. }))
                        .unwrap()
                        + 1;
                    code.insert(after_phase, Inst::FrameLeave);
                }
                replacement.push(code);
            }
            replace_code(&mut unit, replacement);
            let errors = errors(&program, &unit, &program.operations);
            assert!(errors.iter().any(|e| matches!(&e.kind, validate::ViolationKind::Schema(text) if text.contains("before owned descendants"))), "{errors:?}");
        }
    });
}
