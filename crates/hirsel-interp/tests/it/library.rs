//! Boundary oracles for every implemented core library operation, using shared data only.
use crate::support;
use hirsel_interp::{
    library::{collection, float, integer, simulation::Fault, *},
    *,
};
use hirsel_span::{SourceMap, Span};
use hirsel_tir::*;
use hirsel_types::*;
use std::collections::BTreeMap;
use support::{Builder, Provider, ty};
fn run_config() -> RunConfiguration {
    RunConfiguration {
        entry: support::path("main"),
        hash_seed: 42,
        inputs: vec![],
    }
}
struct Harness {
    library: Library,
    types: std::sync::Arc<TypeInterner>,
    sources: SourceMap,
    stdout: Vec<u8>,
    trace: Vec<TraceEvent>,
    run: RunConfiguration,
}
impl Harness {
    fn new(config: SimulationConfig) -> Self {
        let run = run_config();
        let sources = support::sources_of("library.hir", "\n".repeat(20));
        Self {
            library: Library::new(&run, config).unwrap(),
            types: crate::support::session(),
            sources,
            stdout: vec![],
            trace: vec![],
            run,
        }
    }
    fn try_call(
        &mut self,
        name: &str,
        mut args: Vec<Value>,
        result: TypeKind,
    ) -> Result<OperationResult, Error> {
        let result = self.types.intern(ty(result));
        let path = hirsel_syntax::DefPath::core(name);
        let operation = self
            .library
            .call(
                OperationCall {
                    runtime: &crate::support::runtime(),
                    path: &path,
                    generics: &[],
                    arguments: &mut args,
                    result_type: result,
                    types: &self.types,
                    site: support::site(7),
                },
                &mut OperationContext {
                    trace_mode: Default::default(),
                    anchor: None,
                    configuration: &self.run,
                    sources: &self.sources,
                    stdout: &mut self.stdout,
                    trace: &mut self.trace,
                },
            )?
            .ok_or_else(|| Error::Unavailable("missing operation".into()))?;
        Ok(operation)
    }
    fn call(&mut self, name: &str, args: Vec<Value>, result: TypeKind) -> OperationResult {
        self.try_call(name, args, result).unwrap()
    }
    fn value(&mut self, name: &str, args: Vec<Value>) -> Value {
        match self.call(name, args, TypeKind::Tuple(vec![].into())) {
            OperationResult::Complete { value: v, .. } => v,
            other => panic!("{other:?}"),
        }
    }
    fn root(&mut self, name: &str) -> Value {
        self.library.simulation.grant_root(name).unwrap()
    }
    fn project(&mut self, io: &Value, name: &str) -> Value {
        {
            let value = self.value(&format!("io.{name}"), vec![io.clone()]);
            if matches!(name, "stdout" | "tasks" | "net" | "random") {
                unok(value)
            } else {
                value
            }
        }
    }
}
fn n(p: Primitive, x: i128) -> Integer {
    Integer::new(
        p,
        x as u128 & (u128::MAX >> (128 - value::integer_layout(p).unwrap().0)),
    )
    .unwrap()
}
#[test]
fn every_integer_conversion_pair_and_all_four_contracts() {
    use Primitive::*;
    let kinds = [U8, U16, U32, U64, U128, I8, I16, I32, I64, I128];
    for source in kinds {
        let (w, s) = value::integer_layout(source).unwrap();
        let values = [0, 1, (u128::MAX >> (128 - w))];
        for target in kinds {
            let (wt, st) = value::integer_layout(target).unwrap();
            assert_eq!(
                integer::widening_pair(source, target),
                (!s || st) && if !s && st { wt > w } else { wt >= w }
            );
            for bits in values {
                let v = Integer::new(source, bits).unwrap();
                let exact = conversion::exact(v, target).unwrap();
                assert_eq!(integer::convert("try_narrow", v, target).unwrap(), exact);
                assert_eq!(
                    integer::convert("narrow", v, target).is_ok(),
                    exact.is_some()
                );
                if wt <= w {
                    assert_eq!(
                        integer::convert("truncate", v, target).unwrap(),
                        Some(conversion::truncate(v, target).unwrap())
                    );
                } else {
                    assert!(integer::convert("truncate", v, target).is_err());
                }
                assert_eq!(
                    integer::convert("widen", v, target).is_ok(),
                    integer::widening_pair(source, target)
                );
            }
        }
    }
}
#[test]
fn floor_division_divisor_signed_mod_and_exact_division() {
    use Primitive::*;
    for p in [I8, I16, I32, I64, I128] {
        for (a, b, q, r) in [
            (-7, 3, -3, 2),
            (7, -3, -3, -2),
            (-7, -3, 2, -1),
            (7, 3, 2, 1),
            (-6, 3, -2, 0),
        ] {
            assert_eq!(
                integer::divide("div_floor", n(p, a), n(p, b))
                    .unwrap()
                    .signed(),
                q
            );
            assert_eq!(
                integer::divide("mod", n(p, a), n(p, b)).unwrap().signed(),
                r
            );
            assert_eq!(
                integer::divide("div_exact", n(p, a), n(p, b)).is_ok(),
                r == 0
            );
        }
        let (w, _) = value::integer_layout(p).unwrap();
        let min = if w == 128 {
            i128::MIN
        } else {
            -(1i128 << (w - 1))
        };
        for op in ["div_floor", "mod", "div_exact"] {
            assert!(integer::divide(op, n(p, min), n(p, -1)).is_err());
            assert!(integer::divide(op, n(p, 1), n(p, 0)).is_err());
        }
    }
    for p in [U8, U16, U32, U64, U128] {
        assert_eq!(
            integer::divide("div_floor", n(p, 7), n(p, 3))
                .unwrap()
                .bits(),
            2
        );
        assert_eq!(integer::divide("mod", n(p, 7), n(p, 3)).unwrap().bits(), 1);
        assert!(integer::divide("div_exact", n(p, 7), n(p, 3)).is_err());
    }
}
fn f(x: f64) -> Value {
    value::f64_value(x)
}
fn f32v(x: f32) -> Value {
    value::f32_value(x)
}
#[test]
fn float_int_conversion_and_explicit_fusion() {
    for (x, expected) in [
        (f64::NAN, Err(0)),
        (f64::INFINITY, Err(0)),
        (2147483648.0, Err(1)),
        (-2147483649.0, Err(1)),
        (-2147483648.9, Ok(i32::MIN)),
        (2147483647.9, Ok(i32::MAX)),
        (-1.9, Ok(-1)),
        (-0.0, Ok(0)),
    ] {
        assert_eq!(float::to_i32(&f(x)), Some(expected));
    }
    assert_eq!(float::to_i32(&f32v(2147483648.0)), Some(Err(1)));
    assert_eq!(
        float::mul_add(
            f32v(1.0 + f32::EPSILON),
            f32v(1.0 - f32::EPSILON),
            f32v(-1.0)
        ),
        Some(f32v(-f32::EPSILON * f32::EPSILON))
    );
    assert_eq!(
        float::mul_add(f(1.0 + f64::EPSILON), f(1.0 - f64::EPSILON), f(-1.0)),
        Some(f(-f64::EPSILON * f64::EPSILON))
    );
}
#[test]
fn ordered_collections_replace_without_reordering_and_copy_independence() {
    let empty = collection::sequence(BuiltinType::Map, []);
    let a = Value::String("a".into());
    let b = Value::String("b".into());
    let m = inserted(
        inserted(empty.clone(), a.clone(), u64_value(1)),
        b.clone(),
        u64_value(2),
    );
    // A copy shares its entries until the first write detaches the written holder.
    let changed = inserted(m.clone(), a.clone(), u64_value(3));
    let entries = collection::elements(&changed, BuiltinType::Map).unwrap();
    assert_eq!(
        entries[0],
        Value::Tuple(vec![Some(a.clone()), Some(u64_value(3))])
    );
    assert_eq!(
        collection::elements(&m, BuiltinType::Map).unwrap()[0],
        Value::Tuple(vec![Some(a.clone()), Some(u64_value(1))])
    );
    let reversed = inserted(inserted(empty, b, u64_value(2)), a, u64_value(3));
    assert!(!collection::data_equal(&changed, &reversed));
    let mut set = collection::sequence(BuiltinType::Set, []);
    released(collection::set_insert(&mut set, u64_value(1)).unwrap());
    let mut again = set.clone();
    released(collection::set_insert(&mut again, u64_value(1)).unwrap());
    assert_eq!(again, set);
    assert!(!collection::data_equal(&f(f64::NAN), &f(f64::NAN)));
    assert!(collection::data_equal(&f(-0.0), &f(0.0)));
    let mut list = collection::sequence(BuiltinType::List, []);
    collection::push(&mut list, u64_value(1)).unwrap();
    assert_eq!(collection::len(&list).unwrap(), 1);
}
fn unok(v: Value) -> Value {
    if let Value::Variant {
        variant: 0,
        mut fields,
    } = v
    {
        fields.remove(0).unwrap()
    } else {
        panic!("{v:?}")
    }
}
#[test]
fn confined_file_tree_all_operations_and_atomic_commit() {
    let config = SimulationConfig {
        files: [
            ("root/a.txt".into(), b"abc".to_vec()),
            ("root/sub/b.txt".into(), b"b".to_vec()),
        ]
        .into_iter()
        .collect(),
        ..Default::default()
    };
    let mut h = Harness::new(config);
    let io = h.root("Io");
    let dir = unok(h.value("io.dir", vec![io.clone(), path("root")]));
    for p in ["../escape", "/absolute", "../../root/a.txt"] {
        assert!(matches!(
            h.value("file.read_bytes", vec![dir.clone(), path(p)]),
            Value::Variant { variant: 1, .. }
        ));
    }
    assert_eq!(
        h.value("file.size", vec![dir.clone(), path("a.txt")]),
        ok(u64_value(3))
    );
    assert_eq!(
        h.value("file.read_bytes", vec![dir.clone(), path("a.txt")]),
        ok(Value::Bytes(b"abc".to_vec()))
    );
    assert_eq!(
        h.value(
            "file.write_bytes",
            vec![dir.clone(), path("a.txt"), Value::Bytes(b"new".to_vec())]
        ),
        ok(Value::Unit)
    );
    let mut walk = unok(h.value("file.walk", vec![dir.clone()]));
    assert_eq!(
        drain_walk(&mut h, &mut walk),
        vec![ok(path("a.txt")), ok(path("sub/b.txt"))]
    );
    let narrow = unok(h.value("dir.narrow", vec![dir.clone(), path("sub")]));
    let file = unok(h.value("file.open", vec![dir.clone(), path("a.txt")]));
    assert_eq!(h.value("file.narrow", vec![narrow, file]), none());
    let file = unok(h.value("file.open", vec![dir.clone(), path("a.txt")]));
    assert!(matches!(
        h.value("file.narrow", vec![dir.clone(), file]),
        Value::Variant { variant: 1, .. }
    ));
    let atomic = unok(h.value("file.create_atomic", vec![dir.clone(), path("dst")]));
    assert_eq!(
        h.value(
            "file.write_all",
            vec![dir.clone(), atomic.clone(), Value::Bytes(b"part1".to_vec())]
        ),
        ok(Value::Unit)
    );
    h.value(
        "file.write_all",
        vec![dir.clone(), atomic.clone(), Value::Bytes(b"part2".to_vec())],
    );
    assert_eq!(
        h.value("file.commit", vec![dir.clone(), atomic]),
        ok(Value::Unit)
    );
    assert_eq!(
        h.value("file.read_bytes", vec![dir.clone(), path("dst")]),
        ok(Value::Bytes(b"part1part2".to_vec()))
    );
    assert_eq!(
        h.value("file.remove", vec![dir.clone(), path("dst")]),
        ok(Value::Unit)
    );
    assert!(matches!(
        h.value("file.remove", vec![dir, path("dst")]),
        Value::Variant { variant: 1, .. }
    ));
}
/// A grant is a trust boundary: `..` never climbs above its root, even where the escaped
/// path names a file that exists (a sibling grant, a file beside the root, one reached back
/// through the root's own name), so the refusal is the invalid-path code, never a read and
/// never the missing-path code an escape onto nothing would also give.
#[test]
fn an_escape_onto_an_existing_file_is_refused_as_invalid() {
    let config = SimulationConfig {
        files: [
            ("root/a.txt".into(), b"abc".to_vec()),
            ("root/sub/b.txt".into(), b"b".to_vec()),
            ("secret.txt".into(), b"secret".to_vec()),
            ("other/c.txt".into(), b"c".to_vec()),
        ]
        .into_iter()
        .collect(),
        invalid_path_code: 71,
        ..Default::default()
    };
    let mut h = Harness::new(config);
    let io = h.root("Io");
    let dir = unok(h.value("io.dir", vec![io.clone(), path("root")]));
    let refused = error(Value::Variant {
        variant: 0,
        fields: vec![Some(u32_value(71))],
    });
    for p in [
        "../secret.txt",
        "../other/c.txt",
        "sub/../../secret.txt",
        "../root/a.txt",
        "./../other/c.txt",
    ] {
        assert_eq!(
            h.value("file.open", vec![dir.clone(), path(p)]),
            refused,
            "{p}"
        );
        assert!(
            matches!(
                h.value("file.read_bytes", vec![dir.clone(), path(p)]),
                Value::Variant { variant: 1, .. }
            ),
            "{p}"
        );
    }
    let narrow = unok(h.value("dir.narrow", vec![dir.clone(), path("sub")]));
    assert_eq!(
        h.value("file.open", vec![narrow.clone(), path("../a.txt")]),
        refused
    );
    // The accepted twins: the same files inside their own grants, and `..` within one.
    assert_eq!(
        h.value("file.read_bytes", vec![dir.clone(), path("sub/../a.txt")]),
        ok(Value::Bytes(b"abc".to_vec()))
    );
    let top = unok(h.value("io.dir", vec![io, path("")]));
    assert_eq!(
        h.value("file.read_bytes", vec![top, path("secret.txt")]),
        ok(Value::Bytes(b"secret".to_vec()))
    );
}
#[test]
fn capability_inputs_stdout_faults_test_directory_and_checks() {
    let mut h = Harness::new(SimulationConfig {
        args: vec!["a.txt".into()],
        faults: [(
            "out.print".into(),
            [
                Fault::Canceled(0),
                Fault::Fault {
                    code: 42,
                    partial: 2,
                },
            ]
            .into_iter()
            .collect(),
        )]
        .into_iter()
        .collect(),
        ..Default::default()
    });
    let io = h.root("Io");
    let out = h.project(&io, "stdout");
    assert_eq!(
        h.value("out.print", vec![out.clone(), Value::String("abc".into())]),
        error(Value::Variant {
            variant: 1,
            fields: vec![Some(Value::Variant {
                variant: 0,
                fields: vec![]
            })]
        })
    );
    assert!(h.stdout.is_empty());
    assert_eq!(
        h.value("out.print", vec![out.clone(), Value::String("abc".into())]),
        error(Value::Variant {
            variant: 0,
            fields: vec![Some(u32_value(42))]
        })
    );
    assert_eq!(h.stdout, b"ab");
    assert_eq!(
        h.value("out.print", vec![out, Value::String("é\0".into())]),
        ok(Value::Unit)
    );
    assert_eq!(h.stdout, b"ab\xc3\xa9\0");
    let args = h.root("Args");
    assert_eq!(
        h.value("arg.path", vec![args.clone(), u64_value(0)]),
        ok(path("a.txt"))
    );
    assert_eq!(
        h.value("arg.path", vec![args, u64_value(1)]),
        error(Value::Variant {
            variant: 0,
            fields: vec![]
        })
    );
    let test = h.library.simulation.grant_test();
    let dir = h.value(
        "test.dir",
        vec![
            test.clone(),
            collection::sequence(
                BuiltinType::List,
                [Value::Tuple(vec![
                    Some(path("a")),
                    Some(Value::Bytes(b"a".to_vec())),
                ])],
            ),
        ],
    );
    assert_eq!(
        h.value("file.read_bytes", vec![dir, path("a")]),
        ok(Value::Bytes(b"a".to_vec()))
    );
    assert_eq!(
        h.value("test.check", vec![test.clone(), Value::Bool(true)]),
        Value::Unit
    );
    let OperationResult::Failed(failure) = h.call(
        "test.check",
        vec![test, Value::Bool(false)],
        TypeKind::Tuple(vec![].into()),
    ) else {
        panic!()
    };
    assert_eq!(failure.rule.as_str(), "LANG-9");
    assert_eq!(failure.line, 7);
}
#[test]
fn rejected_random_replay_publishes_no_draw_and_keeps_the_seed_state() {
    let draw = |h: &mut Harness, random: &Value| {
        h.try_call(
            "random.bytes",
            vec![random.clone(), u64_value(13)],
            TypeKind::Tuple(vec![].into()),
        )
    };
    let mut h = Harness::new(SimulationConfig {
        random_seed: 42,
        ..Default::default()
    });
    let io = h.root("Io");
    let random = h.project(&io, "random");
    draw(&mut h, &random).unwrap();
    let recorded = h.library.simulation.events.clone();
    let replay = |seed| {
        let mut h = Harness::new(SimulationConfig {
            random_seed: seed,
            ..Default::default()
        });
        h.run.inputs = recorded.clone();
        h.library = Library::new(
            &h.run,
            SimulationConfig {
                random_seed: seed,
                ..Default::default()
            },
        )
        .unwrap();
        let io = h.root("Io");
        let random = h.project(&io, "random");
        let before = h.library.simulation.events.len();
        // The draw a different seed derives contradicts the tape, twice over: the
        // rejected attempt advanced neither the seed state nor the replay position.
        for _ in 0..2 {
            assert!(matches!(draw(&mut h, &random), Err(Error::InvalidTir(_))));
            assert_eq!(h.library.simulation.events.len(), before);
        }
        h.library.simulation.finish_replay()
    };
    assert!(replay(43).is_err());
    let mut h = Harness::new(SimulationConfig {
        random_seed: 42,
        ..Default::default()
    });
    h.run.inputs = recorded.clone();
    h.library = Library::new(
        &h.run,
        SimulationConfig {
            random_seed: 42,
            ..Default::default()
        },
    )
    .unwrap();
    let io = h.root("Io");
    let random = h.project(&io, "random");
    assert_eq!(
        unok(h.value("random.bytes", vec![random, u64_value(13)])),
        Value::Bytes(
            [
                &0xbdd732262feb6e95u64.to_le_bytes()[..],
                &0x28efe333b266f103u64.to_le_bytes()[..5]
            ]
            .concat()
        )
    );
    h.library.simulation.finish_replay().unwrap();
}
#[test]
fn same_seed_configuration_events_replay_and_explicit_clock_env() {
    fn execute() -> (Vec<CapabilityEvent>, Vec<u8>, Vec<u64>) {
        let mut h = Harness::new(SimulationConfig {
            random_seed: 42,
            env: [("NAME".into(), Some("value".into()))]
                .into_iter()
                .collect(),
            monotonic: 77,
            ..Default::default()
        });
        let io = h.root("Io");
        h.library
            .simulation
            .config
            .grants
            .insert("MonotonicClock".into());
        let clock = h.project(&io, "monotonic_clock");
        let Value::Variant { fields, .. } = clock else {
            panic!()
        };
        let clock = fields[0].clone().unwrap();
        let sampled = h.value("clock.monotonic", vec![clock]);
        assert_eq!(
            sampled,
            ok(Value::Handle {
                kind: "Instant".into(),
                id: 77
            })
        );
        let random = h.project(&io, "random");
        let env = h.root("Env");
        assert_eq!(
            h.value("env.get", vec![env, Value::String("NAME".into())]),
            ok(some(Value::String("value".into())))
        );
        let values = (0..10)
            .map(|_| {
                let Value::Variant { variant: 0, fields } =
                    h.value("random.bytes", vec![random.clone(), u64_value(8)])
                else {
                    panic!("splitmix64 draws never fail")
                };
                let Some(Some(Value::Bytes(bytes))) = fields.first() else {
                    panic!("random.bytes returns Bytes")
                };
                u64::from_le_bytes(bytes.as_slice().try_into().unwrap())
            })
            .collect();
        (h.library.simulation.events, h.stdout, values)
    }
    let first = execute();
    for _ in 0..100 {
        assert_eq!(execute(), first);
    }
    assert_eq!(first.2[0], 0xbdd732262feb6e95);
    let mut h = Harness::new(Default::default());
    let io = h.root("Io");
    h.project(&io, "stdout");
    let events = h.library.simulation.events.clone();
    let run = RunConfiguration {
        inputs: events.clone(),
        ..run_config()
    };
    let mut lib = Library::new(&run, Default::default()).unwrap();
    let io = lib.simulation.grant_root("Io").unwrap();
    let t = h.types.intern(ty(TypeKind::Tuple(vec![].into())));
    let path = hirsel_syntax::DefPath::core("io.stdout");
    lib.call(
        OperationCall {
            runtime: &crate::support::runtime(),
            path: &path,
            generics: &[],
            arguments: &mut [io],
            result_type: t,
            types: &h.types,
            site: Span::default(),
        },
        &mut OperationContext {
            trace_mode: Default::default(),
            anchor: None,
            configuration: &run,
            sources: &h.sources,
            stdout: &mut h.stdout,
            trace: &mut h.trace,
        },
    )
    .unwrap();
    lib.simulation.finish_replay().unwrap();
    assert_eq!(lib.simulation.events, events);
}
#[test]
fn unknown_nominal_paths_are_not_library_operations() {
    let mut h = Harness::new(Default::default());
    let t = h.types.intern(ty(TypeKind::Tuple(vec![].into())));
    let mut context = OperationContext {
        trace_mode: Default::default(),
        anchor: None,
        configuration: &h.run,
        sources: &h.sources,
        stdout: &mut h.stdout,
        trace: &mut h.trace,
    };
    let path = support::path("out.print");
    assert_eq!(
        h.library
            .call(
                OperationCall {
                    runtime: &crate::support::runtime(),
                    path: &path,
                    generics: &[],
                    arguments: &mut [],
                    result_type: t,
                    types: &h.types,
                    site: Span::default()
                },
                &mut context
            )
            .unwrap(),
        None
    );
    assert!(h.library.simulation.events.is_empty());
    assert!(h.trace.is_empty());
}
#[test]
fn replay_mismatch_cannot_publish_stdout_or_consume_fault_configuration() {
    let mut h = Harness::new(Default::default());
    let io = h.root("Io");
    h.project(&io, "stdout");
    let mut replay = h.library.simulation.events.clone();
    replay[0].operation = "io.monotonic_clock".into();
    let run = RunConfiguration {
        inputs: replay,
        ..run_config()
    };
    let mut library = Library::new(&run, Default::default()).unwrap();
    let io = library.simulation.grant_root("Io").unwrap();
    let ty = h.types.intern(ty(TypeKind::Tuple(vec![].into())));
    let path = hirsel_syntax::DefPath::core("io.stdout");
    assert!(matches!(
        library.call(
            OperationCall {
                runtime: &crate::support::runtime(),
                path: &path,
                generics: &[],
                arguments: &mut [io],
                result_type: ty,
                types: &h.types,
                site: Span::default()
            },
            &mut OperationContext {
                trace_mode: Default::default(),
                anchor: None,
                configuration: &run,
                sources: &h.sources,
                stdout: &mut h.stdout,
                trace: &mut h.trace
            }
        ),
        Err(Error::InvalidTir(_))
    ));
    assert!(library.simulation.events.is_empty());
    assert!(h.stdout.is_empty());
}
#[test]
fn cleanup_closes_files_aborts_atomic_temporaries_and_records_drop_order() {
    let mut h = Harness::new(SimulationConfig {
        files: [("keep".into(), b"old".to_vec())].into_iter().collect(),
        invalid_path_code: 71,
        ..Default::default()
    });
    let io = h.root("Io");
    let dir = unok(h.value("io.dir", vec![io, path("")]));
    assert_eq!(
        h.value("file.open", vec![dir.clone(), path("../outside")]),
        error(Value::Variant {
            variant: 0,
            fields: vec![Some(u32_value(71))]
        })
    );
    assert_eq!(
        h.value("file.open", vec![dir.clone(), path("missing")]),
        error(Value::Variant {
            variant: 3,
            fields: vec![]
        })
    );
    let file = unok(h.value("file.open", vec![dir.clone(), path("keep")]));
    let atomic = unok(h.value("file.create_atomic", vec![dir.clone(), path("keep")]));
    h.value(
        "file.write_all",
        vec![
            dir.clone(),
            atomic.clone(),
            Value::Bytes(b"partial".to_vec()),
        ],
    );
    let mut b = Builder::new();
    let files = b.parameter(TypeKind::Tuple(vec![].into()));
    let cleanup = b.cleanup(vec![support::drop(files)]);
    let unit = b.unit();
    b.block(
        Terminator::Return {
            value: support::take(unit),
            cleanup,
        },
        9,
    );
    let execution = run_with_operations(
        &mut Provider {
            definitions: vec![b.definition("main")],
            queries: vec![],
        },
        &crate::support::session(),
        &h.sources,
        &h.run,
        vec![Value::Tuple(vec![Some(file), Some(atomic)])],
        Limits::default(),
        &mut h.library,
    )
    .unwrap();
    assert!(matches!(execution.outcome, Outcome::Returned { .. }));
    let events = &h.library.simulation.events;
    assert_eq!(events[events.len() - 2].operation, "file.abort");
    assert_eq!(events[events.len() - 1].operation, "file.close");
    assert_eq!(
        h.value("file.read_bytes", vec![dir.clone(), path("keep")]),
        ok(Value::Bytes(b"old".to_vec()))
    );
    let mut walk = unok(h.value("file.walk", vec![dir]));
    assert_eq!(drain_walk(&mut h, &mut walk), vec![ok(path("keep"))]);
}
#[test]
fn simd_scalar_schema_positive_width_and_checked_integer_lanes() {
    let mut h = Harness::new(Default::default());
    let scalar = Value::Integer(n(Primitive::U8, 3));
    let lane = h.types.intern(ty(TypeKind::Primitive(Primitive::U8)));
    let result = h.types.intern(ty(TypeKind::Builtin {
        kind: BuiltinType::Simd,
        arguments: vec![
            GenericArgument::Type(lane),
            GenericArgument::Size(Size::Constant(3)),
        ]
        .into(),
    }));
    let args = vec![scalar.clone()];
    let value = h
        .library
        .call(
            OperationCall {
                runtime: &crate::support::runtime(),
                path: &hirsel_syntax::DefPath::core("simd.splat"),
                generics: &[
                    GenericArgument::Type(lane),
                    GenericArgument::Size(Size::Constant(3)),
                ],
                arguments: &mut args.to_vec(),
                result_type: result,
                types: &h.types,
                site: support::site(7),
            },
            &mut OperationContext {
                trace_mode: Default::default(),
                anchor: None,
                configuration: &h.run,
                sources: &h.sources,
                stdout: &mut h.stdout,
                trace: &mut h.trace,
            },
        )
        .unwrap()
        .unwrap();
    let OperationResult::Complete { value, .. } = value else {
        panic!()
    };
    assert_eq!(
        collection::elements(&value, BuiltinType::Simd).unwrap(),
        vec![scalar.clone(); 3]
    );
    let doubled = h.value("simd.add", vec![value.clone(), value]);
    assert_eq!(
        collection::elements(&doubled, BuiltinType::Simd).unwrap(),
        vec![Value::Integer(n(Primitive::U8, 6)); 3]
    );
    let overflow = collection::sequence(
        BuiltinType::Simd,
        [Value::Integer(n(Primitive::U8, 255)), scalar.clone()],
    );
    let ones = collection::sequence(
        BuiltinType::Simd,
        [Value::Integer(n(Primitive::U8, 1)), scalar],
    );
    assert!(
        matches!(h.call("simd.add",vec![overflow,ones],TypeKind::Tuple(vec![].into())),OperationResult::Failed(f) if f.rule.as_str()=="SEM-1" && f.line==7)
    );
    for width in [0, 3] {
        let args = vec![Value::String("invalid scalar".into())];
        let err = h.library.call(
            OperationCall {
                runtime: &crate::support::runtime(),
                path: &hirsel_syntax::DefPath::core("simd.splat"),
                generics: &[GenericArgument::Size(Size::Constant(width))],
                arguments: &mut args.to_vec(),
                result_type: result,
                types: &h.types,
                site: support::site(7),
            },
            &mut OperationContext {
                trace_mode: Default::default(),
                anchor: None,
                configuration: &h.run,
                sources: &h.sources,
                stdout: &mut h.stdout,
                trace: &mut h.trace,
            },
        );
        assert!(matches!(err, Err(Error::InvalidTir(_))));
    }
}

// Boundary pattern: rust-lang/rust a7c37f9f0ee5, core/src/num/int_macros.rs:586-587
// and uint_macros.rs:944-947 (in library/): last fitting sum / first overflow.
// MEM-8 uses SEM-1's scalar bounds. Explicit boundary vectors keep this oracle
// independent of integer::binary, which is also used by simd.add itself.
#[test]
fn simd_add_checks_every_integer_width_and_each_lane() {
    use Primitive::*;
    let mut h = Harness::new(Default::default());
    let cases = [
        (U8, 0, u8::MAX as u128),
        (U16, 0, u16::MAX as u128),
        (U32, 0, u32::MAX as u128),
        (U64, 0, u64::MAX as u128),
        (U128, 0, u128::MAX),
        (I8, i8::MIN as i128, i8::MAX as u128),
        (I16, i16::MIN as i128, i16::MAX as u128),
        (I32, i32::MIN as i128, i32::MAX as u128),
        (I64, i64::MIN as i128, i64::MAX as u128),
        (I128, i128::MIN, i128::MAX as u128),
    ];
    for (kind, min, max) in cases {
        let sequence = |values: [i128; 3]| {
            collection::sequence(
                BuiltinType::Simd,
                values.map(|value| Value::Integer(n(kind, value))),
            )
        };
        let added = h.value(
            "simd.add",
            vec![sequence([min, max as i128, 0]), sequence([0, 0, 1])],
        );
        assert_eq!(
            collection::elements(&added, BuiltinType::Simd).unwrap(),
            collection::elements(&sequence([min, max as i128, 1]), BuiltinType::Simd).unwrap(),
            "{kind:?} boundary lanes"
        );
        for lane in 0..3 {
            let mut left = [0; 3];
            let mut right = [0; 3];
            left[lane] = max as i128;
            right[lane] = 1;
            let mut boundaries = vec![(left, right)];
            if min < 0 {
                left[lane] = min;
                right[lane] = -1;
                boundaries.push((left, right));
            }
            for (left, right) in boundaries {
                let result = h.call(
                    "simd.add",
                    vec![sequence(left), sequence(right)],
                    TypeKind::Tuple(vec![].into()),
                );
                assert!(
                    matches!(result, OperationResult::Failed(ref f)
                        if f.rule.as_str() == "SEM-1" && f.line == 7),
                    "{kind:?} lane {lane}: {result:?}"
                );
            }
        }
    }
}

fn released(displaced: collection::Displaced) {
    for value in displaced.0 {
        value.discard();
    }
}
fn inserted(mut map: Value, key: Value, item: Value) -> Value {
    released(collection::insert(&mut map, key, item).unwrap());
    map
}

#[test]
fn unchecked_add_detects_overflow_without_inventing_a_language_failure() {
    let mut config = SimulationConfig::default();
    config.grants.insert("Unsafe".into());
    let mut h = Harness::new(config);
    let permit = h.root("Unsafe");
    let detected = h.call(
        "int.unchecked_add",
        vec![
            permit.clone(),
            Value::Integer(n(Primitive::U8, 200)),
            Value::Integer(n(Primitive::U8, 56)),
        ],
        TypeKind::Primitive(Primitive::U8),
    );
    let OperationResult::UndefinedBehavior(detected) = detected else {
        panic!("{detected:?}")
    };
    assert_eq!(detected.kind, "undefined behaviour detected");
    assert_eq!(
        detected.operation,
        hirsel_syntax::DefPath::core("int.unchecked_add")
    );
    assert_eq!(detected.site, support::site(7));
    assert_eq!(detected.line, 7);
    assert!(detected.contract.contains("sum must fit"));
    let execution = Execution {
        failures: vec![],
        outcome: Outcome::UndefinedBehavior(detected),
        stdout: b"prior".to_vec(),
        trace: vec![],
        steps: 0,
        charged: 0,
    };
    assert!(execution.observable(&h.sources).is_none());
    assert_eq!(
        h.call(
            "int.unchecked_add",
            vec![
                permit,
                Value::Integer(n(Primitive::U8, 200)),
                Value::Integer(n(Primitive::U8, 55))
            ],
            TypeKind::Primitive(Primitive::U8)
        ),
        OperationResult::returned(Value::Integer(n(Primitive::U8, 255)))
    );
}

#[test]
fn opened_identity_controls_cover_live_replacement_aba_and_fault_prefixes_in_each_tree() {
    for fake in [false, true] {
        for mutation in [
            "live", "replace", "unlink", "recreate", "partial", "zero", "canceled",
        ] {
            let mut h = Harness::new(SimulationConfig {
                files: BTreeMap::from([("sub/value".into(), b"data".to_vec())]),
                ..Default::default()
            });
            let dir = if fake {
                let test = h.root("Test");
                h.value(
                    "test.dir",
                    vec![
                        test,
                        collection::sequence(
                            BuiltinType::List,
                            vec![Value::Tuple(vec![
                                Some(path("sub/value")),
                                Some(Value::Bytes(b"data".to_vec())),
                            ])],
                        ),
                    ],
                )
            } else {
                let io = h.root("Io");
                unok(h.value("io.dir", vec![io, path(".")]))
            };
            let sub = unok(h.value("dir.narrow", vec![dir.clone(), path("sub")]));
            let first = unok(h.value("file.open", vec![dir.clone(), path("sub/value")]));
            let second = unok(h.value("file.open", vec![sub.clone(), path("value")]));
            // Successful narrowing transfers the original identity to a new handle.
            let first = match h.value("file.narrow", vec![dir.clone(), first]) {
                Value::Variant {
                    variant: 1,
                    mut fields,
                } => fields.remove(0).unwrap(),
                other => panic!("{other:?}"),
            };
            if matches!(mutation, "unlink" | "recreate") {
                assert_eq!(
                    h.value("file.remove", vec![dir.clone(), path("sub/value")]),
                    ok(Value::Unit)
                );
            }
            if matches!(
                mutation,
                "replace" | "recreate" | "partial" | "zero" | "canceled"
            ) {
                let fault = match mutation {
                    "partial" => Some(Fault::Fault {
                        code: 71,
                        partial: 4,
                    }),
                    "zero" => Some(Fault::Fault {
                        code: 71,
                        partial: 0,
                    }),
                    "canceled" => Some(Fault::Canceled(0)),
                    _ => None,
                };
                if let Some(fault) = fault {
                    h.library
                        .simulation
                        .config
                        .faults
                        .entry("file.write_bytes".into())
                        .or_default()
                        .push_back(fault);
                }
                h.value(
                    "file.write_bytes",
                    vec![sub.clone(), path("value"), Value::Bytes(b"data".to_vec())],
                );
            }
            let survives = matches!(mutation, "live" | "zero" | "canceled");
            for (dir, file) in [(sub.clone(), first), (sub, second)] {
                let narrowed = h.value("file.narrow", vec![dir, file]);
                assert_eq!(
                    matches!(narrowed, Value::Variant { variant: 1, .. }),
                    survives,
                    "{fake}/{mutation}"
                );
            }
        }
    }
}

#[test]
fn clock_projection_sample_errors_wall_reversal_and_monotonic_regression_are_independent() {
    let mut h = Harness::new(SimulationConfig::default());
    let io = h.root("Io");
    assert_eq!(
        h.project(&io, "monotonic_clock"),
        error(Value::Variant {
            variant: 0,
            fields: vec![]
        })
    );
    h.library
        .simulation
        .config
        .grants
        .insert("MonotonicClock".into());
    let Value::Variant { fields, .. } = h.project(&io, "monotonic_clock") else {
        panic!()
    };
    let clock = fields[0].clone().unwrap();
    assert_eq!(
        h.project(&io, "wall_clock"),
        error(Value::Variant {
            variant: 0,
            fields: vec![]
        })
    );
    h.library.simulation.config.monotonic = u64::MAX;
    assert_eq!(
        h.value("clock.monotonic", vec![clock.clone()]),
        ok(Value::Handle {
            kind: "Instant".into(),
            id: u64::MAX
        })
    );
    let before = h.library.simulation.events.clone();
    h.library.simulation.config.monotonic = 0;
    assert!(
        h.try_call(
            "clock.monotonic",
            vec![clock.clone()],
            TypeKind::Tuple(vec![].into())
        )
        .is_err()
    );
    assert_eq!(h.library.simulation.events, before);
    h.library.simulation.config.monotonic = u64::MAX;
    assert_eq!(
        h.value("clock.monotonic", vec![clock.clone()]),
        ok(Value::Handle {
            kind: "Instant".into(),
            id: u64::MAX
        })
    );
    for code in [0, 1] {
        h.library
            .simulation
            .config
            .faults
            .entry("clock.monotonic".into())
            .or_default()
            .push_back(Fault::Fault { code, partial: 0 });
        assert_eq!(
            h.value("clock.monotonic", vec![clock.clone()]),
            error(Value::Variant {
                variant: code,
                fields: vec![]
            })
        );
    }
    h.library
        .simulation
        .config
        .grants
        .insert("WallClock".into());
    let Value::Variant { fields, .. } = h.project(&io, "wall_clock") else {
        panic!()
    };
    let wall = fields[0].clone().unwrap();
    for ticks in [i64::MAX, -1, i64::MIN] {
        h.library.simulation.config.wall = ticks;
        assert_eq!(
            h.value("clock.wall", vec![wall.clone()]),
            ok(Value::Integer(n(Primitive::I64, i128::from(ticks))))
        );
    }
}

#[test]
fn configured_projection_denial_records_not_granted_without_a_handle() {
    for op in ["stdout", "tasks", "net", "random"] {
        for granted in [false, true] {
            let config = SimulationConfig {
                projections: if granted {
                    [op.to_owned()].into()
                } else {
                    Default::default()
                },
                ..Default::default()
            };
            let mut h = Harness::new(config);
            let io = h.root("Io");
            let before = h.library.simulation.observed_state();
            let result = h.value(&format!("io.{op}"), vec![io]);
            if granted {
                assert!(matches!(unok(result), Value::Handle { .. }));
            } else {
                assert_eq!(
                    result,
                    Value::Variant {
                        variant: 1,
                        fields: vec![Some(Value::Variant {
                            variant: 0,
                            fields: vec![]
                        })]
                    }
                );
                assert_eq!(h.library.simulation.observed_state(), before);
            }
            assert_eq!(h.library.simulation.events.len(), 1);
            assert!(h.stdout.is_empty());
        }
    }
}

// Source exercises legal copied Mutex handles across isolate. This operation
// boundary additionally inspects dead-state ownership and the exact cause chain.
#[test]
fn dead_mutex_releases_updated_payload_and_preserves_original_failure() {
    let mut h = Harness::new(Default::default());
    let io = h.root("Io");
    let tasks = h.project(&io, "tasks");
    let mutex = h.value("mutex.create", vec![tasks.clone(), u32_value(1)]);
    assert!(h.library.take_yield());
    let body = Value::Closure {
        invoked: Default::default(),
        definition: (&support::path("body")).into(),
        body: BodyId(0),
        captures: vec![],
        environment: Box::new(hirsel_interp::value::Environment {
            storage: None,
            capture_types: vec![],
            owned_captures: vec![],
            sizes: Default::default(),
            type_arguments: Default::default(),
        }),
    };
    let OperationResult::Call { continuation, .. } = h.call(
        "mutex.with",
        vec![tasks.clone(), mutex.clone(), body.clone()],
        TypeKind::Tuple(vec![].into()),
    ) else {
        panic!("mutex acquisition must call its body");
    };
    // The mutex is acquired: switching tasks here would expose its absent payload.
    assert!(!h.library.take_yield());
    assert!(
        h.library
            .callback_finished(continuation, &[(0, u32_value(7))], false)
            .unwrap()
            .is_empty()
    );
    let anchor = hirsel_interp::observation::Anchor {
        definition: (support::path("body")).into(),
        body: BodyId(0),
        cleanup: None,
        site: support::site(7),
    };
    let original = hirsel_interp::observation::FailureRecord {
        operation: None,
        failure: hirsel_interp::Failure {
            rule: hirsel_diag::RuleId::of("SEM-1"),
            line: 7,
            site: support::site(7),
            kind: "Overflow".into(),
        },
        anchor: anchor.clone(),
        cause: None,
        spawn_path: vec![],
    };
    let mut context = OperationContext {
        trace_mode: Default::default(),
        anchor: Some(&anchor),
        configuration: &h.run,
        sources: &h.sources,
        stdout: &mut h.stdout,
        trace: &mut h.trace,
    };
    let OperationResult::FailureChain { failures, released } = h
        .library
        .resume(
            continuation,
            OperationResult::FailureChain {
                failures: vec![original.clone()],
                released: vec![],
            },
            &mut [],
            &mut context,
        )
        .unwrap()
    else {
        panic!("mutex failure must propagate");
    };
    assert_eq!(failures, vec![original.clone()]);
    assert_eq!(released, vec![u32_value(7)]);
    assert!(h.library.take_yield());
    let path = hirsel_syntax::DefPath::core("mutex.with");
    let result_type = h.types.intern(ty(TypeKind::Tuple(vec![].into())));
    let OperationResult::FailureChain { failures, released } = h
        .library
        .call(
            OperationCall {
                runtime: &crate::support::runtime(),
                path: &path,
                generics: &[],
                arguments: &mut [tasks, mutex, body],
                result_type,
                types: &h.types,
                site: support::site(7),
            },
            &mut context,
        )
        .unwrap()
        .unwrap()
    else {
        panic!("dead mutex must fail without entering its body");
    };
    assert!(released.is_empty());
    assert_eq!(failures.len(), 2);
    assert_eq!(failures[0].failure.rule.as_str(), "FAIL-4");
    assert_eq!(failures[0].failure.kind, "DeadMutex");
    assert_eq!(failures[0].cause, Some(1));
    assert_eq!(failures[1], original);
}

#[test]
fn argument_snapshot_preserves_empty_unicode_and_option_like_text() {
    let texts = vec![String::new(), "λ🐑".into(), "--flag".into()];
    let mut h = Harness::new(SimulationConfig {
        args: texts.clone(),
        ..Default::default()
    });
    let args = h.root("Args");
    assert_eq!(h.value("arg.len", vec![args.clone()]), u64_value(3));
    for (i, text) in texts.into_iter().enumerate() {
        assert_eq!(
            h.value("arg.text", vec![args.clone(), u64_value(i as u64)]),
            ok(Value::String(text))
        );
    }
    for index in [3, u64::MAX] {
        assert_eq!(
            h.value("arg.text", vec![args.clone(), u64_value(index)]),
            error(Value::Variant {
                variant: 0,
                fields: vec![]
            })
        );
    }
}

// A seeded scheduler is no conformance header. The interpreter's choices follow the
// policy kernel's draws (random@1 seed 7: words mod 2, then mod 3), a lone candidate draws
// nothing, and the run record's scheduler words name the kernel's policies.
#[test]
fn seeded_scheduler_choices_follow_the_policy_and_its_words() {
    use hirsel_kernels::schedule::Algorithm;
    use hirsel_run_schema::Scheduler;
    let h = Harness::new(Default::default());
    let mut library = Library::new(
        &h.run,
        SimulationConfig {
            scheduler: Scheduler::parse("random@1:7"),
            ..Default::default()
        },
    )
    .unwrap();
    let chosen: Vec<usize> = [&[4u64, 9][..], &[4], &[4, 9, 12]]
        .into_iter()
        .map(|runnable| library.choose_task(runnable).unwrap())
        .collect();
    assert_eq!(chosen, [1, 0, 0]);
    assert!(library.schedule_recorded(2) && !library.schedule_recorded(1));
    let fifo = Library::new(&h.run, Default::default()).unwrap();
    assert!(!fifo.schedule_recorded(3));
    for (name, word) in Scheduler::CONTRACTS {
        let expected = match name {
            "fifo" => Algorithm::Fifo,
            "random" => Algorithm::Random,
            _ => Algorithm::Pct,
        };
        assert_eq!(Algorithm::from_code(word), Some(expected), "{name}");
    }
}

// Scheduling input cannot be expressed by a conformance header. A recorded
// runnable choice must control replay; an unavailable task is not a substitute.
#[test]
fn recorded_scheduler_choices_select_runnable_tasks_and_reject_blocked_ones() {
    let mut h = Harness::new(Default::default());
    h.run.inputs = vec![CapabilityEvent {
        capability: hirsel_syntax::DefPath::core("Tasks"),
        operation: "task.resume".into(),
        input: 9u64.to_le_bytes().to_vec(),
        output: vec![],
    }];
    h.library = Library::new(&h.run, Default::default()).unwrap();
    assert!(matches!(
        h.library.choose_task(&[4]),
        Err(Error::InvalidTir(_))
    ));
    assert!(h.library.simulation.events.is_empty());
    assert_eq!(h.library.choose_task(&[4, 9]).unwrap(), 1);
    let anchor = hirsel_interp::observation::Anchor {
        definition: (support::path("main")).into(),
        body: BodyId(0),
        cleanup: None,
        site: support::site(7),
    };
    h.library
        .task_resumed(
            9,
            &mut OperationContext {
                trace_mode: Default::default(),
                anchor: Some(&anchor),
                configuration: &h.run,
                sources: &h.sources,
                stdout: &mut h.stdout,
                trace: &mut h.trace,
            },
        )
        .unwrap();
    h.library.simulation.finish_replay().unwrap();
    assert_eq!(h.library.simulation.events, h.run.inputs);
}

#[test]
fn stdin_replay_rejects_oversized_chunks_and_invalid_errors_before_publication() {
    let config = SimulationConfig {
        stdin: vec![0, 255],
        ..Default::default()
    };
    let mut recorded = Harness::new(config.clone());
    let io = recorded.root("Io");
    let input = unok(recorded.value("io.stdin", vec![io]));
    let buffer = Value::Sequence {
        kind: BuiltinType::List,
        elements: vec![Some(Value::Integer(n(Primitive::U8, 7)))].into(),
        arena: None,
    };
    recorded.value("stdin.read", vec![input, buffer.clone(), u64_value(2)]);
    let tape = recorded.library.simulation.events.clone();
    let mut too_long = vec![0];
    too_long.extend_from_slice(&3u64.to_le_bytes());
    too_long.extend_from_slice(&[0, 255, 1]);
    let mut invalid_cause = vec![1];
    invalid_cause.extend_from_slice(&1u32.to_le_bytes());
    invalid_cause.extend_from_slice(&4u32.to_le_bytes());
    for bad_output in [too_long, invalid_cause, vec![0]] {
        let mut h = Harness::new(Default::default());
        h.run.inputs = tape.clone();
        h.run.inputs.last_mut().unwrap().output = bad_output;
        h.library = Library::new(&h.run, SimulationConfig::default()).unwrap();
        let io = h.root("Io");
        let input = unok(h.value("io.stdin", vec![io]));
        assert!(
            h.try_call(
                "stdin.read",
                vec![input, buffer.clone(), u64_value(2)],
                TypeKind::Tuple(vec![].into())
            )
            .is_err()
        );
        assert_eq!(h.library.simulation.events.len(), 1);
    }
}

#[test]
fn process_replay_returns_recorded_completions_and_refuses_contradicting_ones() {
    use hirsel_run_schema::{ExitStatus, ProcessGrant, ProcessOutcome, ProcessRule, ProcessRun};
    let grant = |runs: Vec<ProcessRun>| SimulationConfig {
        process: Some(ProcessGrant {
            timed: None,
            provider: "process.script@1".into(),
            rules: vec![ProcessRule {
                program: "/usr/bin/git".into(),
                env: vec![],
            }],
            runs: Some(runs),
            children: None,
        }),
        ..Default::default()
    };
    let scripted = grant(vec![ProcessRun {
        program: "/usr/bin/git".into(),
        args: vec!["status".into()],
        outcome: ProcessOutcome::Output {
            status: ExitStatus::Exited(3),
            stdout: b"clean".to_vec(),
            stderr: vec![],
        },
    }]);
    let run = |h: &mut Harness, program: &str| {
        let io = h.root("Io");
        let p = unok(h.value("io.process", vec![io]));
        let args = collection::sequence(BuiltinType::List, [Value::String("status".into())]);
        h.try_call(
            "proc.run",
            vec![
                p,
                Value::Record {
                    definition: (&hirsel_syntax::DefPath::core("Command")).into(),
                    fields: vec![
                        Some(path(program)),
                        Some(args),
                        Some(none()),
                        Some(none()),
                        Some(Value::Bool(false)),
                    ],
                },
                Value::Bytes(vec![]),
                u64_value(64),
                none(),
            ],
            TypeKind::Tuple(vec![].into()),
        )
    };
    let mut recorded = Harness::new(scripted);
    let first = run(&mut recorded, "/usr/bin/git").unwrap();
    let OperationResult::Complete { value, .. } = &first else {
        panic!("{first:?}")
    };
    assert_eq!(
        *value,
        process::outcome_value(&ProcessOutcome::Output {
            status: ExitStatus::Exited(3),
            stdout: b"clean".to_vec(),
            stderr: vec![],
        })
    );
    let tape = recorded.library.simulation.events.clone();
    // Replay never consults the script: an empty one still returns the recording.
    let mut h = Harness::new(Default::default());
    h.run.inputs = tape.clone();
    h.library = Library::new(&h.run, grant(vec![])).unwrap();
    let OperationResult::Complete {
        value: replayed, ..
    } = run(&mut h, "/usr/bin/git").unwrap()
    else {
        panic!()
    };
    assert_eq!(&replayed, value);
    // A granted run recorded as NotGranted, or an output past its bound, contradicts
    // the re-derived checks and publishes nothing.
    for forged in [
        process::encode_completion(&ProcessOutcome::Error(
            hirsel_run_schema::ProcessFailure::NotGranted,
        )),
        process::encode_completion(&ProcessOutcome::Output {
            status: ExitStatus::Exited(0),
            stdout: vec![0; 65],
            stderr: vec![],
        }),
        // A timeout needs a deadline, and a cancellation an uncancelled scope cannot give.
        process::encode_completion(&ProcessOutcome::Error(
            hirsel_run_schema::ProcessFailure::TimedOut,
        )),
        process::encode_completion(&ProcessOutcome::Error(
            hirsel_run_schema::ProcessFailure::Canceled(0),
        )),
        vec![2],
    ] {
        let mut h = Harness::new(Default::default());
        h.run.inputs = tape.clone();
        h.run.inputs.last_mut().unwrap().output = forged;
        h.library = Library::new(&h.run, grant(vec![])).unwrap();
        assert!(run(&mut h, "/usr/bin/git").is_err());
        assert_eq!(h.library.simulation.events.len(), 1);
    }
}

#[test]
fn filesystem_snapshot_metadata_survives_initialization_and_failed_create() {
    use simulation::FileMetadata;
    let mut h = Harness::new(SimulationConfig {
        files: BTreeMap::from([("value".into(), b"x".to_vec())]),
        directories: [String::new(), "empty".into()].into_iter().collect(),
        metadata: BTreeMap::from([
            (
                "value".into(),
                FileMetadata {
                    kind: 0,
                    size: 1,
                    modified_unix_ns: Some(-1),
                    permissions: 0o666,
                },
            ),
            (
                "empty".into(),
                FileMetadata {
                    kind: 1,
                    size: 0,
                    modified_unix_ns: Some(-2),
                    permissions: 0o777,
                },
            ),
        ]),
        ..Default::default()
    });
    let io = h.root("Io");
    let d = unok(h.value("io.dir", vec![io, path("")]));
    let metadata = |value: Value| {
        let Value::Record { mut fields, .. } = unok(value) else {
            panic!()
        };
        let Value::Variant {
            variant,
            mut fields,
        } = fields.remove(2).unwrap()
        else {
            panic!()
        };
        if variant == 0 {
            None
        } else {
            let Value::Integer(n) = fields.remove(0).unwrap() else {
                panic!()
            };
            Some(n.bits() as i128)
        }
    };
    assert_eq!(
        metadata(h.value("file.metadata", vec![d.clone(), path("value")])),
        Some(-1)
    );
    assert!(matches!(
        h.value("file.create", vec![d.clone(), path("empty")]),
        Value::Variant { variant: 1, .. }
    ));
    assert_eq!(
        metadata(h.value("file.metadata", vec![d.clone(), path("empty")])),
        Some(-2)
    );
    let _ = h.value("file.create", vec![d.clone(), path("value")]);
    assert_eq!(
        metadata(h.value("file.metadata", vec![d, path("value")])),
        None
    );
}

#[test]
fn output_bytes_capture_partial_fault_cancellation_and_replay() {
    fn variant(variant: u32, fields: Vec<Value>) -> Value {
        Value::Variant {
            variant,
            fields: fields.into_iter().map(Some).collect(),
        }
    }
    let config = SimulationConfig {
        faults: ["out.write", "err.write"]
            .into_iter()
            .map(|name| {
                (
                    name.into(),
                    [
                        Fault::Canceled(0),
                        Fault::Fault {
                            code: 42,
                            partial: 2,
                        },
                    ]
                    .into_iter()
                    .collect(),
                )
            })
            .collect(),
        ..Default::default()
    };
    let execute = |h: &mut Harness| {
        let io = h.root("Io");
        for (projection, name) in [("stdout", "out.write"), ("stderr", "err.write")] {
            let stream = unok(h.value(&format!("io.{projection}"), vec![io.clone()]));
            let data = Value::Bytes(vec![0xff, 0, 0x80, 0xfe]);
            assert_eq!(
                h.value(name, vec![stream.clone(), data.clone()]),
                error(variant(1, vec![variant(0, vec![])]))
            );
            assert_eq!(
                h.value(name, vec![stream.clone(), data.clone()]),
                error(variant(0, vec![u32_value(42)]))
            );
            assert_eq!(h.value(name, vec![stream.clone(), data]), ok(Value::Unit));
            assert_eq!(
                h.value(name, vec![stream, Value::Bytes(vec![])]),
                ok(Value::Unit)
            );
        }
        assert_eq!(h.stdout, [0xff, 0, 0xff, 0, 0x80, 0xfe]);
        assert_eq!(h.library.simulation.stderr, h.stdout);
    };
    let mut live = Harness::new(config.clone());
    execute(&mut live);
    let mut replay = Harness::new(config);
    replay.run.inputs = live.library.simulation.events.clone();
    replay.library = Library::new(&replay.run, replay.library.simulation.config.clone()).unwrap();
    execute(&mut replay);
    replay.library.simulation.finish_replay().unwrap();
    assert_eq!(
        live.library.simulation.events,
        replay.library.simulation.events
    );
}

fn drain_walk(h: &mut Harness, walk: &mut Value) -> Vec<Value> {
    let mut items = vec![];
    loop {
        let OperationResult::Complete {
            value, arguments, ..
        } = h
            .try_call(
                "iter.next",
                vec![walk.clone()],
                TypeKind::Tuple(vec![].into()),
            )
            .unwrap()
        else {
            panic!("walk step")
        };
        *walk = arguments.into_iter().find(|(i, _)| *i == 0).unwrap().1;
        match value {
            Value::Variant { variant: 0, .. } => break,
            Value::Variant {
                variant: 1,
                mut fields,
            } => items.push(fields.pop().unwrap().unwrap()),
            _ => panic!("walk option"),
        }
    }
    items
}

#[test]
fn walk_stream_generated_successes_equal_the_old_eager_sort_and_skip_empty_directories() {
    for seed in 0..8 {
        let mut files = BTreeMap::new();
        for name in [
            "a-", "a.c", "a/b-", "a/b.c", "a/b/d", "a0", "é/x", "中", "z",
        ] {
            files.insert(format!("{seed}/{name}"), vec![]);
        }
        files.insert(
            format!("{}/leaf", "deep/".repeat(32).trim_end_matches('/')),
            vec![],
        );
        let expected: Vec<_> = files.keys().map(|p| ok(path(p))).collect();
        let mut h = Harness::new(SimulationConfig {
            files,
            directories: [String::new(), "empty".into()].into(),
            ..Default::default()
        });
        let io = h.root("Io");
        let dir = unok(h.value("io.dir", vec![io, path("")]));
        let mut walk = unok(h.value("file.walk", vec![dir]));
        assert_eq!(drain_walk(&mut h, &mut walk), expected);
    }
}
#[test]
fn walk_stream_symlink_cycles_are_entries_and_targets_are_never_resolved() {
    let mut h = Harness::new(SimulationConfig {
        files: [("real/leaf".into(), vec![])].into(),
        links: [
            ("alias".into(), "real".into()),
            ("a".into(), "b".into()),
            ("b".into(), "a".into()),
            ("self".into(), ".".into()),
            ("escape".into(), "../outside".into()),
        ]
        .into(),
        ..Default::default()
    });
    let io = h.root("Io");
    let dir = unok(h.value("io.dir", vec![io, path("")]));
    let mut walk = unok(h.value("file.walk", vec![dir]));
    assert_eq!(
        drain_walk(&mut h, &mut walk),
        ["a", "alias", "b", "escape", "real/leaf", "self"].map(|p| ok(path(p)))
    );
}
#[test]
fn walk_stream_descendant_error_is_late_and_fused_while_root_error_is_outer() {
    let mut h = Harness::new(SimulationConfig {
        files: [("a".into(), vec![]), ("z/leaf".into(), vec![])].into(),
        ..Default::default()
    });
    let io = h.root("Io");
    let dir = unok(h.value("io.dir", vec![io, path("")]));
    let mut walk = unok(h.value("file.walk", vec![dir.clone()]));
    h.library.simulation.config.faults.insert(
        "file.read_dir".into(),
        [Fault::File {
            error: simulation::FileError::PermissionDenied,
            partial: 0,
        }]
        .into(),
    );
    let items = drain_walk(&mut h, &mut walk);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0], ok(path("a")));
    assert!(matches!(items[1], Value::Variant { variant: 1, .. }));
    assert!(drain_walk(&mut h, &mut walk).is_empty());
    h.library.simulation.config.faults.insert(
        "file.walk".into(),
        [Fault::File {
            error: simulation::FileError::PermissionDenied,
            partial: 0,
        }]
        .into(),
    );
    assert!(matches!(
        h.value("file.walk", vec![dir]),
        Value::Variant { variant: 1, .. }
    ));
}

#[test]
fn walk_stream_replay_refusal_preserves_cursor_faults_and_prior_events() {
    let config = SimulationConfig {
        files: [("z/leaf".into(), vec![])].into(),
        faults: [(
            "file.read_dir".into(),
            [Fault::File {
                error: simulation::FileError::PermissionDenied,
                partial: 0,
            }]
            .into(),
        )]
        .into(),
        ..Default::default()
    };
    let mut live = Harness::new(config.clone());
    let io = live.root("Io");
    let dir = unok(live.value("io.dir", vec![io, path("")]));
    let mut walk = unok(live.value("file.walk", vec![dir]));
    let expected = drain_walk(&mut live, &mut walk);
    let events = live.library.simulation.events.clone();
    for forged in [false, true] {
        let mut replay = Harness::new(config.clone());
        replay.run.inputs = events.clone();
        if forged {
            replay.run.inputs.last_mut().unwrap().output.push(255);
        }
        replay.library = Library::new(&replay.run, config.clone()).unwrap();
        let io = replay.root("Io");
        let dir = unok(replay.value("io.dir", vec![io, path("")]));
        let mut walk = unok(replay.value("file.walk", vec![dir]));
        if forged {
            let prior = replay.library.simulation.events.clone();
            let faults = replay.library.simulation.config.faults.clone();
            assert!(
                replay
                    .try_call(
                        "iter.next",
                        vec![walk.clone()],
                        TypeKind::Tuple(vec![].into())
                    )
                    .is_err()
            );
            assert_eq!(replay.library.simulation.events, prior);
            assert_eq!(replay.library.simulation.config.faults, faults);
        } else {
            assert_eq!(drain_walk(&mut replay, &mut walk), expected);
            replay.library.simulation.finish_replay().unwrap();
        }
    }
}

#[test]
fn accumulated_file_reads_record_only_the_request_and_new_bytes() {
    for reads in [2, 4, 8] {
        let mut h = Harness::new(SimulationConfig {
            files: [("root/input".into(), vec![42; reads * 64])].into(),
            ..Default::default()
        });
        let io = h.root("Io");
        let dir = unok(h.value("io.dir", vec![io, path("root")]));
        let file = unok(h.value("file.open", vec![dir.clone(), path("input")]));
        let storage = Vec::with_capacity(reads * 64);
        let expected_storage = storage.as_ptr();
        let mut buffer = Value::Sequence {
            kind: BuiltinType::List,
            elements: storage.into(),
            arena: None,
        };
        let mut input_sizes = vec![];
        for _ in 0..reads {
            let result = h.call(
                "file.read",
                vec![dir.clone(), file.clone(), buffer, u64_value(64)],
                TypeKind::Tuple(vec![].into()),
            );
            let OperationResult::Complete {
                value,
                mut arguments,
                ..
            } = result
            else {
                panic!("{result:?}")
            };
            assert_eq!(value, ok(u64_value(64)));
            buffer = arguments.remove(0).1;
            let Value::Sequence { elements, .. } = &buffer else {
                unreachable!()
            };
            assert_eq!(
                elements.as_ptr(),
                expected_storage,
                "file.read cloned the accumulated buffer"
            );
            input_sizes.push(h.library.simulation.events.last().unwrap().input.len());
        }
        assert_eq!(collection::len(&buffer).unwrap(), reads as u64 * 64);
        assert!(
            input_sizes.windows(2).all(|pair| pair[0] == pair[1]),
            "accumulated buffers leaked into recorded requests: {input_sizes:?}"
        );
    }
}

#[test]
fn accumulated_file_reads_replay_exactly_and_preserve_shared_prefixes() {
    let config = SimulationConfig {
        files: [("input".into(), vec![1, 2, 3, 4])].into(),
        ..Default::default()
    };
    let execute = |h: &mut Harness| {
        let io = h.root("Io");
        let dir = unok(h.value("io.dir", vec![io, path(".")]));
        let file = unok(h.value("file.open", vec![dir.clone(), path("input")]));
        let mut buffer =
            collection::sequence(BuiltinType::List, [Value::Integer(n(Primitive::U8, 9))]);
        for expected in [2, 2, 0] {
            let prefix = buffer.clone();
            let OperationResult::Complete {
                value,
                mut arguments,
                ..
            } = h.call(
                "file.read",
                vec![dir.clone(), file.clone(), buffer, u64_value(2)],
                TypeKind::Tuple(vec![].into()),
            )
            else {
                panic!("file read result")
            };
            assert_eq!(value, ok(u64_value(expected)));
            buffer = arguments.remove(0).1;
            let Value::Sequence {
                elements: before, ..
            } = &prefix
            else {
                unreachable!()
            };
            let Value::Sequence {
                elements: after, ..
            } = &buffer
            else {
                unreachable!()
            };
            assert_eq!(&after[..before.len()], &before[..]);
        }
        assert_eq!(collection::len(&buffer).unwrap(), 5);
        buffer
    };
    let mut live = Harness::new(config.clone());
    let expected = execute(&mut live);
    let mut replay = Harness::new(config);
    replay.run.inputs = live.library.simulation.events.clone();
    replay.library = Library::new(&replay.run, replay.library.simulation.config.clone()).unwrap();
    assert_eq!(execute(&mut replay), expected);
    replay.library.simulation.finish_replay().unwrap();
    assert_eq!(
        live.library.simulation.events,
        replay.library.simulation.events
    );
}

#[test]
fn pending_file_read_keeps_accumulated_storage_through_poll_and_release() {
    let mut h = Harness::new(SimulationConfig {
        files: [("input".into(), vec![1, 2])].into(),
        file_reads: [(
            "input".into(),
            FileReadScript::AfterWrite {
                path: "release".into(),
            },
        )]
        .into(),
        ..Default::default()
    });
    let io = h.root("Io");
    let dir = unok(h.value("io.dir", vec![io, path("")]));
    let file = unok(h.value("file.open", vec![dir.clone(), path("input")]));
    let mut storage = Vec::with_capacity(8);
    storage.push(Some(Value::Integer(n(Primitive::U8, 9))));
    let pointer = storage.as_ptr();
    let buffer = Value::Sequence {
        kind: BuiltinType::List,
        elements: storage.into(),
        arena: None,
    };
    let OperationResult::Park { token, .. } = h.call(
        "file.read",
        vec![dir.clone(), file, buffer, u64_value(2)],
        TypeKind::Tuple(vec![].into()),
    ) else {
        panic!("read never parked")
    };
    let pending_size = h.library.simulation.events.last().unwrap().input.len();
    let poll = |h: &mut Harness| {
        h.library
            .poll(
                token,
                &mut OperationContext {
                    trace_mode: Default::default(),
                    anchor: None,
                    configuration: &h.run,
                    sources: &h.sources,
                    stdout: &mut h.stdout,
                    trace: &mut h.trace,
                },
            )
            .unwrap()
    };
    assert!(poll(&mut h).is_none());
    assert!(poll(&mut h).is_none());
    assert_eq!(
        h.value(
            "file.write_bytes",
            vec![dir, path("release"), Value::Bytes(vec![])]
        ),
        ok(Value::Unit)
    );
    let Some(OperationResult::Complete {
        value,
        mut arguments,
        ..
    }) = poll(&mut h)
    else {
        panic!("read never released")
    };
    assert_eq!(value, ok(u64_value(2)));
    assert_eq!(
        pending_size,
        h.library.simulation.events.last().unwrap().input.len()
    );
    let buffer = arguments.remove(0).1;
    let Value::Sequence { elements, .. } = &buffer else {
        panic!("read buffer")
    };
    assert_eq!(
        elements.as_ptr(),
        pointer,
        "parked read cloned accumulated bytes"
    );
    assert_eq!(collection::len(&buffer).unwrap(), 3);
    assert_eq!(elements[0], Some(Value::Integer(n(Primitive::U8, 9))));
    assert_eq!(elements[1], Some(Value::Integer(n(Primitive::U8, 1))));
    assert_eq!(elements[2], Some(Value::Integer(n(Primitive::U8, 2))));
}

#[test]
fn read_dir_stream_keeps_direct_entries_sorted_across_pages_and_reports_refill_errors_late() {
    let mut h = Harness::new(SimulationConfig {
        files: (0..270)
            .map(|n| (format!("{n:04}"), vec![]))
            .chain([("a/leaf".into(), vec![])])
            .collect(),
        ..Default::default()
    });
    let io = h.root("Io");
    let dir = unok(h.value("io.dir", vec![io, path("")]));
    let mut entries = unok(h.value("file.read_dir", vec![dir.clone(), path("")]));
    let items = drain_walk(&mut h, &mut entries);
    assert_eq!(items.len(), 271);
    for (n, item) in items.iter().take(270).enumerate() {
        let Value::Variant {
            variant: 0, fields, ..
        } = item
        else {
            panic!("{item:?}")
        };
        let Value::Record { fields, .. } = fields[0].as_ref().unwrap() else {
            panic!()
        };
        assert_eq!(fields[0], Some(path(&format!("{n:04}"))));
    }
    let mut entries = unok(h.value("file.read_dir", vec![dir.clone(), path("")]));
    h.library.simulation.config.faults.insert(
        "file.read_dir".into(),
        [Fault::File {
            error: simulation::FileError::PermissionDenied,
            partial: 0,
        }]
        .into(),
    );
    let items = drain_walk(&mut h, &mut entries);
    assert_eq!(items.len(), 129);
    assert!(matches!(
        items.last(),
        Some(Value::Variant { variant: 1, .. })
    ));
    assert!(drain_walk(&mut h, &mut entries).is_empty());
    assert!(matches!(
        h.value("file.read_dir", vec![dir, path("missing")]),
        Value::Variant { variant: 1, .. }
    ));
}
#[test]
fn timed_a02_reference_foreign_domain_consumes_only_actual_child() {
    use hirsel_run_schema::{
        ExitStatus, ProcessGrant, ProcessRule, TimedChildRun, TimedChildStep as S,
        TimedProcessScript,
    };
    let process = ProcessGrant {
        provider: "process.script.timed@1".into(),
        rules: vec![ProcessRule {
            program: "/bin/cat".into(),
            env: vec![],
        }],
        runs: Some(vec![]),
        children: None,
        timed: Some(TimedProcessScript {
            clock: "run.monotonic".into(),
            children: vec![TimedChildRun {
                program: "/bin/cat".into(),
                args: vec![],
                steps: vec![
                    S::At(5),
                    S::Print(b"x".to_vec()),
                    S::Exit(ExitStatus::Exited(0)),
                ],
            }],
        }),
    };
    let mut config = SimulationConfig {
        process: Some(process),
        ..Default::default()
    };
    config.grants.insert("MonotonicClock".into());
    let mut h = Harness::new(config);
    let io = h.root("Io");
    let p = unok(h.value("io.process", vec![io]));
    let test = h.root("Test");
    let clock = h.value("test.clock", vec![test]);
    let command = Value::Record {
        definition: (&hirsel_syntax::DefPath::core("Command")).into(),
        fields: vec![
            Some(path("/bin/cat")),
            Some(collection::sequence(BuiltinType::List, [])),
            Some(none()),
            Some(none()),
            Some(Value::Bool(false)),
        ],
    };
    let child = unok(h.value("proc.spawn", vec![p.clone(), command]));
    let at = Value::Handle {
        kind: "Instant".into(),
        id: 5,
    };
    let deadline = Value::Record {
        definition: (&hirsel_syntax::DefPath::core("Deadline")).into(),
        fields: vec![Some(clock), Some(at)],
    };
    assert_eq!(
        h.value("child.wait", vec![p.clone(), child.clone(), some(deadline)]),
        error(process::failure_value(
            hirsel_run_schema::ProcessFailure::InvalidInput
        ))
    );
    assert!(h.stdout.is_empty());
    assert!(
        h.try_call(
            "child.kill",
            vec![
                p,
                child,
                Value::Variant {
                    variant: 0,
                    fields: vec![]
                }
            ],
            TypeKind::Tuple(vec![].into())
        )
        .is_err()
    );
}
