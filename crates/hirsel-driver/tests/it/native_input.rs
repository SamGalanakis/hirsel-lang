use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_types::SignatureKind;
use std::sync::Arc;
fn engine(text: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("snapshot.hir", text.into()).unwrap();
    let result = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    engine
}
const PROGRAM: &str = "pub type E = Error()\npub opaque type Hidden: () uses {} { value: u64 }\nconst answer: u64 = 42u64\npub fn main() -> Result[(), E] { let n = answer; Ok(()) }\n";
#[test]
fn native_snapshot_owns_declarations_materializations_sources_and_program_selection() {
    let mut engine = engine(PROGRAM);
    let input = engine.export_native_input().unwrap();
    assert_eq!(input.completed_source_stages, NativeStatus::Complete);
    assert!(input.instances.edges.is_empty());
    assert_eq!(input.audits.len(), input.definitions.len());
    assert!(input.signatures.len() > input.definitions.len());
    let mut constant = entry_path(&default_context());
    constant.0.name = "answer".into();
    assert!(input.constants.contains_key(&constant));
    assert_eq!(input.constants.len(), input.constant_definitions.len());
    let validated = engine.import_native_input(input).unwrap();
    drop(engine);
    let frozen = validated.snapshot();
    assert!(
        frozen
            .sources
            .file(
                frozen.source_owners[&entry_path(&default_context())]
                    .syntax
                    .file
            )
            .is_some()
    );
    assert!(
        frozen
            .definitions
            .contains_key(&entry_path(&default_context()))
    );
}
#[test]
fn native_import_rejects_forged_tir_declarations_flags_and_selection() {
    let mut engine = engine(PROGRAM);
    let base = engine.export_native_input().unwrap();
    let path = entry_path(&default_context());
    let mut bad = base.clone();
    bad.constant_definitions.reverse();
    assert!(engine.import_native_input(bad).is_err());
    let mut bad = base.clone();
    bad.completed_source_stages = NativeStatus::Unchecked;
    assert!(engine.import_native_input(bad).is_err());
    let mut bad = base.clone();
    bad.operations.pop_first();
    assert!(engine.import_native_input(bad).is_err());
    let mut bad = base.clone();
    bad.registry_revision = Default::default();
    assert!(engine.import_native_input(bad).is_err());
    let mut bad = base.clone();
    bad.definitions.remove(&path);
    assert!(engine.import_native_input(bad).is_err());
    let mut bad = base.clone();
    Arc::make_mut(bad.definitions.get_mut(&path).unwrap()).bodies[0]
        .entry
        .0 = 999;
    assert!(engine.import_native_input(bad).is_err());
    let mut bad = base.clone();
    let declaration = Arc::make_mut(bad.signatures.get_mut(&path).unwrap());
    if let SignatureKind::Function(f) = &mut declaration.kind {
        f.incomplete = true;
    }
    assert!(engine.import_native_input(bad).is_err());
    let mut bad = base.clone();
    bad.constants.values_mut().next().unwrap().root.kind =
        hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Unit);
    assert!(engine.import_native_input(bad).is_err());
    let mut bad = base.clone();
    let private = bad
        .representations
        .values_mut()
        .find_map(Option::as_mut)
        .unwrap();
    private.representation_revision = Default::default();
    assert!(engine.import_native_input(bad).is_err());
    let mut bad = base;
    Arc::make_mut(bad.definitions.get_mut(&path).unwrap()).signature_hash = Default::default();
    assert!(engine.import_native_input(bad).is_err());
}
#[test]
fn same_signature_body_edits_make_an_old_native_snapshot_stale() {
    let mut engine = engine(PROGRAM);
    let old = engine.export_native_input().unwrap();
    let changed = PROGRAM.replace("let n = answer;", "let n = answer + 1u64;");
    let file = engine.sources.add("snapshot.hir", changed).unwrap();
    check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(engine.import_native_input(old).is_err());
    let new = engine.export_native_input().unwrap();
    engine.import_native_input(new).unwrap();
}

#[test]
fn native_snapshots_retain_match_tables_and_reject_forged_patterns() {
    let mut engine = engine(
        "pub fn main(rt:Io,args:Args)->Result[(),()] { let p:(Option[bool],bool)=(Some(true),false); let x = match p { (Some(true),_)=>1u8,(Some(false),_)=>2u8,(None,_)=>3u8 }; Ok(()) }",
    );
    let input = engine.export_native_input().unwrap();
    let path = entry_path(&default_context());
    let definition = &input.definitions[&path];
    assert_eq!(definition.bodies[0].matches.len(), 1);
    assert_eq!(definition.bodies[0].matches[0].arms.len(), 3);
    let validated = engine.import_native_input(input.clone()).unwrap();
    assert_eq!(
        validated.snapshot().definitions[&path].bodies[0].matches,
        definition.bodies[0].matches
    );
    let mut bad = input;
    let table = &mut Arc::make_mut(bad.definitions.get_mut(&path).unwrap()).bodies[0].matches[0];
    table.patterns[table.arms[0].pattern.0 as usize].place.0 = u32::MAX;
    assert!(engine.import_native_input(bad).is_err());
}

#[test]
fn native_snapshots_transport_materialized_keyed_collections() {
    use hirsel_tir::Constructor;
    use hirsel_tir::ValueKind;
    let mut engine = engine(
        "fn build()->Map[u64,u64]{let mut m:Map[u64,u64]=map.empty();map.insert(mut m,2,20);map.insert(mut m,1,10);take m}\nconst M:Map[u64,u64]=build()\nconst S:Set[u64]=set.empty()\npub fn main()->Result[(),()]{let m=copy M;let s=copy S;Ok(())}\n",
    );
    let input = engine.export_native_input().unwrap();
    for (name, constructor, size) in [("M", Constructor::Map, 2), ("S", Constructor::Set, 0)] {
        let mut path = entry_path(&default_context());
        path.0.name = name.into();
        let root = &input.constants[&path].root;
        let ValueKind::Construct {
            constructor: actual,
            fields,
        } = &root.kind
        else {
            panic!("expected materialized collection")
        };
        assert_eq!(actual, &constructor);
        assert_eq!(fields.len(), size);
    }
    engine.import_native_input(input.clone()).unwrap();
    let mut bad = input;
    let mut path = entry_path(&default_context());
    path.0.name = "M".into();
    let ValueKind::Construct { fields, .. } = &mut bad.constants.get_mut(&path).unwrap().root.kind
    else {
        unreachable!()
    };
    let ValueKind::Construct { fields: entry, .. } = &mut fields[1].kind else {
        unreachable!()
    };
    entry[0].kind = ValueKind::Constant(hirsel_tir::Constant::Integer(2));
    assert!(engine.import_native_input(bad).is_err());
}

#[test]
fn independent_validation_compares_nested_set_keys_without_using_insertion_order() {
    use hirsel_tir::{Constant, Rvalue};
    let mut engine = engine(
        "fn key(last:u64)->Set[u64]{let mut s:Set[u64]=set.empty();set.insert(mut s,1);set.insert(mut s,last);take s}\nfn build()->Map[Set[u64],u64]{let mut m:Map[Set[u64],u64]=map.empty();map.insert(mut m,key(2),10);map.insert(mut m,key(3),20);take m}\nconst M:Map[Set[u64],u64]=build()\npub fn main()->Result[(),()]{let m=comptime build();Ok(())}\n",
    );
    let input = engine.export_native_input().unwrap();
    let signatures = input
        .signatures
        .iter()
        .map(|(p, s)| (p.clone(), (**s).clone()))
        .collect();
    let mut path = entry_path(&default_context());
    path.0.name = "M".into();
    let output = engine
        .lower_to_tir(&entry_path(&default_context()))
        .unwrap()
        .value;
    hirsel_validate::assert_valid(&output, &signatures);
    let mut forged = (*output).clone();
    let mut changed = 0;
    for body in &mut forged.bodies {
        for value in &mut body.rvalues {
            if *value == Rvalue::Constant(Constant::Integer(3)) {
                *value = Rvalue::Constant(Constant::Integer(2));
                changed += 1;
            }
        }
    }
    assert_eq!(changed, 1);
    let report = hirsel_validate::validate(&forged, &signatures);
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.kind == hirsel_validate::ViolationKind::InvalidConstructor),
        "{report:?}"
    );
}

#[test]
fn native_snapshots_retain_conditional_exchange_and_reject_a_forged_flag() {
    let mut engine = engine(
        "fn probe(flag:bool){let mut r:String=\"old\";if flag{let moved=take r};r=\"new\"}\npub fn main()->Result[(),()]{probe(false);Ok(())}",
    );
    let input = engine.export_native_input().unwrap();
    let mut path = entry_path(&default_context());
    path.0.name = "probe".into();
    assert!(
        input.definitions[&path].bodies[0]
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .any(|i| matches!(
                i.kind,
                hirsel_tir::InstructionKind::Replace { if_live: true, .. }
            ))
    );
    engine.import_native_input(input.clone()).unwrap();
    let mut forged = input;
    let body = &mut Arc::make_mut(forged.definitions.get_mut(&path).unwrap()).bodies[0];
    for instruction in body.blocks.iter_mut().flat_map(|b| &mut b.instructions) {
        if let hirsel_tir::InstructionKind::Replace { if_live, .. } = &mut instruction.kind {
            *if_live = false;
        }
    }
    assert!(engine.import_native_input(forged).is_err());
}

#[test]
fn native_snapshots_preserve_conditional_loop_loans() {
    let mut engine = engine(
        "fn view(x:Bytes)->Bytes in x{copy x}\nfn probe(x:Bytes,y:Bytes,c:bool)->Bytes in {x, y}{let mut slot=view(x);let mut more=c;while more{slot=view(y);more=false};slot}\npub fn main()->Result[(),()]{Ok(())}",
    );
    let snapshot = engine.export_native_input().unwrap();
    engine.import_native_input(snapshot.clone()).unwrap();
    let mut path = entry_path(&default_context());
    path.0.name = "probe".into();
    // The loop's read loan may still be active from the previous iteration, so
    // it ends conditionally before it begins again (LANG-3).
    for mutable in [false, true] {
        let mut forged = snapshot.clone();
        let definition = Arc::make_mut(forged.definitions.get_mut(&path).unwrap());
        let instructions: Vec<_> = definition
            .bodies
            .iter_mut()
            .flat_map(|body| &mut body.blocks)
            .flat_map(|block| &mut block.instructions)
            .collect();
        let conditional = instructions
            .iter()
            .position(|instruction| {
                matches!(
                    instruction.kind,
                    hirsel_tir::InstructionKind::EndLoan {
                        if_active: true,
                        ..
                    }
                )
            })
            .expect("loop's conditional read end");
        let hirsel_tir::InstructionKind::EndLoan { loan, .. } = instructions[conditional].kind
        else {
            unreachable!()
        };
        let index = if mutable {
            instructions
                .iter()
                .position(|instruction| {
                    matches!(instruction.kind, hirsel_tir::InstructionKind::BeginLoan { id, .. } if id == loan)
                })
                .expect("loop's read begin")
        } else {
            conditional
        };
        let mut instructions = instructions;
        match &mut instructions[index].kind {
            hirsel_tir::InstructionKind::BeginLoan { mutable: mode, .. } => *mode = true,
            hirsel_tir::InstructionKind::EndLoan { if_active, .. } => *if_active = false,
            _ => unreachable!(),
        }
        assert!(engine.import_native_input(forged).is_err());
    }
}

const AUDITED: &str = "resource type Token { n: u32 }\n@noalloc drop(x) { () }\n@noalloc\nfn maybe() -> () { () }\nfn discard(take s: String) -> () { () }\nfn probe() -> () {\n  let f: @noalloc fn() -> () uses {} = fn() { maybe() }\n  f()\n}\npub fn main() -> Result[(), ()] { probe(); Ok(()) }\n";
fn named(name: &str) -> hirsel_engine::DefPath {
    let mut path = entry_path(&default_context());
    path.0.name = name.into();
    path
}
#[test]
fn native_snapshots_record_each_completed_source_audit_with_its_promises() {
    use hirsel_types::{Allocation, DropAllocation, ParamId};
    let mut engine = engine(AUDITED);
    let input = engine.export_native_input().unwrap();
    assert!(input.audits.keys().eq(input.definitions.keys()));
    for (path, audit) in &input.audits {
        assert_eq!(&audit.definition, path);
        assert_eq!(audit.signature, input.signatures[path].fingerprint);
        assert_eq!(
            audit.implementation_revision,
            input.source_owners[path].implementation_revision
        );
    }
    let token = &input.audits[&named("Token")];
    assert!(token.noalloc);
    assert_eq!(
        token.cleanup,
        [(CleanupSite::Fields, DropAllocation::NoAlloc)]
    );
    assert!(input.audits[&named("maybe")].noalloc);
    assert!(!input.audits[&named("probe")].noalloc);
    let discard = &input.audits[&named("discard")];
    assert_eq!(
        discard
            .cleanup
            .iter()
            .map(|(site, _)| site)
            .collect::<Vec<_>>(),
        [&CleanupSite::Parameter(ParamId(0)), &CleanupSite::Result]
    );
    let probe = &input.audits[&named("probe")];
    assert_eq!(probe.closures.len(), 1);
    assert_eq!(probe.closures[0].1, Allocation::NoAlloc);
    // The closure body's call and the call through the closure value.
    assert_eq!(
        probe
            .invocations
            .iter()
            .filter(|(_, allocation)| *allocation == Allocation::NoAlloc)
            .count(),
        2
    );
    engine.import_native_input(input).unwrap();
}
#[test]
fn native_import_rejects_forged_and_missing_source_audit_records() {
    use hirsel_types::{Allocation, DropAllocation};
    let mut engine = engine(AUDITED);
    let base = engine.export_native_input().unwrap();
    let forgeries: Vec<(&str, fn(&mut SourceAudit))> = vec![
        ("maybe", |a| a.noalloc = false),
        ("probe", |a| a.noalloc = true),
        ("Token", |a| a.noalloc = false),
        ("probe", |a| a.closures[0].1 = Allocation::MayAllocate),
        ("probe", |a| {
            a.closures.clear();
        }),
        ("probe", |a| {
            a.invocations.pop();
        }),
        ("probe", |a| a.invocations[0].1 = Allocation::MayAllocate),
        ("discard", |a| a.cleanup[0].1 = DropAllocation::MayAllocate),
        ("Token", |a| a.cleanup.clear()),
        ("main", |a| a.signature = Default::default()),
        ("main", |a| a.implementation_revision = Default::default()),
        ("main", |a| a.definition = named("probe")),
    ];
    for (name, forge) in forgeries {
        let mut bad = base.clone();
        let audit = bad.audits.get_mut(&named(name)).unwrap();
        let before = audit.clone();
        forge(audit);
        assert_ne!(audit, &before, "{name}");
        let error = engine.import_native_input(bad).err().expect(name);
        assert!(format!("{error:?}").contains("source audit"), "{error:?}");
    }
    let mut bad = base.clone();
    bad.audits.remove(&named("maybe"));
    assert!(engine.import_native_input(bad).is_err());
    let mut bad = base.clone();
    let extra = bad.audits[&named("maybe")].clone();
    bad.audits.insert(named("Hidden"), extra);
    assert!(engine.import_native_input(bad).is_err());
    engine.import_native_input(base).unwrap();
}

// R10/R11 native regression controls. Each runs in normal and checked
// execution, which must agree, and each rejected or failing program has an
// accepted twin. Observations are what native engines must reproduce.
const TRACE: &str = "resource type Trace { stdout: Stdout, text: String }\ndrop(x) { let _ = out.print(x.stdout, x.text) }\n";
struct Observed {
    outcome: RunOutcome,
    stdout: String,
    observation: hirsel_interp::observation::Observable,
}
fn observe(body: &str) -> Observed {
    let source = format!("{TRACE}{body}");
    let mut engine = engine(&source);
    let config = RunConfiguration {
        entry: entry_path(&default_context()),
        hash_seed: 0,
        inputs: vec![],
    };
    let mut runs = [false, true].map(|checked| {
        let run = if checked {
            run_observed_checked(&mut engine, &config, SimulationConfig::default())
        } else {
            run_observed_snapshot(&mut engine, &config, SimulationConfig::default())
        }
        .unwrap();
        let observation = run.observation.expect("language observation");
        Observed {
            outcome: run.outcome.unwrap(),
            stdout: String::from_utf8(observation.output.clone()).unwrap(),
            observation,
        }
    });
    let [normal, checked] = &mut runs;
    assert_eq!(normal.outcome, checked.outcome, "{source}");
    assert!(
        normal.observation.equivalent(&checked.observation),
        "{source}"
    );
    let [normal, _] = runs;
    normal
}
fn failure(rule: &str, line: usize) -> RunOutcome {
    RunOutcome::Failure {
        rule: hirsel_diag::RuleId::new(rule).unwrap(),
        site: hirsel_engine::FailureSite {
            line,
            ..Default::default()
        },
    }
}
fn assert_run(observed: &Observed, outcome: RunOutcome, stdout: &str) {
    match (&observed.outcome, &outcome) {
        (
            RunOutcome::Failure { rule, site },
            RunOutcome::Failure {
                rule: expected,
                site: expected_site,
            },
        ) => {
            assert_eq!(rule, expected);
            assert_eq!(site.line, expected_site.line);
            assert!(!site.definition.is_empty());
            assert!(!site.file.as_os_str().is_empty());
            assert!(site.column > 0);
        }
        _ => assert_eq!(observed.outcome, outcome),
    }
    assert_eq!(observed.stdout, stdout);
}

#[test]
fn mem14_through_a_nested_projection_fails_before_extraction_or_write() {
    hirsel_engine::with_compiler_stack(
        mem14_through_a_nested_projection_fails_before_extraction_or_write_on_compiler_thread,
    );
}
fn mem14_through_a_nested_projection_fails_before_extraction_or_write_on_compiler_thread() {
    let replace = |i| {
        format!(
            "type Holder {{ items: List[Trace] }}\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] {{\n  let stdout = io.stdout(rt)?\n  let mut h = Holder(items: [Trace(stdout: stdout, text: \"old\\n\")])\n  let i: u64 = {i}\n  h.items[i] = Trace(stdout: stdout, text: \"new\\n\")\n  Ok(())\n}}\n"
        )
    };
    // The resource right-hand side is released, and the old element was neither
    // extracted nor overwritten.
    assert_run(&observe(&replace(7)), failure("MEM-14", 8), "new\nold\n");
    assert_run(&observe(&replace(0)), RunOutcome::Exit(0), "old\nnew\n");
    let read = |i| {
        format!(
            "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {{\n  let stdout = io.stdout(rt)?\n  let xs: List[List[Trace]] = [[Trace(stdout: stdout, text: \"kept\\n\")]]\n  let i: u64 = {i}\n  let n = copy xs[0][i].text\n  Ok(())\n}}\n"
        )
    };
    let failed = observe(&read(3));
    assert_run(&failed, failure("MEM-14", 7), "kept\n");
    assert_eq!(failed.observation.failures.len(), 1);
    assert_run(&observe(&read(0)), RunOutcome::Exit(0), "kept\n");
}

#[test]
fn mem14_through_a_callback_mut_argument_fails_before_the_callback_runs() {
    hirsel_engine::with_compiler_stack(
        mem14_through_a_callback_mut_argument_fails_before_the_callback_runs_on_compiler_thread,
    );
}
fn mem14_through_a_callback_mut_argument_fails_before_the_callback_runs_on_compiler_thread() {
    let closure = |i| {
        format!(
            "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {{\n  let stdout = io.stdout(rt)?\n  let mut xs: List[Trace] = [Trace(stdout: stdout, text: \"old\\n\")]\n  let i: u64 = {i}\n  let bump = fn(mut t: Trace) {{ t.text = \"new\\n\" }}\n  bump(mut xs[i])\n  Ok(())\n}}\n"
        )
    };
    assert_run(&observe(&closure(7)), failure("MEM-14", 8), "old\n");
    assert_run(&observe(&closure(0)), RunOutcome::Exit(0), "new\n");
    let parameter = |i| {
        format!(
            "fn apply(f: fn(mut t: Trace) -> () uses {{}}, mut xs: List[Trace], i: u64) {{\n  f(mut xs[i])\n}}\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] {{\n  let stdout = io.stdout(rt)?\n  let mut xs: List[Trace] = [Trace(stdout: stdout, text: \"old\\n\")]\n  apply(fn(mut t: Trace) {{ t.text = \"new\\n\" }}, mut xs, {i})\n  Ok(())\n}}\n"
        )
    };
    let failed = observe(&parameter(7));
    assert_run(&failed, failure("MEM-14", 4), "old\n");
    assert_eq!(
        failed.observation.failures[0].stable_site.definition.0.name,
        "apply"
    );
    assert_run(&observe(&parameter(0)), RunOutcome::Exit(0), "new\n");
}

const FAILS: &str = "let z: u8 = 0\n  let q = 1u8 / z\n";
/// Pads a line end with a scoped temporary that needs cleanup, renumbering
/// later bodies, blocks, locals and cleanups without moving any source site.
fn padded(body: &str) -> String {
    body.replacen(
        "let stdout = io.stdout(rt)?\n",
        "let stdout = io.stdout(rt)?; let pad = { let inner: String = \"pad\"; 0u8 }\n",
        1,
    )
}
fn roles(observed: &Observed) -> Vec<(hirsel_interp::observation::SemanticRole, u32)> {
    observed
        .observation
        .effects
        .iter()
        .map(|effect| (effect.anchor.role.clone(), effect.anchor.source.line))
        .collect()
}
/// The observation is the same after transient arenas are renumbered.
fn observe_stably(body: &str) -> Observed {
    let observed = observe(body);
    let renumbered = observe(&padded(body));
    assert_ne!(padded(body), body);
    assert!(
        observed.observation.equivalent(&renumbered.observation),
        "{body}"
    );
    observed
}

#[test]
fn a_return_value_cleanup_failure_releases_the_prepared_result_once() {
    hirsel_engine::with_compiler_stack(
        a_return_value_cleanup_failure_releases_the_prepared_result_once_on_compiler_thread,
    );
}
fn a_return_value_cleanup_failure_releases_the_prepared_result_once_on_compiler_thread() {
    use hirsel_interp::observation::SemanticRole::*;
    let program = |fail: &str| {
        format!(
            "resource type Boom {{ stdout: Stdout }}\ndrop(x) {{\n  let _ = out.print(x.stdout, \"boom\\n\")\n  {fail}}}\nfn make(stdout: Stdout) -> Trace {{\n  let b = Boom(stdout: stdout)\n  Trace(stdout: stdout, text: \"returned\\n\")\n}}\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] {{\n  let stdout = io.stdout(rt)?\n  let outer = Trace(stdout: stdout, text: \"outer\\n\")\n  let t = make(stdout)\n  Ok(())\n}}\n"
        )
    };
    let failed = observe_stably(&program(FAILS));
    assert_run(&failed, failure("SEM-1", 7), "boom\nreturned\nouter\n");
    assert_eq!(failed.observation.failures.len(), 1);
    let finalizers: Vec<_> = roles(&failed)
        .into_iter()
        .filter(|(role, _)| *role == FinalizerEntry)
        .map(|(_, line)| line)
        .collect();
    // Boom's finalizer, then the prepared result and the caller's local, once each.
    assert_eq!(finalizers, [9, 9, 16]);
    assert_run(
        &observe_stably(&program("")),
        RunOutcome::Exit(0),
        "boom\nreturned\nouter\n",
    );
}

#[test]
fn two_finalizer_failures_are_both_observed_in_unwinding_order() {
    hirsel_engine::with_compiler_stack(
        two_finalizer_failures_are_both_observed_in_unwinding_order_on_compiler_thread,
    );
}
fn two_finalizer_failures_are_both_observed_in_unwinding_order_on_compiler_thread() {
    let program = |fail: &str| {
        format!(
            "resource type Boom {{ stdout: Stdout, text: String }}\ndrop(x) {{\n  let _ = out.print(x.stdout, x.text)\n  {fail}}}\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] {{\n  let stdout = io.stdout(rt)?\n  let outer = Trace(stdout: stdout, text: \"outer\\n\")\n  let a = Boom(stdout: stdout, text: \"a\\n\")\n  let b = Boom(stdout: stdout, text: \"b\\n\")\n  Ok(())\n}}\n"
        )
    };
    let failed = observe_stably(&program(FAILS));
    assert_run(&failed, failure("SEM-1", 7), "b\na\nouter\n");
    let failures = &failed.observation.failures;
    assert_eq!(failures.len(), 2);
    for failure in failures {
        assert_eq!(failure.rule.as_str(), "SEM-1");
        assert_eq!(failure.stable_site.definition.0.name, "Boom");
        assert_eq!(failure.stable_site.line, 7);
    }
    assert_run(
        &observe_stably(&program("")),
        RunOutcome::Exit(0),
        "b\na\nouter\n",
    );
}

fn iterator(fail: &str) -> String {
    format!(
        "fn step(mut s: Trace) -> Option[u8] {{\n  s.text = \"updated\\n\"\n  {fail}  None\n}}\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] {{\n  let stdout = io.stdout(rt)?\n  let state = Trace(stdout: stdout, text: \"initial\\n\")\n  let mut it: Iter[u8] = iter.from(take state, step)\n  let first = iter.next(mut it)\n  Ok(())\n}}\n"
    )
}
#[test]
fn an_iterator_callback_returns_its_updated_state() {
    hirsel_engine::with_compiler_stack(|| {
        assert_run(
            &observe_stably(&iterator("")),
            RunOutcome::Exit(0),
            "updated\n",
        );
    });
}
#[test]
fn an_iterator_callback_failure_after_a_state_update_releases_the_updated_state() {
    hirsel_engine::with_compiler_stack(|| {
        // A `mut` argument is the caller's storage: the update precedes the
        // failure, so unwinding releases the updated state, as for a direct call.
        assert_run(
            &observe_stably(&iterator(FAILS)),
            failure("SEM-1", 6),
            "updated\n",
        );
    });
}

#[test]
fn borrowed_associations_and_scoped_promises_are_checked_at_native_import() {
    let text = include_str!("../../../../conformance/SEM/SEM-4-contract6-borrowed-run.hir");
    let mut engine = engine(text);
    let base = engine.export_native_input().unwrap();
    engine.import_native_input(base.clone()).unwrap();
    let owner = base
        .definitions
        .iter()
        .find(|(_, d)| !d.protocol_associations.is_empty())
        .unwrap()
        .0
        .clone();
    let mut erased = base.clone();
    Arc::make_mut(erased.definitions.get_mut(&owner).unwrap())
        .protocol_associations
        .clear();
    assert!(engine.import_native_input(erased).is_err());
    let mut bad = base.clone();
    Arc::make_mut(bad.definitions.get_mut(&owner).unwrap()).protocol_associations[0]
        .association
        .fingerprint = Default::default();
    assert!(engine.import_native_input(bad).is_err());
    let mut bad = base.clone();
    Arc::make_mut(bad.definitions.get_mut(&owner).unwrap()).protocol_associations[0].next =
        hirsel_tir::BlockId(0);
    assert!(engine.import_native_input(bad).is_err());
    let mut bad = base.clone();
    let signature = Arc::make_mut(bad.signatures.get_mut(&owner).unwrap());
    let SignatureKind::Function(f) = &mut signature.kind else {
        panic!()
    };
    f.state_promises = vec![hirsel_types::StatePromise {
        selector: hirsel_types::RelationPlace {
            parameter: hirsel_types::ParameterRef::bound(hirsel_types::ParamId(0)),
            fields: vec![hirsel_types::FieldId(999)].into(),
        },
        advance_only: true,
        preserve_captures: true,
        preserve_unique: false,
        unretained: Default::default(),
    }]
    .into();
    assert!(engine.import_native_input(bad).is_err());
    let mut bad = base;
    let nominal = bad.signatures.values_mut().find(|s| matches!(&s.kind,SignatureKind::Type(t) if t.loop_source.is_some() && !s.definition.0.package.name_is("core"))).unwrap();
    let SignatureKind::Type(t) = &mut Arc::make_mut(nominal).kind else {
        panic!()
    };
    t.loop_source.as_mut().unwrap().option.0.package.name = "foreign".into();
    assert!(engine.import_native_input(bad).is_err());
}
