//! Nested let-else cleanup must agree with independently reconstructed tails.
use hirsel_driver::*;
use hirsel_engine::*;

#[test]
fn recursive_tail_agreement_44_original_helper() {
    with_compiler_stack(|| {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/recursive-tail-agreement-44/original-helper.hir");
        let mut program = load_program(&path).unwrap();
        let checked = program.check(LoweringPolicy::Comptime).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let mut bounded = program.entry.clone();
        bounded.0.name = "bounded".into();
        let analyzed = program.engine.mode_definition(&bounded).unwrap();
        assert!(analyzed.value.typed.checked.tails.is_empty());
        let input = program.engine.export_native_input().unwrap();
        let definition = &input.definitions[&bounded];
        let body = &definition.bodies[0];
        assert!(body.tail_sites.is_empty());
        let calls: Vec<_> = body
            .blocks
            .iter()
            .enumerate()
            .filter(|(_, block)| {
                matches!(&block.terminator, hirsel_tir::Terminator::Call {
                callee: hirsel_tir::Callee::Definition { path, .. }, ..
            } if path == &bounded)
            })
            .collect();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, 11);
        let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
        signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
        let coverage =
            hirsel_validate::invocation_coverage(&input.types, definition, &signatures).unwrap();
        assert!(
            coverage
                .iter()
                .any(|&(body, block, _, restart)| body == 0 && block == 11 && !restart)
        );
        let mut forged = (**definition).clone();
        let hirsel_tir::Terminator::Call { arguments, .. } = &calls[0].1.terminator else {
            unreachable!()
        };
        forged.bodies[0].tail_sites.push(hirsel_tir::TailSite {
            owner_body: hirsel_tir::BodyId(0),
            call_block: hirsel_tir::BlockId(11),
            kind: hirsel_tir::TailKind::SelfTail,
            owned_loop_arguments: vec![(hirsel_types::ParamId(0), arguments[0].value.place)],
        });
        let report = hirsel_validate::validate(&forged, &input.types, &signatures, &mut vec![]);
        assert!(
            report.violations.iter().any(|v| matches!(
                v.kind,
                hirsel_validate::ViolationKind::WrongKind {
                    entity: "tail witness",
                    index: 11
                }
            )),
            "{report:?}"
        );
        let mut forged_input = input.clone();
        forged_input
            .definitions
            .insert(bounded, std::sync::Arc::new(forged));
        assert!(program.engine.import_native_input(forged_input).is_err());
        program.engine.import_native_input(input).unwrap();
    });
}

fn checked_source(source: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine
        .sources
        .add("recursive-tail-44.hir", source.into())
        .unwrap();
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(
        checked.diagnostics.is_empty(),
        "{source}\n{:?}",
        checked.diagnostics
    );
    engine
}

const RESULT: &str = r#"
fn attempt(n:u64)->Result[unique List[String],()] uses {} {
 if n == 0u64 {Ok(["kept"])} else {Error(())}
}
"#;
const NESTED: &str = r#"
fn descend(n:u64)->unique List[String] uses {} {
 let Ok(d) = attempt(n) else {
  let Ok(empty) = attempt(n) else {return descend(n-1u64)}
  return take empty
 }
 take d
}
pub fn main()->Result[(),()] {let d=descend(3u64);failure.check(list.len(d)==1u64,TextInvariant());Ok(())}
"#;

#[test]
fn recursive_tail_agreement_44_variations() {
    with_compiler_stack(|| {
        for (name, source, expected) in [
            ("nested", format!("{RESULT}{NESTED}"), 0),
            (
                "explicit-match",
                format!(
                    r#"{RESULT}
fn descend(n:u64)->unique List[String] {{
 match attempt(n) {{
  Ok(d) => take d,
  Error(_) => match attempt(n) {{Ok(empty)=>take empty,Error(_)=>descend(n-1u64)}},
 }}
}}
pub fn main()->Result[(),()] {{let d=descend(3u64);failure.check(list.len(d)==1u64,TextInvariant());Ok(())}}
"#
                ),
                0,
            ),
            (
                "owned-local",
                format!(
                    "{RESULT}{}",
                    NESTED.replace(
                        " let Ok(d)",
                        " let owner:unique List[String]=[\"pending\"];\n let Ok(d)"
                    )
                ),
                0,
            ),
            (
                "plain-result",
                r#"
fn attempt(n:u64)->Result[u64,()] {if n==0u64 {Ok(7u64)}else{Error(())}}
fn descend(n:u64)->u64 {
 let Ok(d)=attempt(n) else {return descend(n-1u64)}
 if n==0u64 {d}else{descend(n-1u64)}
}
pub fn main()->Result[(),()]{failure.check(descend(8u64)==7u64,TextInvariant());Ok(())}
"#
                .into(),
                2,
            ),
            (
                "consumed-success",
                format!(
                    r#"{RESULT}
fn descend(n:u64)->u64 {{
 let Ok(d)=attempt(0u64) else {{return descend(n-1u64)}}
 {{let _=take d}}
 if n==0u64 {{7u64}}else{{descend(n-1u64)}}
}}
pub fn main()->Result[(),()]{{failure.check(descend(8u64)==7u64,TextInvariant());Ok(())}}
"#
                ),
                1,
            ),
            (
                "borrowed-result",
                r#"
fn descend(n:u64,value:Result[List[String],()])->u64 {
 let Ok(d)=value else {return descend(n-1u64,value)}
 list.len(d)
}
pub fn main()->Result[(),()]{failure.check(descend(1u64,Ok(["kept"]))==1u64,TextInvariant());Ok(())}
"#
                .into(),
                1,
            ),
            (
                "loan",
                r#"
fn descend(n:u64,text:List[String])->u64 {
 let next:unique List[String]=["kept"]
 if n==0u64 {list.len(text)}else{descend(n-1u64,next)}
}
pub fn main()->Result[(),()]{failure.check(descend(3u64,["initial"])==1u64,TextInvariant());Ok(())}
"#
                .into(),
                0,
            ),
            (
                "postcondition",
                r#"
fn descend(n:u64)->u64 ensures result==7u64 {if n==0u64{7u64}else{descend(n-1u64)}}
pub fn main()->Result[(),()]{failure.check(descend(3u64)==7u64,TextInvariant());Ok(())}
"#
                .into(),
                0,
            ),
        ] {
            let mut engine = checked_source(&source);
            let input = engine
                .export_native_input()
                .unwrap_or_else(|e| panic!("{name}: {e:?}"));
            let mut descend = entry_path(&default_context());
            descend.0.name = "descend".into();
            assert_eq!(
                input.definitions[&descend].bodies[0].tail_sites.len(),
                expected,
                "{name}"
            );
            engine.import_native_input(input).unwrap();
            let run = run_observed_snapshot(
                &mut engine,
                &RunConfiguration {
                    entry: entry_path(&default_context()),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap();
            let observation = run.observation.unwrap();
            assert_eq!(observation.exit_code, Some(0), "{name}: {observation:?}");
            assert!(observation.failures.is_empty(), "{name}: {observation:?}");
            assert!(run.step_usage.unwrap().consumed > 0);
            println!("recursive-tail-44 accepted {name}: {expected} tails");
        }
    });
}

// Runner structure follows support/history.rs and native.rs's depth replay test.
use hirsel_driver::native;

fn native_seal(
    roots: &[std::path::PathBuf],
) -> std::collections::BTreeMap<String, serde_json::Value> {
    use sha2::{Digest, Sha256};
    use std::{collections::BTreeSet, os::unix::fs::MetadataExt, path::Path};
    fn walk(
        p: &Path,
        ancestors: &BTreeSet<(u64, u64)>,
        rows: &mut std::collections::BTreeMap<String, serde_json::Value>,
    ) {
        let meta = std::fs::symlink_metadata(p).unwrap();
        let resolved = std::fs::metadata(p).unwrap();
        let link = meta
            .file_type()
            .is_symlink()
            .then(|| std::fs::read_link(p).unwrap());
        let mut row =
            serde_json::json!({"mode":meta.mode(),"target_mode":resolved.mode(),"link":link});
        if resolved.is_dir() {
            let mut children: Vec<_> = std::fs::read_dir(p)
                .unwrap()
                .map(|e| e.unwrap().path())
                .collect();
            children.sort();
            row["members"] = serde_json::json!(
                children
                    .iter()
                    .map(|p| p.file_name().unwrap().to_string_lossy())
                    .collect::<Vec<_>>()
            );
            let mut ancestors = ancestors.clone();
            if ancestors.insert((resolved.dev(), resolved.ino())) {
                for child in children {
                    walk(&child, &ancestors, rows)
                }
            }
        } else {
            row["sha256"] =
                serde_json::json!(format!("{:x}", Sha256::digest(std::fs::read(p).unwrap())));
            row["bytes"] = serde_json::json!(resolved.len());
        }
        rows.insert(p.display().to_string(), row);
    }
    let mut rows = std::collections::BTreeMap::new();
    for p in roots {
        walk(p, &BTreeSet::new(), &mut rows)
    }
    rows
}

struct Oracle<'a> {
    output: &'a [u8],
    failure: Option<(&'a str, &'a str, u32, u32)>,
    owners: usize,
}
fn four_engines(
    name: &str,
    engine: &mut QueryEngine,
    entry: &hirsel_syntax::DefPath,
    oracle: Oracle<'_>,
    tools: &native::Toolchain,
    directory: &std::path::Path,
) {
    let Oracle {
        output: expected,
        failure,
        owners: finalizers,
    } = oracle;
    let input = engine.export_native_input().unwrap();
    if let Some((_, descend)) = input
        .definitions
        .iter()
        .find(|(path, _)| path.0.name == "descend")
    {
        assert_eq!(
            descend.bodies[0].tail_sites.len(),
            if name == "eligible-depth" { 2 } else { 0 },
            "{name}"
        );
    }
    let validated = engine.import_native_input(input).unwrap();
    let run = run_observed_snapshot(
        engine,
        &RunConfiguration {
            entry: entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        },
        SimulationConfig::default(),
    )
    .unwrap();
    assert!(run.step_usage.unwrap().consumed > 0);
    let observed = run.observation.unwrap();
    if failure.is_some_and(|(rule, _, _, _)| rule == "LANG-10") {
        assert_eq!(observed.failures.len(), 1);
        assert_eq!(observed.failures[0].kind, "Depth");
    }
    assert_eq!(observed.effects.iter().filter(|e|matches!(&e.effect,hirsel_interp::observation::Effect::ResourceCreated{definition} if definition.0.name=="Stamp")).count(),finalizers,"{name}: owner creation count");
    assert_eq!(observed.effects.iter().filter(|e|matches!(&e.effect,hirsel_interp::observation::Effect::Finalizer{definition} if definition.0.name=="Stamp")).count(),finalizers,"{name}: finalizer count");
    let reference = native::Observation::of_reference(&observed);
    assert_eq!(reference.output, expected, "{name}");
    assert!(!reference.blocked && reference.budget.is_none() && reference.signaled.is_none());
    if let Some((rule, owner, line, column)) = failure {
        assert_eq!(reference.exit_code, None);
        assert_eq!(reference.failures.len(), 1, "{name}: {reference:?}");
        let f = &reference.failures[0];
        assert_eq!(
            (&*f.rule, &*f.definition.0.name, f.line, f.column),
            (rule, owner, line, column),
            "{name}: {reference:?}"
        );
    } else {
        assert_eq!(reference.exit_code, Some(0));
        assert!(reference.failures.is_empty());
    }
    let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
    let heap = native::Configuration {
        options: options.clone(),
        engine: native::Engine::Heap { campaign: true },
    };
    let heap_results = native::differential(
        &validated,
        entry,
        &reference,
        &[heap],
        tools,
        &directory.join(format!("{name}-h")),
    );
    assert_eq!(heap_results.len(), 1);
    assert!(
        matches!(heap_results[0].1, native::Outcome::Agree),
        "{name}: {heap_results:?}"
    );
    let ready = native::build(&validated, &options).unwrap();
    let cell = NativeCell {
        name,
        ready: &ready,
        validated: &validated,
        entry,
        reference: &reference,
        tools,
        directory,
        record_effects: !expected.is_empty(),
    };
    for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
        cell.run(backend)
    }
    println!(
        "TAIL44_BODY {name}: reference, checked-H, LLVM, Cranelift; exact replay and identity rejection"
    );
}

struct NativeCell<'a> {
    name: &'a str,
    ready: &'a native::ReadyProgram,
    validated: &'a ValidatedNativeInput,
    entry: &'a hirsel_syntax::DefPath,
    reference: &'a native::Observation,
    tools: &'a native::Toolchain,
    directory: &'a std::path::Path,
    record_effects: bool,
}
impl NativeCell<'_> {
    fn run(&self, backend: native::Backend) {
        let Self {
            name,
            ready,
            validated,
            entry,
            reference,
            tools,
            directory,
            ..
        } = *self;
        let dir = directory.join(format!("{name}-{backend:?}"));
        std::fs::create_dir_all(&dir).unwrap();
        let objects = native::emit_backend_objects(
            ready,
            backend,
            native::OptLevel::None,
            native::Partition::Whole,
            false,
            None,
        )
        .unwrap();
        let runnable = native::link_runnable(ready, entry, &objects, tools, &dir, None).unwrap();
        let roots: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        let before = native_seal(&roots);
        let tape = dir.join("run.tape");
        let live = native::launch_runnable(
            &runnable,
            &ready.target,
            tools,
            &dir,
            true,
            native::Tape::Live {
                record: Some(&tape),
                revocations: None,
            },
        )
        .unwrap();
        let sites = native::Sites::new(validated.snapshot());
        assert_eq!(
            native::observe(&live, ready, &sites).unwrap(),
            *reference,
            "{name}/{backend:?}"
        );
        let tape_before = std::fs::read(&tape).unwrap();
        let replay = native::launch_runnable(
            &runnable,
            &ready.target,
            tools,
            &dir,
            true,
            native::Tape::Replay(&tape),
        )
        .unwrap();
        assert_eq!(
            native::observe(&replay, ready, &sites).unwrap(),
            *reference,
            "{name}/{backend:?}/replay"
        );
        assert_eq!(std::fs::read(&tape).unwrap(), tape_before);
        assert_eq!(
            native_seal(&roots),
            before,
            "{name}/{backend:?}: live input changed"
        );
        // A distinct corrupt tape must fail admission before the program enters.
        let recording = hirsel_rt::launch::parse_recording(
            &hirsel_rt::launch::read_recording_text(&tape).unwrap(),
        )
        .unwrap();
        let mut bad = recording.clone();
        bad.run_identity[0] ^= 1;
        let wrong = dir.join("wrong-identity.tape");
        std::fs::write(&wrong, hirsel_rt::launch::render_recording(&bad).unwrap()).unwrap();
        let refused = native::launch_runnable(
            &runnable,
            &ready.target,
            tools,
            &dir,
            true,
            native::Tape::Replay(&wrong),
        )
        .unwrap();
        assert!(refused.stdout.is_empty());
        assert!(matches!(
            native::observe_run(&refused, ready, &sites).unwrap(),
            native::Observed::Refused(_)
        ));
        if self.record_effects {
            assert!(
                !recording.events.is_empty(),
                "{name}: missing completion mutation needs real effects"
            );
            eprintln!(
                "TAIL44_COMPLETION_MUTATION {name}/{backend:?}: {} events",
                recording.events.len()
            );
            let mut bad = recording;
            bad.events.clear();
            let missing = dir.join("missing-completion.tape");
            std::fs::write(&missing, hirsel_rt::launch::render_recording(&bad).unwrap()).unwrap();
            let stopped = native::launch_runnable(
                &runnable,
                &ready.target,
                tools,
                &dir,
                true,
                native::Tape::Replay(&missing),
            )
            .unwrap();
            assert!(matches!(
                native::observe_run(&stopped, ready, &sites).unwrap(),
                native::Observed::Stopped(RunOutcome::Diverged(_))
            ));
        } else {
            assert!(
                recording.events.is_empty(),
                "{name}: pure control unexpectedly records effects"
            );
        }
        assert_eq!(native_seal(&roots), before);
        assert_eq!(std::fs::read(&tape).unwrap(), tape_before);
        eprintln!(
            "TAIL44_NATIVE {}",
            serde_json::json!({"case":name,"backend":format!("{backend:?}"),"inputs":before,"tape_sha":format!("{:?}",hirsel_syntax::ContentHash::of_bytes(&tape_before))})
        );
    }
}

const FINALIZERS: &str = r#"resource type Stamp {out:Stdout,text:String}
drop(s) {let _=out.print(s.out,s.text)}
fn attempt(n:u64,out:Stdout,text:String)->Result[u64,Stamp] {
 if n==0u64 {Ok(7u64)}else{Error(Stamp(out:out,text:unique text))}
}
fn descend(n:u64,out:Stdout)->u64 {
 let Ok(d)=attempt(n,out,if n==1u64 {"O1"}else{"O2"}) else {
  let Ok(empty)=attempt(n,out,if n==1u64 {"I1"}else{"I2"}) else {return descend(n-1u64,out)}
  return empty
 }
 d
}
pub fn main(rt:Io)->Result[(),GrantError] {
 let out=io.stdout(rt)?
 failure.check(descend(2u64,out)==7u64,TextInvariant())
 let _=out.print(out,"R")
 Ok(())
}
"#;
const DEPTH: &str = r#"fn descend(n:u64)->unique List[String] {
 if n==0u64 {return ["kept"]}
 let Ok(d):Result[unique List[String],()]=Error(()) else {
  let Ok(empty):Result[unique List[String],()]=Error(()) else {return descend(n-1u64)}
  return take empty
 }
 take d
}
pub fn main()->Result[(),()]{let d=descend(1023u64);failure.check(list.len(d)==1u64,TextInvariant());Ok(())}
"#;
fn coordinate(source: &str, needle: &str) -> (u32, u32) {
    let offset = source.find(needle).unwrap();
    let prefix = &source[..offset];
    (
        prefix.bytes().filter(|&b| b == b'\n').count() as u32 + 1,
        prefix.rsplit('\n').next().unwrap().len() as u32 + 1,
    )
}

#[test]
fn recursive_tail_agreement_44_four_engines_and_replay() {
    with_compiler_stack(|| {
        const CASES: &[&str] = &[
            "same-helper-pure",
            "same-helper-effect-companion",
            "opaque-reachable",
            "nested-reachable",
            "explicit-match",
            "finalizer-order",
            "failure-cleanup",
            "ordinary-depth",
            "eligible-depth",
        ];
        let selection = std::env::var("TAIL44_CASES").unwrap_or_else(|_| CASES.join(","));
        let selected: Vec<_> = selection.split(',').collect();
        assert!(
            !selected.is_empty() && selected.iter().all(|case| CASES.contains(case)),
            "invalid TAIL44_CASES: {selection}"
        );
        eprintln!("TAIL44_SELECTED {selection}");
        let directory = tempfile::tempdir().unwrap();
        let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
        let matrix = [
            native::Engine::Heap { campaign: true },
            native::Engine::Native {
                opt: native::OptLevel::None,
                partition: native::Partition::Whole,
                runtime: native::Runtime::Diagnostic,
            },
            native::Engine::Cranelift {
                partition: native::Partition::Whole,
                runtime: native::Runtime::Diagnostic,
            },
        ]
        .map(|engine| native::Configuration {
            options: options.clone(),
            engine,
        });
        let mut tools =
            native::Toolchain::for_matrix(&directory.path().join("runtime"), &matrix).unwrap();
        tools.execution = native::Execution::Executable;
        let mut tool_inputs = vec![
            tools.lld.clone(),
            tools.kit.clone(),
            std::env::current_exe().unwrap(),
        ];
        tool_inputs.extend(tools.runtime.values().cloned());
        if let Some(floor) = &tools.glibc_floor {
            tool_inputs.push(floor.clone())
        }
        let sealed_tools = native_seal(&tool_inputs);
        eprintln!(
            "TAIL44_TOOLS {}",
            serde_json::to_string(&sealed_tools).unwrap()
        );
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/recursive-tail-agreement-44/original-helper.hir");
        if selected.contains(&"same-helper-pure") {
            let mut original = load_program(&path).unwrap();
            original.check_for_execution().unwrap();
            let pure_input = original.engine.export_native_input().unwrap();
            let pure = original.engine.import_native_input(pure_input).unwrap();
            let ready = native::build(&pure, &options).unwrap();
            let empty = native::Observation {
                exit_code: Some(0),
                blocked: false,
                budget: None,
                signaled: None,
                output: vec![],
                failures: vec![],
                roots: vec![],
            };
            NativeCell {
                name: "same-helper-pure",
                ready: &ready,
                validated: &pure,
                entry: &original.entry,
                reference: &empty,
                tools: &tools,
                directory: directory.path(),
                record_effects: false,
            }
            .run(native::Backend::Cranelift);
        }
        let original_text = std::fs::read_to_string(&path).unwrap();
        // The pure helper has no Io events. Keep its observation separate from
        // this deterministic, granted-effect companion's nonvacuous replay proof.
        if selected.contains(&"same-helper-effect-companion") {
            let effect=original_text.replace("pub fn main()->Result[(),()] {", "pub fn main(rt:Io)->Result[(),GrantError] {let output=io.stdout(rt)?;let _=out.print(output,\"entered\");");
            let mut companion = load_program_source(&path, Some(&effect)).unwrap();
            companion.check_for_execution().unwrap();
            four_engines(
                "same-helper-effect-companion",
                &mut companion.engine,
                &companion.entry,
                Oracle {
                    output: b"entered",
                    failure: None,
                    owners: 0,
                },
                &tools,
                directory.path(),
            );
        }
        if selected.contains(&"opaque-reachable") {
            let reachable = original_text
                .replace("failure.check(false", "failure.check(true")
                .replace(
                    "return bounded(\"\", options)",
                    "return bounded(\"\", markdown.Options(ok: true))",
                )
                .replace(
                    "bounded(\"source\", markdown.Options(ok: true))",
                    "bounded(\"source\", markdown.Options(ok: false))",
                )
                .replace("markdown.length(d) == 6", "markdown.length(d) == 0");
            let mut opaque = load_program_source(&path, Some(&reachable)).unwrap();
            opaque.check_for_execution().unwrap();
            four_engines(
                "opaque-reachable",
                &mut opaque.engine,
                &opaque.entry,
                Oracle {
                    output: b"",
                    failure: None,
                    owners: 0,
                },
                &tools,
                directory.path(),
            );
        }
        for (name,source,expected,failed,owners) in [
            ("nested-reachable",format!("{RESULT}{NESTED}"),&b""[..],false,0),
            ("explicit-match",format!(r#"{RESULT}
fn descend(n:u64)->unique List[String] {{
 match attempt(n) {{Ok(d)=>take d,Error(_)=>match attempt(n) {{Ok(empty)=>take empty,Error(_)=>descend(n-1u64)}}}}
}}
pub fn main()->Result[(),()]{{let d=descend(3u64);failure.check(list.len(d)==1u64,TextInvariant());Ok(())}}
"#),&b""[..],false,0),
            ("finalizer-order",FINALIZERS.into(),&b"I1O1I2O2R"[..],false,4),
            ("failure-cleanup",FINALIZERS.replace(" d\n}"," failure.check(false,TextInvariant())\n d\n}"),&b"I1O1I2O2"[..],true,4),
            ("ordinary-depth",DEPTH.into(),&b""[..],true,0),
            ("eligible-depth",r#"fn descend(n:u64)->u64 {let Ok(x):Result[u64,()]=Ok(n) else {return descend(n-1u64)};if x==0u64 {7u64}else{descend(n-1u64)}}
pub fn main()->Result[(),()]{failure.check(descend(1030u64)==7u64,TextInvariant());Ok(())}"#.into(),&b""[..],false,0),
        ] {
            if !selected.contains(&name) {continue;}
            let mut engine=checked_source(&source);
            let failure=if failed {
                let (line,column)=coordinate(&source,if name=="ordinary-depth" {"descend(n-1u64)"}else{"failure.check(false"});
                Some((if name=="ordinary-depth"{"LANG-10"}else{"SEM-3"},"descend",line,column))
            }else{None};
            four_engines(name,&mut engine,&entry_path(&default_context()),Oracle{output:expected,failure,owners},&tools,directory.path());
        }
        assert_eq!(native_seal(&tool_inputs), sealed_tools);
    });
}

#[test]
fn recursive_tail_agreement_44_loop_owner_counts_and_tooling_depth() {
    use hirsel_interp::{Limits, Outcome, TraceEvent, Value};
    with_compiler_stack(|| {
        let source = r#"fn descend(n:u64,text:String)->u64 {
 let Ok(x):Result[u64,()]=Ok(n) else {return descend(n-1u64,"next")}
 if x==0u64 {string.byte_len(text)}else{descend(n-1u64,"next")}
}
fn main()->u64 {descend(32u64,"initial")}"#;
        for (source, eligible) in [
            (source.to_owned(), true),
            (format!("{RESULT}{NESTED}"), false),
        ] {
            let mut engine = checked_source(&source);
            engine.export_native_input().unwrap();
            let sources = engine.sources.clone();
            let mut runtime = engine.runtime_types().unwrap();
            runtime.trace_mode = hirsel_interp::TraceMode::Full;
            let config = RunConfiguration {
                entry: entry_path(&default_context()),
                hash_seed: 0,
                inputs: vec![],
            };
            let mut library =
                hirsel_interp::library::Library::new(&config, Default::default()).unwrap();
            let run = hirsel_interp::run_with_runtime(
                &mut engine,
                &sources,
                &config,
                vec![],
                Limits {
                    steps: Some(1_000_000),
                    call_depth: 4,
                    source: None,
                },
                &mut library,
                &runtime,
            )
            .unwrap();
            if eligible {
                assert!(
                    matches!(run.outcome,Outcome::Returned{value:Value::Integer(n),..} if n.bits()==4),
                    "{:?}",
                    run.outcome
                );
                let counts: Vec<_> = run
                    .trace
                    .iter()
                    .filter_map(|e| {
                        if let TraceEvent::LoopOwnership { released, live } = e {
                            Some((*released, *live))
                        } else {
                            None
                        }
                    })
                    .collect();
                assert_eq!(counts.iter().map(|x| x.0).sum::<usize>(), 32);
                assert_eq!(counts.iter().map(|x| x.1).max(), Some(1));
                assert_eq!(counts.last().unwrap().1, 0);
            } else {
                assert!(
                    matches!(
                        run.outcome,
                        Outcome::ToolingLimit(hirsel_tir::ToolingLimit {
                            kind: hirsel_tir::ToolingLimitKind::CallDepth,
                            ..
                        })
                    ),
                    "{:?}",
                    run.outcome
                );
            }
        }
    });
}
