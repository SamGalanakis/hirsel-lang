//! Constant-time equality is a property of the operation sequence, which no corpus
//! program can observe: the interpreter's full trace of `std@1/constant_time.equal`
//! must be the same whether, and wherever, equal-length inputs differ, and the
//! ciphers' traces must not depend on their keys or messages.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_interp::{Limits, Outcome, TraceEvent, TraceMode, Value, library::Library};
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

/// The returned value, the step count and every control event of one call of
/// `entry` with byte-string arguments.
fn trace(
    loaded: &mut LoadedProgram,
    entry: &DefPath,
    args: &[&[u8]],
) -> (Value, u64, Vec<TraceEvent>) {
    let mut runtime = loaded.engine.runtime_types().unwrap();
    runtime.trace_mode = TraceMode::Full;
    let sources = loaded.engine.sources.clone();
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
        args.iter().map(|a| Value::Bytes(a.to_vec())).collect(),
        Limits {
            steps: Some(20_000_000),
            ..Default::default()
        },
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
}

fn fixture() -> LoadedProgram {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/constant-time-trace.hir");
    let mut loaded = load_program(&fixture).unwrap();
    loaded.check_for_execution().unwrap();
    loaded
}

#[test]
fn constant_time_equal_runs_one_operation_sequence_per_length() {
    with_compiler_stack(|| {
        let mut loaded = fixture();
        let mut trace = |entry: &DefPath, a: &[u8], b: &[u8]| trace(&mut loaded, entry, &[a, b]);
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

/// Asserts that `entry` gives different results but one step count and one trace
/// for two argument lists that differ only in secret contents.
fn same_trace(
    loaded: &mut LoadedProgram,
    module: &str,
    name: &str,
    first: &[&[u8]],
    second: &[&[u8]],
) {
    let entry = path("std", &[module], name);
    let (value, steps, events) = trace(loaded, &entry, first);
    let (other, other_steps, other_events) = trace(loaded, &entry, second);
    assert!(
        value != other,
        "{module}.{name}: the secrets give one result"
    );
    assert_eq!(steps, other_steps, "{module}.{name}");
    assert!(
        events == other_events,
        "{module}.{name}: a secret changed the trace"
    );
}

/// Two keys of one length, with plaintexts of one length, take the same trace
/// through each AEAD's seal, so no step depends on a secret byte.
#[test]
fn aead_secrets_run_one_operation_sequence_per_length() {
    with_compiler_stack(|| {
        let mut loaded = fixture();
        let (nonce, data) = ([7u8; 12], *b"header");
        let (zeros, ones) = ([0u8; 40], [0xffu8; 40]);
        same_trace(
            &mut loaded,
            "aes_gcm",
            "seal",
            &[&[0x11; 16], &nonce, &zeros, &data],
            &[&[0xee; 16], &nonce, &ones, &data],
        );
        same_trace(
            &mut loaded,
            "chacha20poly1305",
            "seal",
            &[&[0x11; 32], &nonce, &zeros, &data],
            &[&[0xee; 32], &nonce, &ones, &data],
        );
    });
}
