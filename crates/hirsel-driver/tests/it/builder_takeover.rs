//! A flat record of one list built only for its pushes takes the builder's block when the
//! builder is unique (`hirsel-llvm` `adopt_builder`); a shared builder is copied instead.
//! No source program shares a builder that import chose, so the copy branch is driven by
//! a forged twin of the producer's own unit: the builder gains a second holder before the
//! record is constructed and loses it after, on the normal and the failure path, which C
//! seals like any unit. Both programs must build the same record as an ordinary list, under
//! the diagnostic and the production runtime.
use hirsel_driver::compiler_adapters;
use hirsel_driver::native::{
    OptLevel, Options, Partition, ReadyProgram, TargetSpec, Toolchain, build, emit_ir, launch, link,
};
use hirsel_engine::{
    QueryEngine, check_progress, default_context, default_environment, entry_path,
    with_compiler_stack,
};
use hirsel_lir::ir::*;
use std::path::PathBuf;

const SOURCE: &str = "flat type Message { id: u64, tags: List[u64], payload: Bytes }
flat type Messages { items: List[Message] }
fn digest(messages: Messages) -> u64 {
  let mut s: u64 = list.len(messages.items)
  for m in messages.items {
    let mut t: u64 = list.len(m.tags)
    for tag in m.tags {
      t = int.wrapping_add(t ^ 2654435761u64, tag)
    }
    s = int.wrapping_add(s ^ 1540483477u64, m.id ^ t ^ bytes.len(m.payload))
  }
  s
}
fn build(n: u64) -> Messages {
  let mut items: List[Message] = []
  for i in 0u64..n {
    list.push(mut items, Message(id: i, tags: [i, 2u64 * i], payload: b\"abc\"))
  }
  Messages(items: take items)
}
fn ordinary(n: u64) -> u64 {
  let mut items: List[Message] = []
  for i in 0u64..n {
    let seen = list.len(items)
    list.push(mut items, Message(id: i + seen - seen, tags: [i, 2u64 * i], payload: b\"abc\"))
  }
  digest(Messages(items: take items))
}
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let s = io.stdout(rt)?
  let ok = digest(build(0)) == ordinary(0) && digest(build(1)) == ordinary(1) && digest(
    build(40)
  ) == ordinary(40)
  let _ = out.print(s, if ok { \"ok\\n\" } else { \"bad\\n\" })
  Ok(())
}
";

fn ready(target: &TargetSpec) -> ReadyProgram {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("takeover.hir", SOURCE.into()).unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let input = engine.export_native_input().unwrap();
    let validated = engine.import_native_input(input).unwrap();
    build(&validated, &Options::new(target.clone())).unwrap()
}

/// `build`'s unit with its builder `v` held twice across the takeover construct: `Dup v`
/// before `v` is stored into the borrowed slot, and `v` released after the slot's own
/// release. Each failure edge between them (the construct's, and the slot release's) first
/// passes through a cleanup block releasing `v`.
fn share_the_builder(program: &mut ReadyProgram) {
    let index = program
        .units
        .iter()
        .position(|u| u.unit().header.definition.0.name == "build")
        .expect("build is lowered");
    let mut unit = program.units[index].unit().clone();
    let mut code: Vec<Vec<Inst>> = unit
        .blocks
        .iter()
        .map(|b| {
            unit.instructions[b.instructions.start as usize..][..b.instructions.len as usize]
                .to_vec()
        })
        .collect();
    // Store a <- v; BeginLoan a; Read Borrow a; Construct: the dying builder's construct.
    let (block, store, builder, constructed) = code
        .iter()
        .enumerate()
        .find_map(|(b, insts)| {
            (3..insts.len()).find_map(|k| {
                match (&insts[k - 3], &insts[k - 2], &insts[k - 1], &insts[k]) {
                    (
                        Inst::Store { value, .. },
                        Inst::BeginLoan { .. },
                        Inst::Read {
                            mode: ReadMode::Borrow,
                            ..
                        },
                        Inst::Construct { failure, .. },
                    ) => Some((b, k - 3, *value, *failure)),
                    _ => None,
                }
            })
        })
        .expect("a dying builder's construct");
    let released = store
        + code[block][store..]
            .iter()
            .position(|i| matches!(i, Inst::Release { .. }))
            .expect("the slot's release");
    let Inst::Release {
        layout,
        live,
        anchor,
        failure: slot_failure,
        ..
    } = code[block][released].clone()
    else {
        unreachable!()
    };
    let release = |failure| Inst::Release {
        value: builder,
        layout,
        live,
        anchor,
        failure,
    };
    let after = fresh_failure(&mut unit, slot_failure, true);
    code[block].insert(released + 1, release(after));
    code[block].insert(store, Inst::Dup { value: builder });
    // The slot release's failure enters a block releasing `v` before its own cleanup.
    release_first(&mut unit, &mut code, slot_failure, &release);
    // The construct's failure releases `v` after its cleanup releases the slot, as `v`
    // came from an outer temporary (DROP-2's order).
    let cleanup =
        unit.tables.edges[unit.tables.failures[constructed.index()].cleanup.index()].target;
    let failing: Vec<FailureId> = code[cleanup.index()]
        .iter()
        .filter_map(|i| match i {
            Inst::Release { failure, .. } => Some(*failure),
            _ => None,
        })
        .collect();
    for failure in failing {
        release_first(&mut unit, &mut code, failure, &release);
    }
    let Terminator::Jump(exit) = unit.terminators[cleanup.index()] else {
        panic!("the construct's cleanup ends in a jump");
    };
    let next = unit.tables.edges[exit.index()].clone();
    let on = fresh_failure(&mut unit, constructed, false);
    let unwound = unit.tables.failures[on.index()].cleanup;
    let onward = edge_like(&mut unit, &next);
    unit.tables.edges.swap(unwound.index(), onward.index());
    unit.tables.edges.pop();
    let onward = edge_like(&mut unit, &next);
    let after = cleanup_block(&mut unit, &mut code, next.target, release(on), onward);
    unit.tables.edges[exit.index()] = Edge {
        target: after,
        arguments: PoolRange::push(&mut unit.pools.values, []),
    };
    let mut instructions = Vec::new();
    for (b, insts) in code.into_iter().enumerate() {
        unit.blocks[b].instructions = PoolRange::push(&mut instructions, insts);
    }
    unit.instructions = instructions;
    let inputs = hirsel_lir::validate::Inputs {
        discharge: None,
        target: &program.target,
        layouts: &program.table,
        manifest: &program.manifest,
        operations: &program.operations,
    };
    let unit = hirsel_lir::validate::validate(unit, &inputs)
        .unwrap_or_else(|r| panic!("C rejects the shared builder: {:#?}", r.violations));
    program.replace_unit(index, unit).unwrap();
}

/// `failure` enters a new cleanup block that runs `release` (its own failure continuing to
/// the original cleanup), then jumps to the original cleanup.
fn release_first(
    unit: &mut LirUnit<hirsel_lir::stage::Ready>,
    code: &mut Vec<Vec<Inst>>,
    failure: FailureId,
    release: &dyn Fn(FailureId) -> Inst,
) {
    let original = unit.tables.failures[failure.index()].cleanup;
    let target = unit.tables.edges[original.index()].clone();
    let on = fresh_failure(unit, failure, false);
    let onward = edge_like(unit, &target);
    let entered = cleanup_block(unit, code, target.target, release(on), onward);
    unit.tables.edges[original.index()] = Edge {
        target: entered,
        arguments: PoolRange::push(&mut unit.pools.values, []),
    };
}

/// An edge of its own (each is used once) to `like`'s target with its arguments.
fn edge_like(unit: &mut LirUnit<hirsel_lir::stage::Ready>, like: &Edge) -> EdgeId {
    let arguments: Vec<ValueId> =
        unit.pools.values[like.arguments.start as usize..][..like.arguments.len as usize].to_vec();
    unit.tables.edges.push(Edge {
        target: like.target,
        arguments: PoolRange::push(&mut unit.pools.values, arguments),
    });
    EdgeId(unit.tables.edges.len() as u32 - 1)
}

/// A new failure cleanup block holding `inst`, then jumping along `onward`, anchored as
/// `like`.
fn cleanup_block(
    unit: &mut LirUnit<hirsel_lir::stage::Ready>,
    code: &mut Vec<Vec<Inst>>,
    like: BlockId,
    inst: Inst,
    onward: EdgeId,
) -> BlockId {
    let block = BlockId(unit.blocks.len() as u32);
    unit.blocks.push(Block {
        kind: BlockKind::FailureCleanup,
        parameters: PoolRange::push(&mut unit.pools.values, []),
        instructions: PoolRange::push(&mut Vec::new(), []),
        anchor: unit.blocks[like.index()].anchor,
    });
    unit.terminators.push(Terminator::Jump(onward));
    code.push(vec![inst]);
    block
}

/// A failure site of its own (each is used once) with `like`'s cleanup target and
/// arguments, and its disposals when `dispose`.
fn fresh_failure(
    unit: &mut LirUnit<hirsel_lir::stage::Ready>,
    like: FailureId,
    dispose: bool,
) -> FailureId {
    let old = unit.tables.failures[like.index()].clone();
    let edge = unit.tables.edges[old.cleanup.index()].clone();
    let disposals: Vec<ValueId> = if dispose {
        unit.pools.values[old.dispose.start as usize..][..old.dispose.len as usize].to_vec()
    } else {
        Vec::new()
    };
    let cleanup = edge_like(unit, &edge);
    unit.tables.failures.push(FailureEdge {
        cleanup,
        rule: old.rule,
        anchor: old.anchor,
        dispose: PoolRange::push(&mut unit.pools.values, disposals),
    });
    FailureId(unit.tables.failures.len() as u32 - 1)
}

/// The takeover is emitted for both programs (unoptimized, the builder's descriptor is
/// retyped keeping its kind bits on the unique branch; optimized, the shared twin's known
/// count folds to the copy branch alone), and both build the ordinary list's record at O0
/// and O3 under the diagnostic and the production runtime, with nothing left live.
#[test]
fn a_shared_builder_is_copied_into_its_record() {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let tmp = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let tools = Toolchain::in_tree_diagnostic(&tmp.join("native-runtime"), &[&target]).unwrap();
    for shared in [false, true] {
        for opt in [OptLevel::None, OptLevel::Speed] {
            let dir = tmp.join(format!("builder-takeover-{shared}-{opt:?}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let target = target.clone();
            let (tools_ref, dir_ref) = (&tools, &dir);
            let exe = with_compiler_stack(move || {
                let mut program = ready(&target);
                if shared {
                    share_the_builder(&mut program);
                }
                if opt == OptLevel::None {
                    let ir = emit_ir(&program, OptLevel::None).unwrap();
                    let start = ir
                        .lines()
                        .position(|l| l.starts_with("define") && l.contains("5buildI"))
                        .expect("build is defined");
                    let body: Vec<&str> =
                        ir.lines().skip(start).take_while(|l| *l != "}").collect();
                    assert!(
                        body.iter()
                            .any(|l| l.contains(" and i32 ") && l.contains("-536870912")),
                        "{}",
                        body.join("\n")
                    );
                }
                link(
                    &program,
                    &entry_path(&default_context()),
                    opt,
                    Partition::Whole,
                    tools_ref,
                    dir_ref,
                )
                .unwrap()
            });
            for diagnostic in [true, false] {
                let run = launch(
                    &exe,
                    &TargetSpec::x86_64_unknown_linux_gnu(),
                    &tools,
                    &dir,
                    diagnostic,
                )
                .unwrap();
                assert_eq!(
                    (run.exit, String::from_utf8_lossy(&run.stdout).into_owned()),
                    (Some(0), "ok\n".to_owned()),
                    "shared {shared}, {opt:?}, diagnostic {diagnostic}: {run:?}"
                );
                if diagnostic {
                    assert!(run.record.contains("audit 0 0 "), "{}", run.record);
                }
            }
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }
}
