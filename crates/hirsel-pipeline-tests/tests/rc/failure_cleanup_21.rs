//! Finite DROP-2/FAIL-3 source histories with an independent owner stack.
//! Rust MIR initialized.rs:255-303 and framework/tests.rs:1-68 motivate
//! before/after terminator and separate normal/failure edges; no Rust analysis
//! state or drop rules are imported. Existing ownership_history_21 supplies
//! empty/equal/short/long nested-copy snapshots, rather than duplicating them.
use crate::rc_support::{history::*, *};
use hirsel_heapcheck::{Config, Event, NoOperations, Observed};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Exit {
    Join,
    Break,
    Return,
    Error,
    Division,
}

#[derive(Clone, Copy, Debug)]
struct Case {
    exit: Exit,
    right: bool,
    bad_drops: bool,
}

fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for exit in [
        Exit::Join,
        Exit::Break,
        Exit::Return,
        Exit::Error,
        Exit::Division,
    ] {
        for right in [false, true] {
            cases.push(Case {
                exit,
                right,
                bad_drops: false,
            });
        }
    }
    for exit in [Exit::Return, Exit::Division] {
        for right in [false, true] {
            cases.push(Case {
                exit,
                right,
                bad_drops: true,
            });
        }
    }
    cases
}

#[derive(Debug)]
struct Expected {
    drops: Vec<u32>,
    failures: Vec<&'static str>,
    output: String,
    error: bool,
}

/// A scope stack of source owner identities, with no compiler state or flags.
/// Transfer changes ownership before the exit; failed drops still consume the
/// owner and leave the remaining stack to drain. Failure sites are ordered data.
fn model(case: Case) -> Expected {
    let mut scopes = vec![vec![0, 1, 2], Vec::new()];
    let selected = usize::from(case.right);
    let owner = scopes[0].remove(selected);
    scopes[1].push(owner);
    let mut expected = Expected {
        drops: Vec::new(),
        failures: if case.exit == Exit::Division {
            vec!["probe"]
        } else {
            Vec::new()
        },
        output: "B".into(),
        error: case.exit == Exit::Error,
    };
    while let Some(mut scope) = scopes.pop() {
        while let Some(owner) = scope.pop() {
            expected.drops.push(owner);
            expected.output.push(char::from(b'1' + owner as u8));
            if case.bad_drops {
                expected.failures.push(["R0", "R1", "R2"][owner as usize]);
            }
        }
        if scopes.len() == 1
            && expected.failures.is_empty()
            && matches!(case.exit, Exit::Join | Exit::Break)
        {
            expected.output.push('J');
        }
    }
    if expected.failures.is_empty() {
        expected.output.push(if expected.error { 'E' } else { 'O' });
    }
    expected
}

/// The only difference between the synchronous twin and NoOperations twin is
/// observation IO. Ownership, control flow and arithmetic failure sites match.
fn source(case: Case, traced: bool) -> String {
    let mut text = String::new();
    for n in 0..3 {
        if traced {
            text.push_str(&format!("resource type R{n} {{out:Stdout,d:u64}}\n drop(x) {{let _=out.print(x.out,\"{}\");let _=1u64/x.d}}\n",n+1));
        } else {
            text.push_str(&format!(
                "resource type R{n} {{d:u64}}\n @noalloc drop(x) {{let _=1u64/x.d}}\n"
            ));
        }
    }
    text.push_str(if traced {
        "fn probe(out:Stdout,right:bool)->Result[(),()] {\n"
    } else {
        "fn probe(right:bool)->Result[(),()] {\n"
    });
    for (name, n) in [("a", 0), ("b", 1), ("c", 2)] {
        text.push_str(&format!(
            "let {name}=R{n}({}d:{}u64)\n",
            if traced { "out:out," } else { "" },
            if case.bad_drops { 0 } else { 1 }
        ));
    }
    if traced {
        text.push_str("let _=out.print(out,\"B\")\n");
    }
    if case.exit == Exit::Break {
        text.push_str("while true {\n");
    }
    text.push_str("if right {let moved=take b;\n");
    let tail = match case.exit {
        Exit::Join => "()",
        Exit::Break => "break",
        Exit::Return => "return Ok(())",
        Exit::Error => "let e:Result[(),()]=Error(());e?;()",
        Exit::Division => "let zero:u64=0;let _=1u64/zero",
    };
    text.push_str(tail);
    text.push_str("\n} else {let moved=take a;\n");
    text.push_str(tail);
    text.push_str("\n}\n");
    if case.exit == Exit::Break {
        text.push_str("}\n");
    }
    if traced && matches!(case.exit, Exit::Join | Exit::Break) {
        text.push_str("let _=out.print(out,\"J\")\n");
    }
    text.push_str("Ok(())\n}\n");
    if traced {
        text.push_str(&format!("pub fn main(rt:Io)->Result[(),GrantError] {{let stdout=io.stdout(rt)?;let value=probe(stdout,{});let _=out.print(stdout,match value {{Ok(_)=>\"O\",Error(_)=>\"E\"}});Ok(())}}", case.right));
    } else {
        text.push_str(&format!(
            "pub fn main()->u64 {{let value=probe({});match value {{Ok(_)=>7u64,Error(_)=>9u64}}}}",
            case.right
        ));
    }
    text
}

fn dropped(run: &hirsel_heapcheck::Run) -> Vec<u32> {
    run.trace
        .iter()
        .filter_map(|event| match event {
            Event::Finalize { resource } => Some(resource.0),
            _ => None,
        })
        .collect()
}

#[test]
fn finite_exits_reached_h_faults() {
    let mut injections = 0;
    let mut owned_injections = 0;
    for (index, case) in cases().into_iter().enumerate() {
        let source = source(case, false);
        let expected = model(case);
        // Admission of main's source call precedes every owner in probe.
        // Derive its exact location from the source, independently of LIR.
        let call = format!("probe({})", case.right);
        let start = source.rfind(&call).unwrap();
        let base = item_bases(&source)[&path("main")] as usize;
        let entry_site = ((start - base) as u32, (start + call.len() - base) as u32);
        let mut entry_injections = 0;
        let mut case_owned_injections = 0;
        let program = super::noalloc_cleanup_model_16::program(&source);
        let campaign = hirsel_heapcheck::fault_campaign(
            &program,
            &path("main"),
            &[],
            &mut || Box::new(NoOperations),
            &Config {
                full_recount_every_step: true,
                compare_campaigns: true,
                step_limit: 100_000,
                ..Config::default()
            },
        );
        assert!(
            campaign.first_violation().is_none(),
            "{case:?}\n{source}\n{:?}",
            campaign.first_violation()
        );
        let clean = &campaign.clean;
        let audit = clean.result.as_ref().unwrap();
        assert_eq!(dropped(clean), expected.drops, "{case:?}");
        assert_eq!(clean.counters.finalizers, 3);
        assert_eq!(clean.counters.live_heap_bytes, 0);
        assert_eq!(clean.counters.allocations, 0);
        let definitions: Vec<_> = audit
            .observation
            .failures
            .iter()
            .map(|f| f.definition.0.name.as_str())
            .collect();
        assert_eq!(definitions, expected.failures, "{case:?}");
        assert!(audit.observation.failures.iter().all(|f| f.rule == "SEM-1"));
        if expected.failures.is_empty() {
            assert!(
                matches!(audit.observation.returned, Some(Observed::Integer {bits, ..}) if bits == if expected.error {9} else {7})
            );
        } else {
            assert!(audit.observation.returned.is_none());
        }
        assert_eq!(campaign.injected.len() as u64, clean.sites);
        assert!(clean.sites > 0, "positive injection control: {case:?}");
        for (site, run) in &campaign.injected {
            let audit = run.result.as_ref().unwrap();
            assert!(run.sites > *site, "injection must actually be reached");
            assert!(
                audit
                    .observation
                    .failures
                    .iter()
                    .any(|f| f.rule == hirsel_heapcheck::INJECTED_RULE)
            );
            assert_eq!(run.counters.live_heap_bytes, 0);
            assert_eq!(run.counters.allocations, 0);
            let injected = audit
                .observation
                .failures
                .iter()
                .find(|failure| failure.rule == hirsel_heapcheck::INJECTED_RULE)
                .unwrap();
            if injected.definition == path("main") && injected.site == entry_site {
                // The source model has not entered probe: it owns nothing.
                // Only this exact call admission, reached before all its owners,
                // may have an empty drop trace. Every later fault keeps the
                // original owner-stack and cause-chain assertions below.
                assert_eq!(*site, 0, "{case:?}: {injected:?}");
                assert_eq!(audit.observation.failures.len(), 1);
                assert!(audit.observation.returned.is_none());
                assert!(dropped(run).is_empty());
                assert_eq!(run.counters.finalizers, 0);
                entry_injections += 1;
                injections += 1;
                continue;
            }
            case_owned_injections += 1;
            owned_injections += 1;
            // All owners are built after entry admission and before the next
            // injectable operation. Cleanup faults preserve their identity/order.
            assert_eq!(dropped(run), expected.drops, "{case:?} site={site}");
            assert_eq!(run.counters.finalizers, 3);
            // A failure raised before this injected cleanup site stays primary.
            let clean_failures = &clean.result.as_ref().unwrap().observation.failures;
            let first_injected = audit
                .observation
                .failures
                .iter()
                .position(|f| f.rule == hirsel_heapcheck::INJECTED_RULE)
                .unwrap();
            assert_eq!(
                &audit.observation.failures[..first_injected],
                &clean_failures[..first_injected]
            );
            // A reached failing division is replaced; an otherwise successful
            // drop gains one injected cause. Later cleanup failures retain
            // their exact identities, sites and order in either case.
            let replaced = case.bad_drops || (case.exit == Exit::Division && first_injected == 0);
            assert_eq!(
                &audit.observation.failures[first_injected + 1..],
                &clean_failures[first_injected + usize::from(replaced)..],
                "{case:?} site={site}"
            );
            injections += 1;
        }
        assert_eq!(
            entry_injections, 1,
            "{case:?}: exact pre-owner admission witness"
        );
        assert!(
            case_owned_injections > 0,
            "{case:?}: owner-live fault witness"
        );
        persist(
            &std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("failure-cleanup-21"),
            &format!("h-{index}"),
            index as u64,
            &source,
            &format!(
                "case={case:?} expected={expected:?} sites={} injections={}",
                clean.sites,
                campaign.injected.len()
            ),
        );
        eprintln!(
            "failure_cleanup_21 H case={index} {case:?}: sites={} reached={} finalizers=3 live=0",
            clean.sites,
            campaign.injected.len()
        );
    }
    assert!(injections >= 42);
    assert!(owned_injections >= 42);
    eprintln!(
        "failure_cleanup_21: 14 H clean cases; {injections} positively reached injections; fork/replay equal"
    );
}

#[test]
fn finite_exits_all_engines() {
    let artifacts =
        std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("failure-cleanup-21");
    let runner = Runner::new(&artifacts.join("runs"))
        .expect("toolchain setup is incomplete, never agreement");
    let mut count = 0;
    for (index, case) in cases().into_iter().enumerate() {
        let source = source(case, true);
        let expected = model(case);
        let report = hirsel_engine::with_compiler_stack(|| {
            use hirsel_engine::*;
            use hirsel_interp::observation::{Effect, SemanticRole};
            let mut engine =
                QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
            let file = engine.sources.add("main.hir", source.clone()).unwrap();
            let checked = check_progress(&mut engine, file, &default_context()).unwrap();
            assert!(
                checked.diagnostics.is_empty(),
                "{case:?}: {:?}\n{source}",
                checked.diagnostics
            );
            let input = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(input).unwrap();
            let entry = entry_path(&default_context());
            let run = run_observed_snapshot(
                &mut engine,
                &RunConfiguration {
                    entry: entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap();
            let observation = run.observation.expect("tooling limit is incomplete");
            let finalizers: Vec<_> = observation
                .effects
                .iter()
                .filter(|e| matches!(e.effect, Effect::Finalizer { .. }))
                .collect();
            assert_eq!(finalizers.len(), 3);
            let definitions: Vec<_> = finalizers
                .iter()
                .map(|e| {
                    assert_eq!(e.anchor.role, SemanticRole::FinalizerEntry);
                    match &e.effect {
                        Effect::Finalizer { definition } => definition.0.name.as_str(),
                        _ => unreachable!(),
                    }
                })
                .collect();
            let expected_definitions: Vec<_> = expected
                .drops
                .iter()
                .map(|id| ["R0", "R1", "R2"][*id as usize])
                .collect();
            assert_eq!(definitions, expected_definitions, "{case:?}");
            for (index, failure) in observation.failures.iter().enumerate() {
                assert_eq!(failure.kind, "DivisionByZero");
                assert!(failure.spawn_path.is_empty());
                let causes: Vec<_> = (index + 1 < observation.failures.len())
                    .then_some(index + 1)
                    .into_iter()
                    .collect();
                assert_eq!(failure.cause_indices, causes, "{case:?}");
            }
            let reference = hirsel_driver::native::Observation::of_reference(&observation);
            runner.compare(&validated, &entry, &reference, &format!("exit-{index}"))
        });
        persist(
            &artifacts,
            &format!("source-{index}"),
            index as u64,
            &source,
            &format!("case={case:?} expected={expected:?}\n{report:#?}"),
        );
        assert_eq!(report.cells.len(), 4, "{case:?}: {report:#?}");
        assert!(report.agrees(), "{case:?}: {report:#?}");
        for cell in &report.cells {
            let observed = cell.observation.as_ref().unwrap();
            assert_eq!(
                observed.output,
                expected.output.as_bytes(),
                "{case:?} {}",
                cell.engine
            );
            assert_eq!(
                observed.exit_code,
                expected.failures.is_empty().then_some(0)
            );
            let definitions: Vec<_> = observed
                .failures
                .iter()
                .map(|f| f.definition.0.name.as_str())
                .collect();
            assert_eq!(definitions, expected.failures, "{case:?} {}", cell.engine);
            assert!(observed.failures.iter().all(|f| f.rule == "SEM-1"));
            assert!(!observed.blocked && observed.budget.is_none() && observed.signaled.is_none());
            count += 1;
        }
        eprintln!(
            "failure_cleanup_21 all engines case={index} {case:?}: output={} failures={:?}",
            expected.output, expected.failures
        );
    }
    assert_eq!(count, 56);
    eprintln!(
        "failure_cleanup_21: 14 histories, 56 complete reference/H/LLVM/Cranelift observations; native injected faults unavailable (synchronous failures executed)"
    );
}
