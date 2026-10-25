//! Mutations of real source-built Ready cleanup, checked through public C.
use hirsel_driver::{compiler_adapters, native};
use hirsel_engine::*;
use hirsel_lir::{ir::*, stage::Ready, validate};
use std::collections::{BTreeMap, BTreeSet};

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
    source_program(source)
}

fn source_program(source: &str) -> native::ReadyProgram {
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
    let mut flags = BTreeMap::new();
    for (block, resource) in entries(unit) {
        let code = unit.blocks[block.index()]
            .instructions
            .get(&unit.instructions)
            .unwrap();
        let enter = code
            .iter()
            .position(|inst| matches!(inst, Inst::FrameEnter { .. }))
            .unwrap();
        let address = code[enter + 1..]
            .iter()
            .find_map(|inst| match inst {
                Inst::Store { address, .. } => Some(*address),
                _ => None,
            })
            .unwrap();
        flags.insert(address, resource);
    }
    let mut retired = BTreeMap::new();
    for (index, block) in unit.blocks.iter().enumerate() {
        let Terminator::Branch { condition, yes, .. } = unit.terminators[index] else {
            continue;
        };
        let code = block.instructions.get(&unit.instructions).unwrap();
        let Some(address) = code.iter().find_map(|inst| match inst {
            Inst::Read {
                result, address, ..
            } if *result == condition => Some(*address),
            _ => None,
        }) else {
            continue;
        };
        let Some(resource) = flags.get(&address) else {
            continue;
        };
        let target = unit.tables.edges[yes.index()].target;
        let code = unit.blocks[target.index()]
            .instructions
            .get(&unit.instructions)
            .unwrap();
        let ordinal = code
            .iter()
            .position(|inst| matches!(inst, Inst::FrameLeave))
            .unwrap();
        retired.insert((target, ordinal), *resource);
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

#[test]
fn finalizer_obligation_52_nested_cleanup_has_shared_failure_suffixes() {
    with_compiler_stack(|| {
        let counts = [2usize, 4, 8].map(|depth| {
            let mut source = String::from("resource type R0 {n:u64}\ndrop(x) {()}\n");
            let mut value = String::from("R0(n:0u64)");
            for level in 1..depth {
                source.push_str(&format!(
                    "resource type R{level} {{inner:R{}}}\ndrop(x) {{()}}\n",
                    level - 1
                ));
                value = format!("R{level}(inner:{value})");
            }
            source.push_str(&format!(
                "pub fn main()->Result[(),()] {{let root={value};Ok(())}}\n"
            ));
            let program = source_program(&source);
            program
                .raw_units()
                .iter()
                .flat_map(|unit| &unit.instructions)
                .filter(|inst| matches!(inst, Inst::FinalizeBegin { .. }))
                .count()
        });
        let first = counts[1].checked_sub(counts[0]).unwrap();
        let second = counts[2].checked_sub(counts[1]).unwrap();
        assert!(
            first > 0 && 2 * second <= 5 * first,
            "FinalizeBegin at inline resource depths 2/4/8: {counts:?}"
        );
        eprintln!("FinalizeBegin at inline resource depths 2/4/8: {counts:?}");
    });
}

#[test]
fn finalizer_obligation_52_false_guard_cannot_hide_an_entered_cleanup() {
    with_compiler_stack(|| {
        let program = program();
        let mut unit = main_unit(&program);
        assert!(errors(&program, &unit, &program.operations).is_empty());
        let mut changed = 0;
        for (block, address) in entries(&unit) {
            if !path(&unit, address).is_empty() {
                continue;
            }
            let code = unit.blocks[block.index()]
                .instructions
                .get(&unit.instructions)
                .unwrap();
            let enter = code
                .iter()
                .position(|inst| matches!(inst, Inst::FrameEnter { .. }))
                .unwrap();
            let constant = code[enter + 1..]
                .iter()
                .find_map(|inst| match inst {
                    Inst::Const { constant, .. }
                        if matches!(
                            unit.tables.constants[constant.index()].value,
                            ConstantValue::Bool(true)
                        ) =>
                    {
                        Some(*constant)
                    }
                    _ => None,
                })
                .unwrap();
            unit.tables.constants[constant.index()].value = ConstantValue::Bool(false);
            changed += 1;
        }
        assert!(changed > 0);
        let violations = errors(&program, &unit, &program.operations);
        assert!(violations.iter().any(|v| matches!(&v.kind, validate::ViolationKind::Schema(message) if message.contains("guard") || message.contains("invocation"))), "{violations:?}");
    });
}
