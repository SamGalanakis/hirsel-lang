//! Checked execution as a third witness: every trap
//! rule fires on lowered, hand-edited TIR, and each accepted twin runs in
//! checked mode exactly as it runs normally.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_interp::checked::TrapRule;
use hirsel_interp::{Outcome, library::LaunchError};
use hirsel_syntax::DefPath;
use hirsel_tir::{Definition, DefinitionRef, TirProvider};
use std::collections::BTreeMap;

#[test]
fn task_scope_returns_updated_owned_list() {
    with_compiler_stack(|| {
        let source = r#"
fn values(tasks: Tasks, input: List[u64]) -> List[u64] {
  let initial: List[u64] = [0u64]
  task.scope(tasks, fn(s: Scope) -> List[u64] {
    let mut rows = take initial
    rows[0] = input[0]
    take rows
  })
}
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let rows = values(io.tasks(rt)?, [7u64])
  if rows[0] == 7u64 { let _ = out.print(io.stdout(rt)?, "seven\n") }
  Ok(())
}
"#;
        let mut engine = accepted(source);
        assert_eq!(printed(run(&mut engine, &[], false)), "seven\n");
        assert_eq!(printed(run(&mut engine, &[], true)), "seven\n");
    });
}

/// Serves edited definitions in place of the engine's own.
struct Edited<'a> {
    engine: &'a mut QueryEngine,
    replaced: BTreeMap<DefPath, DefinitionRef>,
}
impl TirProvider for Edited<'_> {
    fn definition(&mut self, path: &DefPath) -> Result<DefinitionRef, hirsel_types::QueryError> {
        match self.replaced.get(path) {
            Some(definition) => Ok(definition.clone()),
            None => self.engine.definition(path),
        }
    }
    fn constant(
        &mut self,
        path: &DefPath,
    ) -> Result<hirsel_tir::MaterializedRef, hirsel_types::QueryError> {
        self.engine.constant(path)
    }
    fn constant_definitions(&mut self) -> Result<Vec<DefPath>, hirsel_types::QueryError> {
        self.engine.constant_definitions()
    }
}

fn accepted(source: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("source.hir", source.into()).unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(
        output
            .diagnostics
            .iter()
            .all(|d| d.severity != hirsel_diag::Severity::Error),
        "{:?}",
        output.diagnostics
    );
    engine
}
fn accepted_with_std(source: &str) -> QueryEngine {
    let mut loaded = load_program_source(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_numeric.hir"),
        Some(source),
    )
    .unwrap();
    loaded
        .check_for_execution()
        .unwrap_or_else(|e| panic!("{source}\n{e:?}"));
    loaded.engine
}
fn path(name: &str) -> DefPath {
    let mut path = entry_path(&default_context());
    path.0.name = name.into();
    path
}
fn lowered(engine: &mut QueryEngine, name: &str) -> Definition {
    (*engine.lower_to_tir(&path(name)).unwrap().value).clone()
}
/// Run `main` with some definitions replaced; `None` when normal execution
/// cannot run it (arenas are M2 outside checked execution).
fn run(engine: &mut QueryEngine, edits: &[Definition], checked: bool) -> Option<(Outcome, String)> {
    let runtime = if checked {
        engine.checked_runtime_types()
    } else {
        engine.runtime_types()
    }
    .unwrap();
    run_with(engine, edits, runtime)
}
fn run_with(
    engine: &mut QueryEngine,
    edits: &[Definition],
    runtime: hirsel_interp::aggregate::RuntimeTypes,
) -> Option<(Outcome, String)> {
    let entry = entry_path(&default_context());
    let signature = engine.item_signature(&entry).unwrap().value;
    let sources = engine.sources.clone();
    let mut provider = Edited {
        engine,
        replaced: edits
            .iter()
            .map(|d| (d.path.path().clone(), DefinitionRef::new(d.clone())))
            .collect(),
    };
    let config = RunConfiguration {
        entry,
        hash_seed: 0,
        inputs: vec![],
    };
    match hirsel_interp::library::launch_with_runtime(
        &mut provider,
        &sources,
        &signature,
        &config,
        SimulationConfig::default(),
        hirsel_interp::Limits::default(),
        &runtime,
    ) {
        Ok(launched) => Some((
            launched.execution.outcome,
            String::from_utf8(launched.execution.stdout).unwrap(),
        )),
        Err(LaunchError::Execution { cause, .. })
            if matches!(*cause, LaunchError::Unavailable(_)) =>
        {
            None
        }
        Err(error) => panic!("{error:?}"),
    }
}
fn trapped(outcome: Option<(Outcome, String)>) -> TrapRule {
    match outcome {
        Some((Outcome::Trapped(trap), _)) => trap.rule,
        other => panic!("expected a trap, got {other:?}"),
    }
}
/// The trap's detail, which names the site that found the violation.
fn trap_detail(outcome: Option<(Outcome, String)>) -> String {
    match outcome {
        Some((Outcome::Trapped(trap), _)) => trap.detail,
        other => panic!("expected a trap, got {other:?}"),
    }
}
fn printed(outcome: Option<(Outcome, String)>) -> String {
    match outcome {
        Some((Outcome::Returned { exit_code: 0, .. }, stdout)) => stdout,
        other => panic!("expected a clean exit, got {other:?}"),
    }
}
/// The IR checker's verdict on an edited definition.
fn validator_rejects(engine: &mut QueryEngine, definition: &Definition) -> bool {
    let input = engine.export_native_input().unwrap();
    let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
    signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
    let mut definition = definition.clone();
    definition.types = hirsel_tir::definition_types(engine.types(), &definition);
    !hirsel_validate::validate(&definition, engine.types(), &signatures, &mut vec![])
        .violations
        .is_empty()
}
/// Declare every `unique` local of `definition` aliased, so an edit that
/// turns a fresh value into a share no longer also claims it unique.
fn without_unique_locals(types: &hirsel_types::TypeInterner, definition: &mut Definition) {
    let mut aliased = |ty: &mut hirsel_types::TypeId| {
        let mut row = types.row(*ty).clone();
        if row.modes.uniqueness == hirsel_types::Uniqueness::Unique {
            row.modes.uniqueness = hirsel_types::Uniqueness::Aliased;
            *ty = types.intern(row);
        }
    };
    for body in &mut definition.bodies {
        body.locals
            .iter_mut()
            .for_each(|local| aliased(&mut local.ty));
        body.places
            .iter_mut()
            .filter(|place| place.projection.is_empty())
            .for_each(|place| aliased(&mut place.ty));
    }
    definition.types = hirsel_tir::definition_types(types, definition);
}
fn named_locals(body: &hirsel_tir::Body) -> Vec<usize> {
    body.locals
        .iter()
        .enumerate()
        .filter(|(_, l)| l.storage == hirsel_tir::Storage::Local)
        .map(|(i, _)| i)
        .collect()
}

/// Review 3 finding 2 (`y1`), rejected at source today (MODE-1). Its accepted
/// twin binds `t` outside the block; moving `t`'s storage into the block
/// rebuilds the reviewer's program as IR.
const LOCAL_RESULT: &str = "fn wrap(x: String) -> List[String] in x { [copy x] }\nfn n(xs: List[String]) -> u64 { list.len(xs) }\nfn probe(x: String) -> u64 { let t = copy x; let slot = { let u = wrap(t); copy u }; n(slot) }\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let s: String = \"abc\"\n  if probe(s) == 1u64 { let _ = out.print(io.stdout(rt)?, \"one\\n\") }\n  Ok(())\n}";

#[test]
fn a_copy_of_a_local_result_traps_after_its_source_scope_exits() {
    with_compiler_stack(|| {
        let mut engine = accepted(LOCAL_RESULT);
        assert_eq!(printed(run(&mut engine, &[], false)), "one\n");
        assert_eq!(printed(run(&mut engine, &[], true)), "one\n");
        let mut probe = lowered(&mut engine, "probe");
        let body = &mut probe.bodies[0];
        let (t, u) = match named_locals(body)[..] {
            [t, u, ..] => (t, u),
            _ => panic!("probe binds t, u and slot"),
        };
        body.locals[t].scope = body.locals[u].scope;
        // The interpreter alone still prints: it never frees a region.
        assert_eq!(printed(run(&mut engine, &[probe.clone()], false)), "one\n");
        assert_eq!(
            trapped(run(&mut engine, &[probe.clone()], true)),
            TrapRule::RegionUseAfterScopeExit
        );
        assert!(validator_rejects(&mut engine, &probe));
    });
}

#[test]
fn a_result_declared_global_cannot_carry_a_borrowed_parameter() {
    with_compiler_stack(|| {
        let mut engine = accepted(LOCAL_RESULT);
        // Forge `wrap`'s declared result as global: its list still holds a part
        // of the borrowed `x`, which ends with the call.
        let mut wrap = lowered(&mut engine, "wrap");
        let result = wrap.bodies[0].result;
        let mut row = engine.types().get(result).unwrap().clone();
        row.modes.relation = hirsel_types::Relation::global();
        wrap.bodies[0].result = engine.types().intern(row);
        assert_eq!(printed(run(&mut engine, &[wrap.clone()], false)), "one\n");
        assert_eq!(
            trapped(run(&mut engine, &[wrap], true)),
            TrapRule::RegionUseAfterScopeExit
        );
    });
}

/// Review 3 finding 1 (`k1`), rejected at source today (PROV-4): the returned
/// closure captures `(x, 1u8)` with `x` in the arena, and runs after it closed.
const ARENA_CLOSURE: &str = "fn mk(a: Arena) -> fn() -> u64 { let x: String = \"xy\"; let p = (x, 1u8); fn() { let (s, n) = p; string.byte_len(s) } }\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let grant_stdout = io.stdout(rt)?\n\n  let f = arena.with(fn(a: Arena) -> fn() -> u64 { mk(a) })\n  if f() == 2u64 { let _ = out.print(grant_stdout, \"two\\n\") }\n  Ok(())\n}";

#[test]
fn a_returned_closure_over_arena_content_traps_after_the_arena_ends() {
    with_compiler_stack(|| {
        let mut engine = accepted(ARENA_CLOSURE);
        assert_eq!(printed(run(&mut engine, &[], false)), "two\n");
        assert_eq!(printed(run(&mut engine, &[], true)), "two\n");
        // Place `x` in `a`, as `let x: String in a` declares.
        let mut mk = lowered(&mut engine, "mk");
        let body = &mut mk.bodies[mk.entry.0 as usize];
        let x = named_locals(body)[0];
        let mut row = engine.types().get(body.locals[x].ty).unwrap().clone();
        row.modes.relation =
            row.modes
                .relation
                .union(&hirsel_types::Relation::place(hirsel_types::Origin {
                    root: hirsel_types::OriginRoot::Parameter(hirsel_types::ParameterRef::Body {
                        scope: body.parameter_scope,
                        parameter: hirsel_types::ParamId(0),
                    }),
                    fields: Default::default(),
                }));
        let placed = engine.types().intern(row);
        body.locals[x].ty = placed;
        for place in body.places.iter_mut() {
            if place.root.0 as usize == x && place.projection.is_empty() {
                place.ty = placed;
            }
        }
        // The interpreter alone still prints: an arena allocates nothing there.
        assert_eq!(printed(run(&mut engine, &[mk.clone()], false)), "two\n");
        assert_eq!(
            trapped(run(&mut engine, &[mk.clone()], true)),
            TrapRule::ArenaUseAfterEnd
        );
        assert!(validator_rejects(&mut engine, &mk));
    });
}

const RESOURCES: &str = "resource type Trace { stdout: Stdout, text: String }\n  drop(x) { let _ = out.print(x.stdout, x.text) }\nfn look(t: Trace) -> u64 { string.byte_len(t.text) }\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let stdout = io.stdout(rt)?\n  let a = Trace(stdout: stdout, text: \"a\\n\")\n  let b = Trace(stdout: stdout, text: \"b\\n\")\n  let n = look(a)\n  Ok(())\n}";

#[test]
fn a_resource_finalised_twice_traps() {
    with_compiler_stack(|| {
        let mut engine = accepted(RESOURCES);
        assert_eq!(printed(run(&mut engine, &[], true)), "b\na\n");
        // `look` drops its borrowed parameter: `a` is finalised twice.
        let mut look = lowered(&mut engine, "look");
        let body = &mut look.bodies[0];
        let parameter = body.parameters[0];
        let place = body
            .places
            .iter()
            .position(|p| p.root == parameter && p.projection.is_empty() && p.loan.is_none())
            .unwrap();
        let actions = body
            .cleanup_actions(hirsel_tir::CleanupId(0))
            .cloned()
            .chain([hirsel_tir::CleanupAction::Drop {
                place: hirsel_tir::PlaceId(place as u32),
                if_live: true,
            }])
            .collect();
        let cleanup = body.add_cleanup(actions);
        for block in body.blocks.iter_mut() {
            if let hirsel_tir::Terminator::Return { cleanup: c, .. } = &mut block.terminator {
                *c = cleanup;
            }
        }
        assert_eq!(
            printed(run(&mut engine, &[look.clone()], false)),
            "a\nb\na\n"
        );
        assert_eq!(
            trapped(run(&mut engine, &[look], true)),
            TrapRule::FinalizerCount
        );
    });
}

/// Rewrite every cleanup list of `main` with `edit`.
fn edit_cleanups(
    main: &mut Definition,
    edit: impl Fn(Vec<hirsel_tir::CleanupAction>) -> Vec<hirsel_tir::CleanupAction>,
) {
    let body = &mut main.bodies[main.entry.0 as usize];
    for id in 0..body.cleanups.len() {
        let id = hirsel_tir::CleanupId(id as u32);
        let actions = body.cleanup_actions(id).cloned().collect();
        body.set_cleanup(id, edit(actions));
    }
}
fn drops_of(main: &Definition, locals: &[usize]) -> Vec<hirsel_tir::PlaceId> {
    let body = &main.bodies[main.entry.0 as usize];
    locals
        .iter()
        .map(|l| {
            hirsel_tir::PlaceId(
                body.places
                    .iter()
                    .position(|p| p.root.0 as usize == *l && p.projection.is_empty())
                    .unwrap() as u32,
            )
        })
        .collect()
}

#[test]
fn resources_finalised_in_declaration_order_trap() {
    with_compiler_stack(|| {
        let mut engine = accepted(RESOURCES);
        let mut main = lowered(&mut engine, "main");
        let body = &main.bodies[main.entry.0 as usize];
        let resources: Vec<usize> = named_locals(body)
            .into_iter()
            .filter(|l| {
                matches!(
                    engine.types().get(body.locals[*l].ty).map(|t| &t.kind),
                    Some(hirsel_types::TypeKind::Named { definition, .. })
                        if definition.path().0.name == "Trace"
                )
            })
            .collect();
        let [a, b] = drops_of(&main, &resources)[..] else {
            panic!("main owns a and b")
        };
        // Swap every `Drop b; Drop a` into `Drop a; Drop b`.
        edit_cleanups(&mut main, |mut actions| {
            let at = |p| {
                actions.iter().position(
                    |x| matches!(x, hirsel_tir::CleanupAction::Drop { place, .. } if *place == p),
                )
            };
            if let (Some(i), Some(j)) = (at(a), at(b)) {
                actions.swap(i, j);
            }
            actions
        });
        assert_eq!(printed(run(&mut engine, &[main.clone()], false)), "a\nb\n");
        assert_eq!(
            trapped(run(&mut engine, &[main], true)),
            TrapRule::FinalizerOrder
        );
    });
}

const NOALLOC: &str = "@noalloc\nfn count(xs: List[u64]) -> u64 { list.len(xs) }\nfn grow(xs: List[u64]) -> u64 { list.len(xs) + 1u64 }\n@noalloc\nfn tally(xs: List[u64]) -> u64 { let ys: List[u64] = [1u64]; list.len(ys) + list.len(xs) }\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let xs: List[u64] = [1u64, 2u64]\n  if count(xs) + tally(xs) == 5u64 { let _ = out.print(io.stdout(rt)?, \"five\\n\") }\n  let g = grow(xs)\n  Ok(())\n}";

#[test]
fn a_heap_block_constructed_in_a_noalloc_body_traps() {
    with_compiler_stack(|| {
        let mut engine = accepted(NOALLOC);
        assert_eq!(printed(run(&mut engine, &[], true)), "five\n");
        let mut tally = lowered(&mut engine, "tally");
        let mut heap = 0;
        for rvalue in tally.bodies[0].rvalues.iter_mut() {
            if let hirsel_tir::Rvalue::Construct { destination, .. } = rvalue {
                *destination = hirsel_types::Destination::Heap;
                heap += 1;
            }
        }
        assert_eq!(heap, 1, "tally constructs one list");
        assert_eq!(printed(run(&mut engine, &[tally.clone()], false)), "five\n");
        assert_eq!(
            trapped(run(&mut engine, &[tally], true)),
            TrapRule::AllocationInNoalloc
        );
    });
}

#[test]
fn an_allocating_call_from_a_noalloc_body_traps() {
    with_compiler_stack(|| {
        let mut engine = accepted(NOALLOC);
        let mut count = lowered(&mut engine, "count");
        let mut calls = 0;
        for block in count.bodies[0].blocks.iter_mut() {
            if let hirsel_tir::Terminator::Call {
                callee:
                    hirsel_tir::Callee::Definition {
                        path: callee,
                        arguments,
                    },
                ..
            } = &mut block.terminator
            {
                *callee = path("grow");
                arguments.clear();
                calls += 1;
            }
        }
        assert_eq!(calls, 1, "count calls list.len once");
        assert_eq!(printed(run(&mut engine, &[count.clone()], false)), "");
        assert_eq!(
            trapped(run(&mut engine, &[count], true)),
            TrapRule::AllocatingCallInNoalloc
        );
    });
}

#[test]
fn a_noalloc_block_that_escapes_its_frame_traps() {
    with_compiler_stack(|| {
        let mut engine = accepted(
            "fn make() -> List[u64] { [1u64] }\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  if list.len(make()) == 1u64 { let _ = out.print(io.stdout(rt)?, \"one\\n\") }\n  Ok(())\n}",
        );
        assert_eq!(printed(run(&mut engine, &[], true)), "one\n");
        // Forge `make` as `@noalloc`: its fresh list can only be region-placed,
        // and that region ends before the caller reads the result.
        let mut runtime = engine.checked_runtime_types().unwrap();
        runtime
            .checked
            .as_mut()
            .unwrap()
            .noalloc
            .insert(path("make"));
        assert_eq!(
            trapped(run_with(&mut engine, &[], runtime)),
            TrapRule::RegionUseAfterScopeExit
        );
    });
}

#[test]
fn a_library_local_result_keeps_the_borrowed_binding_scope() {
    with_compiler_stack(|| {
        let mut engine = accepted(
            "fn probe(x: Bytes) -> bool {\n  let t = copy x\n  let slot = { let u = bytes.slice(t, 0, bytes.len(t)); copy u }\n  slot == b\"bbc\"\n}\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  if probe(b\"bbc\") { let _ = out.print(io.stdout(rt)?, \"yes\\n\") }\n  Ok(())\n}",
        );
        assert_eq!(printed(run(&mut engine, &[], true)), "yes\n");
        let mut probe = lowered(&mut engine, "probe");
        let body = &mut probe.bodies[0];
        let names = named_locals(body);
        body.locals[names[0]].scope = body.locals[names[1]].scope;
        assert_eq!(printed(run(&mut engine, &[probe.clone()], false)), "yes\n");
        assert_eq!(
            trapped(run(&mut engine, &[probe], true)),
            TrapRule::RegionUseAfterScopeExit
        );
    });
}

#[test]
fn callbacks_cannot_return_their_ended_parameter_storage() {
    with_compiler_stack(|| {
        let mut engine = accepted_with_std(
            "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let grant_stdout = io.stdout(rt)?\n\n  let it = iter.from(0u64, fn(mut n: u64) -> Option[String] {\n    if n == 0u64 { n = 1u64; Some(\"item\") } else { None }\n  })\n  let mut mapped = iter.map(take it, fn(x: String) -> List[String] { [\"fresh\"] })\n  let item = iter.next(mut mapped)\n  match item { Some(xs) => { let _ = out.print(grant_stdout, \"one\\n\") }, None => {} }\n  Ok(())\n}",
        );
        assert_eq!(printed(run(&mut engine, &[], true)), "one\n");
        let mut main = lowered(&mut engine, "main");
        let mapper = main
            .bodies
            .iter_mut()
            .find(|b| {
                b.parameters.len() == 1
                    && b.locals.iter().any(|l| {
                        matches!(
                            engine.types().get(l.ty).map(|t| &t.kind),
                            Some(hirsel_types::TypeKind::Builtin {
                                kind: hirsel_types::BuiltinType::List,
                                ..
                            })
                        )
                    })
            })
            .unwrap();
        let parameter = mapper.parameters[0];
        let parameter_place = mapper
            .places
            .iter()
            .position(|p| p.root == parameter && p.projection.is_empty())
            .unwrap();
        let literal = mapper
            .rvalues
            .iter_mut()
            .find(|r| {
                matches!(
                    r,
                    hirsel_tir::Rvalue::Constant(hirsel_tir::Constant::String(_))
                )
            })
            .unwrap();
        *literal = hirsel_tir::Rvalue::Use(hirsel_tir::Operand {
            place: hirsel_tir::PlaceId(parameter_place as u32),
            usage: hirsel_types::UseKind::Copy,
        });
        assert_eq!(printed(run(&mut engine, &[main.clone()], false)), "one\n");
        // The literal's temporary is declared unique; the copy shares it.
        assert_eq!(
            trapped(run(&mut engine, &[main.clone()], true)),
            TrapRule::UniqueShared
        );
        without_unique_locals(engine.types(), &mut main);
        assert_eq!(
            trapped(run(&mut engine, &[main], true)),
            TrapRule::RegionUseAfterScopeExit
        );
    });
}

#[test]
fn deep_unique_extraction_under_noalloc_traps_even_when_discarded() {
    with_compiler_stack(|| {
        let mut engine = accepted(
            "@noalloc fn promise(x: List[u64]) {}
fn clone_it(x: List[u64]) { let y = unique x }
pub fn main(rt: Io, args: Args) -> Result[(), ()] { promise([1u64]); Ok(()) }",
        );
        assert_eq!(printed(run(&mut engine, &[], true)), "");
        // Keep the declared noalloc contract and replace only its TIR body.
        let mut forged = lowered(&mut engine, "clone_it");
        forged.path = hirsel_syntax::DefId::intern(&path("promise"));
        assert_eq!(printed(run(&mut engine, &[forged.clone()], false)), "");
        assert_eq!(
            trapped(run(&mut engine, &[forged], true)),
            TrapRule::AllocationInNoalloc
        );
    });
}

#[test]
fn a_shared_counted_write_under_noalloc_traps() {
    with_compiler_stack(|| {
        let mut engine = accepted(
            "@noalloc fn promise(take xs: List[u64]) -> u64 { 9u64 }\nfn change(take xs: List[u64]) -> u64 { let mut ys = take xs; ys[0u64] = 9u64; ys[0u64] }\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let xs: List[u64] = [1u64]\n  if promise(copy xs) == 9u64 { let _ = out.print(io.stdout(rt)?, \"nine\\n\") }\n  Ok(())\n}",
        );
        assert_eq!(printed(run(&mut engine, &[], true)), "nine\n");
        let mut forged = lowered(&mut engine, "change");
        forged.path = hirsel_syntax::DefId::intern(&path("promise"));
        assert_eq!(
            printed(run(&mut engine, &[forged.clone()], false)),
            "nine\n"
        );
        assert_eq!(
            trapped(run(&mut engine, &[forged], true)),
            TrapRule::AllocationInNoalloc
        );
    });
}

#[test]
fn an_implicit_allocating_destructor_under_noalloc_traps() {
    with_compiler_stack(|| {
        let mut engine = accepted(
            "resource type R { n: u64 } drop(x) {}
@noalloc fn promise() {}
fn dispose() { let r = R(n: 1u64) }
pub fn main(rt: Io, args: Args) -> Result[(), ()] { promise(); Ok(()) }",
        );
        assert_eq!(printed(run(&mut engine, &[], true)), "");
        let mut forged = lowered(&mut engine, "dispose");
        forged.path = hirsel_syntax::DefId::intern(&path("promise"));
        assert_eq!(printed(run(&mut engine, &[forged.clone()], false)), "");
        assert_eq!(
            trapped(run(&mut engine, &[forged], true)),
            TrapRule::AllocatingCallInNoalloc
        );
    });
}

#[test]
fn declaration_order_is_independent_of_storage_live_order() {
    with_compiler_stack(|| {
        let mut engine = accepted(RESOURCES);
        let mut main = lowered(&mut engine, "main");
        let body = &mut main.bodies[0];
        let owners: Vec<_> = named_locals(body).into_iter().filter(|i| {
            matches!(engine.types().get(body.locals[*i].ty).map(|t| &t.kind), Some(hirsel_types::TypeKind::Named { definition, .. }) if definition.path().0.name == "Trace")
        }).collect();
        let [a, b] = owners[..] else {
            panic!("two resources")
        };
        // Reserve b's storage before a's, while declaration and construction
        // order remain a then b. StorageLive is not a declaration event.
        for block in &mut body.blocks {
            block.instructions.retain(|i| !matches!(i.kind, hirsel_tir::InstructionKind::StorageLive(l) if l.0 as usize == b));
            if let Some(at) = block.instructions.iter().position(|i| matches!(i.kind, hirsel_tir::InstructionKind::StorageLive(l) if l.0 as usize == a)) {
                let mut live = block.instructions[at].clone();
                live.kind = hirsel_tir::InstructionKind::StorageLive(hirsel_tir::LocalId(b as u32));
                block.instructions.insert(at, live);
            }
        }
        assert_eq!(printed(run(&mut engine, &[main.clone()], true)), "b\na\n");
        let [a, b] = drops_of(&main, &owners)[..] else {
            unreachable!()
        };
        edit_cleanups(&mut main, |mut actions| {
            let first = actions.iter().position(
                |x| matches!(x, hirsel_tir::CleanupAction::Drop { place, .. } if *place == a),
            );
            let second = actions.iter().position(
                |x| matches!(x, hirsel_tir::CleanupAction::Drop { place, .. } if *place == b),
            );
            if let (Some(i), Some(j)) = (first, second) {
                actions.swap(i, j);
            }
            actions
        });
        assert_eq!(printed(run(&mut engine, &[main.clone()], false)), "a\nb\n");
        assert_eq!(
            trapped(run(&mut engine, &[main], true)),
            TrapRule::FinalizerOrder
        );
    });
}

#[test]
fn nested_scope_order_survives_separate_cleanup_chains_on_failure() {
    with_compiler_stack(|| {
        let mut engine = accepted(
            "resource type R { n: u64 } drop(x) {}
pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let a = R(n: 1u64)
  { let b = R(n: 2u64); let z = 1u64 / 0u64 }
  Ok(())
}",
        );
        assert!(matches!(
            run(&mut engine, &[], true),
            Some((Outcome::Failed(_), _))
        ));
        let mut main = lowered(&mut engine, "main");
        let body = &main.bodies[0];
        let owners: Vec<_> = named_locals(body).into_iter().filter(|i| {
            matches!(engine.types().get(body.locals[*i].ty).map(|t| &t.kind), Some(hirsel_types::TypeKind::Named { definition, .. }) if definition.path().0.name == "R")
        }).collect();
        let [a, b] = drops_of(&main, &owners)[..] else {
            panic!("two resources")
        };
        // Put the outer drop in its own earlier chain; the eventual failure
        // chain still has the inner drop. A per-chain monotonic audit misses it.
        let body = &mut main.bodies[0];
        let cleanup = body.add_cleanup(vec![hirsel_tir::CleanupAction::Drop {
            place: a,
            if_live: true,
        }]);
        let block = body.blocks.iter_mut().find(|block| block.instructions.iter().any(|i| {
            matches!(i.kind, hirsel_tir::InstructionKind::StorageLive(l) if l.0 as usize == owners[1])
        })).unwrap();
        let at = block.instructions.iter().position(|i| matches!(i.kind, hirsel_tir::InstructionKind::Assign { destination, .. } if destination == b)).unwrap();
        let mut instruction = block.instructions[at].clone();
        instruction.kind = hirsel_tir::InstructionKind::RunCleanup(cleanup);
        block.instructions.insert(at + 1, instruction);
        assert!(matches!(
            run(&mut engine, &[main.clone()], false),
            Some((Outcome::Failed(_), _))
        ));
        assert_eq!(
            trapped(run(&mut engine, &[main], true)),
            TrapRule::FinalizerOrder
        );
    });
}

#[test]
fn a_tail_restart_poisons_the_previous_invocations_binding_scopes() {
    with_compiler_stack(|| {
        let mut engine = accepted(
            "fn descend(n: u64, text: String) -> u64 {\n  if n == 0u64 { string.byte_len(text) } else { descend(n-1u64, text) }\n}\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  if descend(3u64, \"abc\") == 3u64 { let _ = out.print(io.stdout(rt)?, \"three\\n\") }\n  Ok(())\n}",
        );
        assert_eq!(printed(run(&mut engine, &[], true)), "three\n");
        let mut descend = lowered(&mut engine, "descend");
        let body = &mut descend.bodies[0];
        assert_eq!(body.tail_sites.len(), 1);
        let block_id = body.tail_sites[0].call_block;
        let parameter = body.parameters[1];
        let place = body
            .places
            .iter()
            .find(|p| p.root == parameter && p.projection.is_empty())
            .unwrap()
            .clone();
        let local = hirsel_tir::LocalId(body.locals.len() as u32);
        body.locals.push(hirsel_tir::Local {
            storage: hirsel_tir::Storage::Local,
            ty: place.ty,
            scope: hirsel_tir::ScopeId(0),
            span: body.blocks[block_id.0 as usize].span,
        });
        let source = hirsel_tir::PlaceId(body.places.len() as u32);
        body.places.push(place.clone());
        let destination = hirsel_tir::PlaceId(body.places.len() as u32);
        body.places.push(hirsel_tir::Place {
            root: local,
            ..place
        });
        let value = hirsel_tir::RvalueId(body.rvalues.len() as u32);
        body.rvalues
            .push(hirsel_tir::Rvalue::Use(hirsel_tir::Operand {
                place: source,
                usage: hirsel_types::UseKind::Copy,
            }));
        let block = &mut body.blocks[block_id.0 as usize];
        block.instructions.push(hirsel_tir::Instruction {
            kind: hirsel_tir::InstructionKind::Assign { destination, value },
            span: block.span,
            on_failure: hirsel_tir::CleanupId(0),
        });
        let hirsel_tir::Terminator::Call { arguments, .. } = &mut block.terminator else {
            unreachable!()
        };
        arguments
            .iter_mut()
            .find(|a| a.parameter.0 == 1)
            .unwrap()
            .value
            .place = destination;
        assert_eq!(
            printed(run(&mut engine, &[descend.clone()], false)),
            "three\n"
        );
        assert_eq!(
            trapped(run(&mut engine, &[descend], true)),
            TrapRule::RegionUseAfterScopeExit
        );
    });
}

#[test]
fn fresh_and_moved_unique_blocks_need_no_detach_even_in_aliased_storage() {
    with_compiler_stack(|| {
        let mut engine = accepted(
            "@noalloc fn fresh() -> u64 { let mut xs: List[u64] = [1u64]; xs[0u64] = 7u64; xs[0u64] }\n@noalloc fn changed(take xs: unique List[u64]) -> u64 { let mut ys = take xs; ys[0u64] = 9u64; ys[0u64] }\n@noalloc fn scalar(x: u64) -> u64 { unique x }\n@noalloc fn nested() { let mut xs: List[List[u64]] = [[1u64]]; xs[0u64][0u64] = 2u64 }\nresource type Safe { n: u64 } @noalloc drop(x) {}\n@noalloc fn dispose(take x: Safe) {}\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let xs: List[u64] = [1u64]\n  nested()\n  dispose(Safe(n: 1u64))\n  if fresh() + changed(take xs) + scalar(1u64) == 17u64 { let _ = out.print(io.stdout(rt)?, \"seventeen\\n\") }\n  Ok(())\n}",
        );
        assert_eq!(printed(run(&mut engine, &[], true)), "seventeen\n");
    });
}

#[test]
fn a_plain_field_of_a_borrowed_capture_keeps_no_lifetime() {
    with_compiler_stack(|| {
        let mut engine = accepted(
            "flat type Item { text: String, n: u64 }
fn dup[T: flat](take w: T) -> T { (fn() { w })() }
pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let original = Item(text: \"flat\", n: 7)
  let copied = dup(take original)
  Ok(())
}",
        );
        assert_eq!(
            printed(run(&mut engine, &[], true)),
            printed(run(&mut engine, &[], false))
        );
    });
}

#[test]
fn nested_callable_content_still_traps_after_its_source_scope_ends() {
    with_compiler_stack(|| {
        let mut engine = accepted_with_std(include_str!(
            "../../../../conformance/CLO/CLO-1-consumed-loop-content-capture-nested-run.hir"
        ));
        assert_eq!(printed(run(&mut engine, &[], false)), "15\n");
        assert_eq!(printed(run(&mut engine, &[], true)), "15\n");
        let mut total = lowered(&mut engine, "total");
        let body = &mut total.bodies[total.entry.0 as usize];
        let v = body
            .rvalues
            .iter()
            .find_map(|r| match r {
                hirsel_tir::Rvalue::Closure { captures, .. } => {
                    Some(body.places[captures[0].place.0 as usize].root.0 as usize)
                }
                _ => None,
            })
            .expect("f captures v");
        let scope = hirsel_tir::ScopeId(body.scopes.len() as u32);
        let mut nested = body.scopes[body.locals[v].scope.0 as usize].clone();
        nested.parent = Some(body.locals[v].scope);
        body.scopes.push(nested);
        body.locals[v].scope = scope;
        let block = body
            .blocks
            .iter_mut()
            .find(|b| {
                matches!(
                    &b.terminator,
                    hirsel_tir::Terminator::Call {
                        callee: hirsel_tir::Callee::Definition { path, .. }, ..
                    } if path.0.name.as_str() == "byte_len"
                )
            })
            .expect("total consumes h result in byte_len");
        // Rebuild a real escape: v lives in an inner scope that ends after
        // h() returns, while the result holder survives until byte_len.
        let mut end = block.instructions.last().unwrap().clone();
        end.kind = hirsel_tir::InstructionKind::EndScope(scope);
        block.instructions.push(end);
        assert_eq!(printed(run(&mut engine, &[total.clone()], false)), "15\n");
        assert_eq!(
            trapped(run(&mut engine, &[total.clone()], true)),
            TrapRule::RegionUseAfterScopeExit
        );
        assert!(validator_rejects(&mut engine, &total));
    });
}

/// The environment itself stays bounded by the region: a closure built in a
/// forged `@noalloc` frame and returned traps when the caller calls it.
#[test]
fn a_region_placed_closure_environment_that_escapes_its_frame_traps() {
    with_compiler_stack(|| {
        let mut engine = accepted(
            "fn make(take text: String) -> fn() -> u64 { fn() { string.byte_len(text) } }\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let f = make(take \"abc\")\n  if f() == 3u64 { let _ = out.print(io.stdout(rt)?, \"three\\n\") }\n  Ok(())\n}",
        );
        assert_eq!(printed(run(&mut engine, &[], true)), "three\n");
        let mut runtime = engine.checked_runtime_types().unwrap();
        runtime
            .checked
            .as_mut()
            .unwrap()
            .noalloc
            .insert(path("make"));
        assert_eq!(
            trapped(run_with(&mut engine, &[], runtime)),
            TrapRule::RegionUseAfterScopeExit
        );
    });
}

#[test]
fn borrowed_protocol_single_yield_composes_the_original_source() {
    let source = r#"
@loop_source(open, next)
type View { source: String }
type Cursor { source: String }
fn view(s: String) -> View in s { View(source: copy s) }
fn open(v: View) -> Cursor in v { Cursor(source: copy v.source) }
fn next(mut c: Cursor) -> Option[String] in c { Some(copy c.source) }
fn first(s: String) -> String in s {
  for piece in view(s) { return copy piece }
  s
}
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let _ = out.print(io.stdout(rt)?, first("a"))
  Ok(())
}
"#;
    let mut engine = accepted(source);
    assert_eq!(printed(run(&mut engine, &[], false)), "a");
    assert_eq!(printed(run(&mut engine, &[], true)), "a");
}

#[test]
fn borrowed_protocol_lifetime_transfer_requires_the_published_receipt() {
    let source = include_str!("../../../../conformance/SEM/SEM-4-contract6-borrowed-run.hir");
    let mut engine = accepted(source);
    let mut first = lowered(&mut engine, "first");
    assert_eq!(printed(run(&mut engine, &[], true)), "a\n");
    first.protocol_associations[0].association.fingerprint = Default::default();
    assert!(validator_rejects(&mut engine, &first));
    assert_eq!(
        trapped(run(&mut engine, &[first], true)),
        TrapRule::RegionUseAfterScopeExit
    );
}

// Independent of std@1/uri: borrowed String -> unique -> constructor -> return.
const UNIQUE_CONSTRUCTOR: &str = r#"
type Wrapped { text: String }
fn wrap(text: String) -> Wrapped { Wrapped(text: unique text) }
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let s: String = "x"
  let w = wrap(s)
  let _ = out.print(io.stdout(rt)?, w.text)
  Ok(())
}
"#;

#[test]
fn a_borrowed_string_copied_into_a_constructor_still_cannot_escape() {
    with_compiler_stack(|| {
        let mut engine = accepted(UNIQUE_CONSTRUCTOR);
        let mut wrap = lowered(&mut engine, "wrap");
        let mut replaced = 0;
        for body in &mut wrap.bodies {
            for rvalue in &mut body.rvalues {
                if let hirsel_tir::Rvalue::Unary {
                    op: hirsel_syntax::UnaryOp::Unique,
                    value,
                } = rvalue
                {
                    // An ordinary counted share retains the own-root loan.
                    *rvalue = hirsel_tir::Rvalue::Use(value.clone());
                    replaced += 1;
                }
            }
        }
        assert_eq!(replaced, 1);
        assert_eq!(printed(run(&mut engine, &[wrap.clone()], false)), "x");
        // `unique text`'s temporary is still declared unique.
        assert_eq!(
            trapped(run(&mut engine, &[wrap.clone()], true)),
            TrapRule::UniqueShared
        );
        assert!(validator_rejects(&mut engine, &wrap));
        without_unique_locals(engine.types(), &mut wrap);
        assert_eq!(
            trapped(run(&mut engine, &[wrap.clone()], true)),
            TrapRule::RegionUseAfterScopeExit
        );
    });
}

/// `keep` holds a copy of `a`, so `unique a` deep-copies before the freeze.
const FREEZE_COPY: &str = "fn freeze(take x: unique List[List[u64]]) -> u64 {\n  let s = shared.freeze(take x)\n  1u64\n}\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let a: unique List[List[u64]] = [[1u64]]\n  let mut keep: List[List[List[u64]]] = []\n  list.push(mut keep, copy a)\n  if freeze(unique a) + list.len(keep) == 2u64 { let _ = out.print(io.stdout(rt)?, \"two\\n\") }\n  Ok(())\n}";

/// The ownership review's m7 with V bypassed: a false uniqueness proof
/// elides `unique a`'s deep copy, so the value claimed unique is the block
/// `keep` still holds. Checked execution traps at each place the IR claims
/// it unique, independently of both checkers: the `unique` temporary in
/// `main`, `freeze`'s `take x: unique` parameter, then `shared.freeze`'s.
#[test]
fn freezing_a_value_another_live_holder_shares_traps() {
    with_compiler_stack(|| {
        let mut engine = accepted(FREEZE_COPY);
        assert_eq!(printed(run(&mut engine, &[], true)), "two\n");
        // A false uniqueness proof elides the deep copy (the ownership
        // review's m6/m7): the frozen block is the one `keep` still holds.
        let mut main = lowered(&mut engine, "main");
        let mut replaced = 0;
        for rvalue in &mut main.bodies[0].rvalues {
            if let hirsel_tir::Rvalue::Unary {
                op: hirsel_syntax::UnaryOp::Unique,
                value,
            } = rvalue
            {
                *rvalue = hirsel_tir::Rvalue::Use(value.clone());
                replaced += 1;
            }
        }
        assert_eq!(replaced, 1);
        assert_eq!(printed(run(&mut engine, &[main.clone()], false)), "two\n");
        assert_eq!(
            trapped(run(&mut engine, &[main.clone()], true)),
            TrapRule::UniqueShared
        );
        assert!(validator_rejects(&mut engine, &main));
        let site = |engine: &mut QueryEngine, edits: &[Definition]| {
            let detail = trap_detail(run(engine, edits, true));
            detail
                .split(" is declared unique")
                .next()
                .unwrap()
                .to_string()
        };
        let local = site(&mut engine, &[main.clone()]);
        assert!(local.starts_with("local "), "{local}");
        without_unique_locals(engine.types(), &mut main);
        assert_eq!(site(&mut engine, &[main.clone()]), "parameter 0");
        let mut freeze = lowered(&mut engine, "freeze");
        without_unique_locals(engine.types(), &mut freeze);
        assert_eq!(site(&mut engine, &[main, freeze]), "freeze parameter 0");
    });
}

/// A task takes `sent`, a deep copy of `a`, while its parent keeps `keep`, a
/// counted copy of `a`, and reads it after the task ends.
const TASK_COPY: &str = "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let tasks = io.tasks(rt)?\n  let a: List[List[u64]] = [[1u64]]\n  let keep: List[List[u64]] = copy a\n  let sent: unique List[List[u64]] = unique a\n  let n = task.scope(\n    tasks,\n    fn(s: Scope) -> u64 {\n      let t = task.async(s, fn() -> u64 {\n        let mine = take sent\n        list.len(mine)\n      })\n      task.await(s, take t)\n    }\n  )\n  if n + list.len(keep) == 2u64 { let _ = out.print(io.stdout(rt)?, \"two\\n\") }\n  Ok(())\n}";

/// OWN-6 with both checkers bypassed: a false uniqueness proof elides
/// `unique a`'s deep copy, so the block the task takes, and so owns, is one
/// its parent still counts. With the IR's `unique` claims removed too,
/// checked execution still traps when the parent reads the task's block.
#[test]
fn two_tasks_counting_one_block_trap() {
    with_compiler_stack(|| {
        let mut engine = accepted(TASK_COPY);
        assert_eq!(printed(run(&mut engine, &[], true)), "two\n");
        let mut main = lowered(&mut engine, "main");
        let mut replaced = 0;
        for body in &mut main.bodies {
            for rvalue in &mut body.rvalues {
                if let hirsel_tir::Rvalue::Unary {
                    op: hirsel_syntax::UnaryOp::Unique,
                    value,
                } = rvalue
                {
                    *rvalue = hirsel_tir::Rvalue::Use(value.clone());
                    replaced += 1;
                }
            }
        }
        assert_eq!(replaced, 1);
        assert_eq!(printed(run(&mut engine, &[main.clone()], false)), "two\n");
        assert_eq!(
            trapped(run(&mut engine, &[main.clone()], true)),
            TrapRule::UniqueShared
        );
        assert!(validator_rejects(&mut engine, &main));
        without_unique_locals(engine.types(), &mut main);
        assert_eq!(
            trapped(run(&mut engine, &[main], true)),
            TrapRule::TaskShared
        );
    });
}

/// `xs = take fresh` displaces content that `kept`, a copy of `xs`, still
/// holds (MODE-2-release-failure-block-end-run).
const DISPLACED: &str = "fn grow(mut xs: unique List[u64]) -> u64 uses {} {\n  let fresh: unique List[u64] = [1u64]\n  let kept = copy xs\n  xs = take fresh\n  list.len(kept)\n}\n\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let mut xs: unique List[u64] = [0u64]\n  if grow(mut xs) == 1u64 { let _ = out.print(io.stdout(rt)?, \"one\\n\") }\n  Ok(())\n}";

/// The old content's uniqueness is flow knowledge, not the place's
/// declaration, so a Replace's displaced temporary is aliased. Giving it the
/// place's declared `unique` row back is the false claim lowering once made,
/// and checked execution traps on it.
#[test]
fn a_displaced_value_claims_no_uniqueness() {
    with_compiler_stack(|| {
        let mut engine = accepted(DISPLACED);
        assert_eq!(printed(run(&mut engine, &[], true)), "one\n");
        let mut grow = lowered(&mut engine, "grow");
        let body = &grow.bodies[grow.entry.0 as usize];
        let displaced: Vec<_> = body
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .filter_map(|instruction| match instruction.kind {
                hirsel_tir::InstructionKind::Replace { displaced, .. } => {
                    Some(body.places[displaced.0 as usize].root)
                }
                _ => None,
            })
            .collect();
        assert_eq!(displaced.len(), 1);
        let local = displaced[0].0 as usize;
        let mut row = engine.types().get(body.locals[local].ty).unwrap().clone();
        assert_eq!(row.modes.uniqueness, hirsel_types::Uniqueness::Aliased);
        row.modes.uniqueness = hirsel_types::Uniqueness::Unique;
        let unique = engine.types().intern(row);
        let entry = grow.entry.0 as usize;
        grow.bodies[entry].locals[local].ty = unique;
        for place in &mut grow.bodies[entry].places {
            if place.root.0 as usize == local && place.projection.is_empty() {
                place.ty = unique;
            }
        }
        assert_eq!(
            trapped(run(&mut engine, &[grow], true)),
            TrapRule::UniqueShared
        );
    });
}

#[test]
fn nested_list_builder_aliases_trap_even_with_forged_noalloc_body() {
    with_compiler_stack(|| {
        for edits in [
            "list.push(mut rows, copy shared)",
            "list.push(mut rows, [2u64]); list.push(mut rows, [3u64]); list.swap(mut rows, 0, 1); list.truncate(mut rows, 0); list.push(mut rows, copy shared)",
            "list.push(mut rows, [2u64]); let _ = list.remove(mut rows, 0); list.push(mut rows, copy shared)",
            "list.push(mut rows, [2u64]); let _ = list.pop(mut rows); list.push(mut rows, copy shared)",
            "list.push(mut rows, [2u64]); list.truncate(mut rows, 0); list.push(mut rows, copy shared)",
        ] {
            let source = format!(
                "@noalloc fn promise(mut rows: List[List[u64]]) {{}}\n\
                 fn change(mut rows: List[List[u64]]) {{ rows[0][0] = 9u64 }}\n\
                 pub fn main() -> Result[(), ()] {{\n\
                   let shared = [1u64]\n\
                   let mut rows: List[List[u64]] = []\n\
                   {edits}\n\
                   promise(mut rows)\n\
                   if shared[0] != 1u64 {{ return Error(()) }}\n\
                   Ok(())\n\
                 }}"
            );
            let mut engine = accepted(&source);
            assert_eq!(printed(run(&mut engine, &[], true)), "");
            let mut forged = lowered(&mut engine, "change");
            forged.path = hirsel_syntax::DefId::intern(&path("promise"));
            assert_eq!(printed(run(&mut engine, &[forged.clone()], false)), "");
            assert_eq!(
                trapped(run(&mut engine, &[forged], true)),
                TrapRule::AllocationInNoalloc,
                "{edits}"
            );
        }
    });
}

#[test]
fn dropped_temporary_views_release_their_checked_holders() {
    with_compiler_stack(|| {
        let source = r#"
type View { values: List[u64] }
fn view(values: List[u64]) -> View in values { View(values: copy values) }
fn count(v: View) -> u64 { list.len(v.values) }
fn consume(take values: unique List[u64]) -> u64 { list.len(values) }
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let values: unique List[u64] = [7u64]
  let _ = count(view(values))
  let n = consume(take values)
  if n == 1u64 { let _ = out.print(io.stdout(rt)?, "one\n") }
  Ok(())
}
"#;
        let mut engine = accepted(source);
        assert_eq!(printed(run(&mut engine, &[], false)), "one\n");
        assert_eq!(printed(run(&mut engine, &[], true)), "one\n");
    });
}

#[test]
fn dropped_borrowed_resources_finish_before_unique_consumption() {
    with_compiler_stack(|| {
        let source = r#"
resource type Watch { values: List[u64], stdout: Stdout }
drop(w) {
  failure.check(list.len(w.values) == 1u64, TextInvariant())
  let _ = out.print(w.stdout, "released\n")
}
fn watch(values: List[u64], stdout: Stdout) -> Watch in values {
  Watch(values: copy values, stdout: stdout)
}
fn consume(take values: unique List[u64]) -> u64 { list.len(values) }
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let stdout = io.stdout(rt)?
  let values: unique List[u64] = [7u64]
  let _ = watch(values, stdout)
  let n = consume(take values)
  if n == 1u64 { let _ = out.print(stdout, "one\n") }
  Ok(())
}
"#;
        let mut engine = accepted(source);
        assert_eq!(printed(run(&mut engine, &[], false)), "released\none\n");
        assert_eq!(printed(run(&mut engine, &[], true)), "released\none\n");
    });
}
#[test]
fn a_forged_private_unique_result_still_traps_with_both_checkers_bypassed() {
    with_compiler_stack(|| {
        let source = r#"
fn counted(input: List[u64]) -> List[u64] in input { copy input }
fn fresh(input: List[u64]) -> List[u64] { [1] }
fn need(take values: unique List[u64]) {}
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
    let original = [2u64]
    need(fresh(original))
    let _ = list.len(original)
    Ok(())
}
"#;
        let mut engine = accepted(source);
        assert_eq!(printed(run(&mut engine, &[], true)), "");
        let mut fresh = lowered(&mut engine, "fresh");
        assert!(
            fresh
                .inferred_unique
                .results
                .contains(&hirsel_types::UniqueResultRow::Root)
        );
        // Reuse a lowered producer that shares its borrowed input, keeping
        // the proved header and the caller's unique parameter. The caller
        // retains the other owner after this producer returns.
        let counted = lowered(&mut engine, "counted");
        fresh.bodies = counted.bodies;
        without_unique_locals(engine.types(), &mut fresh);
        assert!(validator_rejects(&mut engine, &fresh));
        assert_eq!(
            trapped(run(&mut engine, &[fresh], true)),
            TrapRule::UniqueShared
        );
    });
}
