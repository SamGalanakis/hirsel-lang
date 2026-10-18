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
    types: TypeStore,
    sources: SourceMap,
    stdout: Vec<u8>,
    trace: Vec<TraceEvent>,
    run: RunConfiguration,
}
impl Harness {
    fn new(config: SimulationConfig) -> Self {
        let run = run_config();
        let mut sources = SourceMap::default();
        sources.add("library.hir", "\n".repeat(20)).unwrap();
        Self {
            library: Library::new(&run, config).unwrap(),
            types: TypeStore::default(),
            sources,
            stdout: vec![],
            trace: vec![],
            run,
        }
    }
    fn try_call(
        &mut self,
        name: &str,
        args: Vec<Value>,
        result: TypeKind,
    ) -> Result<OperationResult, Error> {
        let result = self.types.push(ty(result));
        let path = hirsel_syntax::DefPath::core(name);
        let operation = self
            .library
            .call(
                OperationCall {
                    runtime: &Default::default(),
                    path: &path,
                    generics: &[],
                    arguments: &args,
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
    let m = updated(
        collection::insert(
            updated(collection::insert(empty.clone(), a.clone(), u64_value(1)).unwrap()),
            b.clone(),
            u64_value(2),
        )
        .unwrap(),
    );
    let changed = updated(collection::insert(m.clone(), a.clone(), u64_value(3)).unwrap());
    let entries = collection::elements(&changed, BuiltinType::Map).unwrap();
    assert_eq!(
        entries[0],
        Value::Tuple(vec![Some(a.clone()), Some(u64_value(3))])
    );
    assert_eq!(
        collection::elements(&m, BuiltinType::Map).unwrap()[0],
        Value::Tuple(vec![Some(a.clone()), Some(u64_value(1))])
    );
    let reversed = updated(
        collection::insert(
            updated(collection::insert(empty, b, u64_value(2)).unwrap()),
            a,
            u64_value(3),
        )
        .unwrap(),
    );
    assert!(!collection::data_equal(&changed, &reversed));
    let set = updated(
        collection::set_insert(collection::sequence(BuiltinType::Set, []), u64_value(1)).unwrap(),
    );
    assert_eq!(
        updated(collection::set_insert(set.clone(), u64_value(1)).unwrap()),
        set
    );
    assert!(!collection::data_equal(&f(f64::NAN), &f(f64::NAN)));
    assert!(collection::data_equal(&f(-0.0), &f(0.0)));
    assert_eq!(
        collection::len(&updated(
            collection::push(collection::sequence(BuiltinType::List, []), u64_value(1)).unwrap()
        ))
        .unwrap(),
        1
    );
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
    let walk = unok(h.value("file.walk", vec![dir.clone()]));
    assert_eq!(
        collection::elements(&walk, BuiltinType::Iter).unwrap(),
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
        let mut context = OperationContext {
            trace_mode: Default::default(),
            anchor: None,
            configuration: &h.run,
            sources: &h.sources,
            stdout: &mut h.stdout,
            trace: &mut h.trace,
        };

        let values = (0..10)
            .map(|_| {
                h.library
                    .simulation
                    .random_u64(&random, &mut context)
                    .unwrap()
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
    let t = h.types.push(ty(TypeKind::Tuple(vec![].into())));
    let path = hirsel_syntax::DefPath::core("io.stdout");
    lib.call(
        OperationCall {
            runtime: &Default::default(),
            path: &path,
            generics: &[],
            arguments: &[io],
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
fn m2_and_unknown_nominal_paths_have_distinct_unavailable_outcomes() {
    let mut h = Harness::new(Default::default());
    let t = h.types.push(ty(TypeKind::Tuple(vec![].into())));
    let path = hirsel_syntax::DefPath::core("arena.seal");
    let mut context = OperationContext {
        trace_mode: Default::default(),
        anchor: None,
        configuration: &h.run,
        sources: &h.sources,
        stdout: &mut h.stdout,
        trace: &mut h.trace,
    };
    assert!(matches!(
        h.library.call(
            OperationCall {
                runtime: &Default::default(),
                path: &path,
                generics: &[],
                arguments: &[],
                result_type: t,
                types: &h.types,
                site: Span::default()
            },
            &mut context
        ),
        Err(Error::Unavailable(_))
    ));
    let path = support::path("out.print");
    assert_eq!(
        h.library
            .call(
                OperationCall {
                    runtime: &Default::default(),
                    path: &path,
                    generics: &[],
                    arguments: &[],
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
    let ty = h.types.push(ty(TypeKind::Tuple(vec![].into())));
    let path = hirsel_syntax::DefPath::core("io.stdout");
    assert!(matches!(
        library.call(
            OperationCall {
                runtime: &Default::default(),
                path: &path,
                generics: &[],
                arguments: &[io],
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
    let walk = unok(h.value("file.walk", vec![dir]));
    assert_eq!(
        collection::elements(&walk, BuiltinType::Iter).unwrap(),
        vec![ok(path("keep"))]
    );
}
#[test]
fn simd_scalar_schema_positive_width_and_checked_integer_lanes() {
    let mut h = Harness::new(Default::default());
    let scalar = Value::Integer(n(Primitive::U8, 3));
    let lane = h.types.push(ty(TypeKind::Primitive(Primitive::U8)));
    let result = h.types.push(ty(TypeKind::Builtin {
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
                runtime: &Default::default(),
                path: &hirsel_syntax::DefPath::core("simd.splat"),
                generics: &[
                    GenericArgument::Type(lane),
                    GenericArgument::Size(Size::Constant(3)),
                ],
                arguments: &args,
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
                runtime: &Default::default(),
                path: &hirsel_syntax::DefPath::core("simd.splat"),
                generics: &[GenericArgument::Size(Size::Constant(width))],
                arguments: &args,
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

fn updated(mutation: collection::Mutation) -> Value {
    let OperationResult::Complete {
        mut arguments,
        released,
        ..
    } = mutation.into_result(0, vec![])
    else {
        unreachable!()
    };
    for value in released {
        value.discard();
    }
    arguments.pop().unwrap().1
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

// Safe source cannot retain a non-sendable mutex across isolate (#22). Exercise
// the runtime boundary directly to inspect dead-state ownership and causes.
#[test]
fn dead_mutex_releases_updated_payload_and_preserves_original_failure() {
    let mut h = Harness::new(Default::default());
    let io = h.root("Io");
    let tasks = h.project(&io, "tasks");
    let mutex = h.value("mutex.create", vec![tasks.clone(), u32_value(1)]);
    assert!(h.library.take_yield());
    let body = Value::Closure {
        invoked: Default::default(),
        definition: support::path("body"),
        body: BodyId(0),
        captures: vec![],
        owned_captures: vec![],
        sizes: Default::default(),
        type_arguments: Default::default(),
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
        definition: support::path("body"),
        body: BodyId(0),
        cleanup: None,
        site: support::site(7),
    };
    let original = hirsel_interp::observation::FailureRecord {
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
    let result_type = h.types.push(ty(TypeKind::Tuple(vec![].into())));
    let OperationResult::FailureChain { failures, released } = h
        .library
        .call(
            OperationCall {
                runtime: &Default::default(),
                path: &path,
                generics: &[],
                arguments: &[tasks, mutex, body],
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
    assert_eq!(h.value("arg.count", vec![args.clone()]), u64_value(3));
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
        definition: support::path("main"),
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
        elements: vec![Some(Value::Integer(n(Primitive::U8, 7)))],
    };
    recorded.value("stdin.read", vec![input, buffer.clone(), u64_value(2)]);
    let tape = recorded.library.simulation.events.clone();
    let mut too_long = vec![0];
    too_long.extend_from_slice(&3u64.to_le_bytes());
    too_long.extend_from_slice(&[0, 255, 1]);
    let mut invalid_cause = vec![1];
    invalid_cause.extend_from_slice(&1u32.to_le_bytes());
    invalid_cause.extend_from_slice(&3u32.to_le_bytes());
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
    use hirsel_run_config::{ExitStatus, ProcessGrant, ProcessOutcome, ProcessRule, ProcessRun};
    let grant = |runs: Vec<ProcessRun>| SimulationConfig {
        process: Some(ProcessGrant {
            provider: "process.script@1".into(),
            rules: vec![ProcessRule {
                program: "/usr/bin/git".into(),
                env: vec![],
            }],
            runs: Some(runs),
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
                path(program),
                args,
                Value::Bytes(vec![]),
                none(),
                u64_value(64),
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
            hirsel_run_config::ProcessFailure::NotGranted,
        )),
        process::encode_completion(&ProcessOutcome::Output {
            status: ExitStatus::Exited(0),
            stdout: vec![0; 65],
            stderr: vec![],
        }),
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
                },
            ),
            (
                "empty".into(),
                FileMetadata {
                    kind: 1,
                    size: 0,
                    modified_unix_ns: Some(-2),
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
        } = fields.pop().unwrap().unwrap()
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
