//! Constant-time equality is a property of the operation sequence, which no corpus
//! program can observe: the interpreter's full trace of `std@1/constant_time.equal`
//! must be the same whether, and wherever, equal-length inputs differ.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_interp::{Outcome, TraceEvent, TraceMode, Value, library::Library};
use hirsel_syntax::{DefPath, ItemPath, PackageId};
use std::path::PathBuf;

fn path(package: &str, modules: &[&str], name: &str) -> DefPath {
    DefPath(ItemPath {
        package: PackageId {
            name: package.into(),
            version: "1".into(),
        },
        modules: modules.iter().map(|m| (*m).into()).collect(),
        name: name.into(),
    })
}

#[test]
fn constant_time_equal_runs_one_operation_sequence_per_length() {
    with_compiler_stack(|| {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/constant-time-trace.hir");
        let mut loaded = load_program(&fixture).unwrap();
        loaded.check_for_execution().unwrap();
        let mut runtime = loaded.engine.runtime_types().unwrap();
        runtime.trace_mode = TraceMode::Full;
        let sources = loaded.engine.sources.clone();
        // The returned value, the step count and every control event of one call.
        let mut trace = |entry: &DefPath, a: &[u8], b: &[u8]| {
            let configuration = RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            };
            let mut library = Library::new(&configuration, Default::default()).unwrap();
            let execution = hirsel_interp::run_with_runtime(
                &mut loaded.engine,
                &sources,
                &configuration,
                vec![Value::Bytes(a.to_vec()), Value::Bytes(b.to_vec())],
                Default::default(),
                &mut library,
                &runtime,
            )
            .unwrap();
            let Outcome::Returned { value, .. } = execution.outcome else {
                panic!("{:?}", execution.outcome);
            };
            let events: Vec<_> = execution
                .trace
                .into_iter()
                .filter(|event| {
                    matches!(
                        event,
                        TraceEvent::Enter { .. }
                            | TraceEvent::Instruction { .. }
                            | TraceEvent::Terminator { .. }
                            | TraceEvent::Cleanup { .. }
                            | TraceEvent::Return { .. }
                    )
                })
                .collect();
            assert!(!events.is_empty());
            (value, execution.steps, events)
        };
        let secret = [0x5au8; 32];
        let differing = |at: usize| {
            let mut other = secret;
            other[at] ^= 0x80;
            other
        };
        let equal = path("std", &["constant_time"], "equal");
        let (value, steps, events) = trace(&equal, &secret, &secret);
        assert_eq!(value, Value::Bool(true));
        for at in [0, 15, 31] {
            let (value, other_steps, other_events) = trace(&equal, &secret, &differing(at));
            assert_eq!(value, Value::Bool(false), "difference at {at}");
            assert_eq!(other_steps, steps, "difference at {at}");
            assert!(
                other_events == events,
                "difference at {at} changed the trace"
            );
        }
        // The control: an early exit is visible in exactly this observation.
        let early = path("application", &[], "early_exit");
        let (_, _, first) = trace(&early, &secret, &differing(0));
        let (_, _, last) = trace(&early, &secret, &differing(31));
        assert!(first != last, "an early exit must change the trace");
    });
}
