use crate::support;
use hirsel_interp::{library::*, *};
use hirsel_span::SourceMap;
use hirsel_tir::*;
use hirsel_types::*;
use support::*;
struct Harness {
    library: Library,
    run: RunConfiguration,
    sources: SourceMap,
    stdout: Vec<u8>,
    trace: Vec<TraceEvent>,
    types: TypeStore,
}
impl Harness {
    fn new() -> Self {
        let run = RunConfiguration {
            entry: support::path("main"),
            hash_seed: 42,
            inputs: vec![],
        };
        Self {
            library: Library::new(
                &run,
                SimulationConfig {
                    test_seed: 42,
                    ..Default::default()
                },
            )
            .unwrap(),
            run,
            sources: sources(),
            stdout: vec![],
            trace: vec![],
            types: TypeStore::default(),
        }
    }
    fn call(&mut self, name: &str, args: &[Value]) -> OperationResult {
        let result_type = self.types.push(ty(TypeKind::Tuple(vec![].into())));
        self.library
            .call(
                OperationCall {
                    runtime: &Default::default(),
                    path: &hirsel_syntax::DefPath::core(name),
                    arguments: args,
                    generics: &[],
                    result_type,
                    types: &self.types,
                    site: site(7),
                },
                &mut OperationContext {
                    trace_mode: Default::default(),
                    anchor: None,
                    configuration: &self.run,
                    sources: &self.sources,
                    stdout: &mut self.stdout,
                    trace: &mut self.trace,
                },
            )
            .unwrap()
            .unwrap()
    }
}
fn returned(result: OperationResult) -> Value {
    let OperationResult::Complete { value: v, .. } = result else {
        panic!("expected value")
    };
    v
}
#[test]
fn hash_random_and_test_seeds_vary_independently() {
    let sample = |hash_seed, random_seed, test_seed| {
        let mut h = Harness::new();
        h.run.hash_seed = hash_seed;
        h.library = Library::new(
            &h.run,
            SimulationConfig {
                random_seed,
                test_seed,
                ..Default::default()
            },
        )
        .unwrap();
        let test = h.library.simulation.grant_test();
        let io = h.library.simulation.grant_root("Io").unwrap();
        let Value::Variant { variant: 0, fields } = returned(h.call("io.random", &[io])) else {
            panic!("configured Random grant must succeed")
        };
        let random = fields[0].clone().unwrap();
        let Value::Variant { variant: 0, fields } =
            returned(h.call("random.bytes", &[random, u64_value(8)]))
        else {
            panic!("splitmix64 draws never fail")
        };
        let number = fields[0].clone().unwrap();
        let _ = h.call("test.check_case", &[test, Value::Bool(false), u64_value(7)]);
        (number, h.stdout)
    };
    let baseline = sample(0, 3, 5);
    assert_eq!(baseline, sample(1, 3, 5));
    let random_changed = sample(0, 7, 5);
    assert_ne!(baseline.0, random_changed.0);
    assert_eq!(baseline.1, random_changed.1);
    let test_changed = sample(0, 3, 11);
    assert_eq!(baseline.0, test_changed.0);
    assert_eq!(baseline.1, b"test failure: seed=5 case=7\n");
    assert_eq!(test_changed.1, b"test failure: seed=11 case=7\n");
}
