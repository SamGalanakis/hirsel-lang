//! Boundary tests for frozen scripted-network semantics, independent of source lowering.
use hirsel_interp::{library::*, *};
use hirsel_run_config::{NetConnection, NetGrant, NetPeer, NetRule, NetStep, PeerTls};
use hirsel_span::{SourceMap, Span};
use hirsel_tir::RunConfiguration;
use hirsel_types::{BuiltinType, Primitive, Type, TypeKind, TypeStore};

struct Harness {
    library: Library,
    run: RunConfiguration,
    sources: SourceMap,
    types: TypeStore,
    stdout: Vec<u8>,
    trace: Vec<TraceEvent>,
}
impl Harness {
    fn new(peers: Vec<NetPeer>, rules: Vec<NetRule>) -> Self {
        let run = RunConfiguration {
            entry: hirsel_syntax::DefPath::core("main"),
            hash_seed: 0,
            inputs: vec![],
        };
        let config = SimulationConfig {
            net: Some(NetGrant {
                provider: "net.script@2".into(),
                rules,
                peers: Some(peers),
            }),
            ..Default::default()
        };
        Self {
            library: Library::new(&run, config).unwrap(),
            run,
            sources: SourceMap::default(),
            types: TypeStore::default(),
            stdout: vec![],
            trace: vec![],
        }
    }
    fn try_call(&mut self, name: &str, args: Vec<Value>) -> Result<OperationResult, Error> {
        let ty = self.types.push(Type::unit());
        self.library
            .call(
                OperationCall {
                    path: &hirsel_syntax::DefPath::core(name),
                    arguments: &args,
                    generics: &[],
                    types: &self.types,
                    result_type: ty,
                    site: Span::default(),
                },
                &mut OperationContext {
                    trace_mode: TraceMode::Effects,
                    anchor: None,
                    configuration: &self.run,
                    sources: &self.sources,
                    stdout: &mut self.stdout,
                    trace: &mut self.trace,
                },
            )
            .map(|v| v.unwrap())
    }
    fn call(&mut self, name: &str, args: Vec<Value>) -> OperationResult {
        self.try_call(name, args).unwrap()
    }
    fn value(&mut self, name: &str, args: Vec<Value>) -> Value {
        match self.call(name, args) {
            OperationResult::Complete { value, .. } => value,
            other => panic!("{other:?}"),
        }
    }
    fn net(&mut self) -> Value {
        let io = self.library.simulation.grant_root("Io").unwrap();
        unok(self.value("io.net", vec![io]))
    }
    fn connect(&mut self, n: &Value) -> Value {
        unok(self.value(
            "net.connect",
            vec![
                n.clone(),
                Value::String("example.com".into()),
                p(80),
                unbounded(),
            ],
        ))
    }
}
fn p(port: u16) -> Value {
    Value::Integer(Integer::new(Primitive::U16, port.into()).unwrap())
}
fn v(tag: u32, fields: Vec<Value>) -> Value {
    Value::Variant {
        variant: tag,
        fields: fields.into_iter().map(Some).collect(),
    }
}
fn unok(value: Value) -> Value {
    match value {
        Value::Variant {
            variant: 0,
            mut fields,
        } => fields.remove(0).unwrap(),
        v => panic!("expected Ok: {v:?}"),
    }
}
fn unbounded() -> Value {
    v(0, vec![])
}
fn bounded() -> Value {
    v(1, vec![u64_value(1)])
}
fn denied() -> Value {
    error(v(0, vec![]))
}
fn list(values: Vec<Value>) -> Value {
    collection::sequence(BuiltinType::List, values)
}
fn rule(operation: &str) -> NetRule {
    NetRule {
        address: "example.com".into(),
        service: "80".into(),
        operation: operation.into(),
    }
}
fn rule_value(host: &str, port: u16, op: u32) -> Value {
    Value::Record {
        definition: hirsel_syntax::DefPath::core("NetRule"),
        fields: vec![
            Some(Value::String(host.into())),
            Some(p(port)),
            Some(v(op, vec![])),
        ],
    }
}
fn peer(connections: Vec<NetConnection>) -> NetPeer {
    NetPeer {
        address: "example.com".into(),
        port: 80,
        resolves_to: vec!["127.0.0.1".into(), "::1".into()],
        tls: PeerTls::Plain,
        connections,
    }
}
fn setup(steps: Vec<NetStep>) -> (Harness, Value, Value) {
    let mut h = Harness::new(
        vec![peer(vec![NetConnection::Open(steps)])],
        vec![rule("connect")],
    );
    let n = h.net();
    let s = h.connect(&n);
    (h, n, s)
}
#[test]
fn denied_calls_and_invalid_input_leave_connection_queue_untouched() {
    let mut h = Harness::new(
        vec![peer(vec![NetConnection::Open(vec![])])],
        vec![rule("connect")],
    );
    let n = h.net();
    assert_eq!(
        h.value(
            "net.connect",
            vec![
                n.clone(),
                Value::String("other.com".into()),
                p(80),
                unbounded()
            ]
        ),
        denied()
    );
    assert_eq!(
        h.value(
            "net.connect",
            vec![n.clone(), Value::String("127.1".into()), p(80), unbounded()]
        ),
        error(v(2, vec![]))
    );
    assert_eq!(
        h.value(
            "net.connect",
            vec![
                n.clone(),
                Value::String("example.com".into()),
                p(80),
                v(1, vec![u64_value(0)])
            ]
        ),
        error(v(2, vec![]))
    );
    h.connect(&n);
    assert_eq!(
        h.value(
            "net.connect",
            vec![n, Value::String("example.com".into()), p(80), unbounded()]
        ),
        error(v(5, vec![]))
    );
}
#[test]
fn script_cannot_widen_explicit_empty_rules() {
    let mut h = Harness::new(vec![peer(vec![NetConnection::Open(vec![])])], vec![]);
    let n = h.net();
    assert_eq!(
        h.value(
            "net.resolve",
            vec![n, Value::String("example.com".into()), p(80)]
        ),
        denied()
    );
}
#[test]
fn narrowing_is_a_subset_and_cannot_restore_plaintext() {
    let mut h = Harness::new(vec![], vec![rule("connect")]);
    let n = h.net();
    let tls = unok(h.value(
        "net.narrow",
        vec![n.clone(), list(vec![rule_value("example.com", 80, 1)])],
    ));
    assert_eq!(
        h.value(
            "net.narrow",
            vec![tls, list(vec![rule_value("example.com", 80, 0)])]
        ),
        denied()
    );
    assert_eq!(
        h.value(
            "net.narrow",
            vec![n.clone(), list(vec![rule_value("other.com", 80, 0)])]
        ),
        denied()
    );
    let empty = unok(h.value("net.narrow", vec![n, list(vec![])]));
    assert_eq!(
        h.value(
            "net.resolve",
            vec![empty, Value::String("example.com".into()), p(80)]
        ),
        denied()
    );
}
#[test]
fn refusal_timeout_exhaustion_and_unbounded_connect_quiescence() {
    let mut h = Harness::new(
        vec![peer(vec![
            NetConnection::Refused,
            NetConnection::Unanswered,
            NetConnection::Unanswered,
        ])],
        vec![rule("connect")],
    );
    let n = h.net();
    let a = |timeout| {
        vec![
            n.clone(),
            Value::String("example.com".into()),
            p(80),
            timeout,
        ]
    };
    assert_eq!(h.value("net.connect", a(unbounded())), error(v(5, vec![])));
    assert_eq!(h.value("net.connect", a(bounded())), error(v(15, vec![])));
    assert!(
        matches!(h.try_call("net.connect",a(unbounded())), Err(Error::Blocked(waits)) if waits.len() == 1)
    );
    assert_eq!(h.value("net.connect", a(unbounded())), error(v(5, vec![])));
}
#[test]
fn reads_append_and_inline_reads_fill_only_a_prefix() {
    let (mut h, n, s) = setup(vec![
        NetStep::Send(b"abc".to_vec()),
        NetStep::Send(b"d".to_vec()),
    ]);
    let result = h.call(
        "net.read",
        vec![
            n.clone(),
            s.clone(),
            list(vec![Value::Integer(
                Integer::new(Primitive::U8, 9).unwrap(),
            )]),
            u64_value(2),
        ],
    );
    let OperationResult::Complete {
        value, arguments, ..
    } = result
    else {
        panic!()
    };
    assert_eq!(value, ok(u64_value(2)));
    assert_eq!(
        collection::elements(&arguments[0].1, BuiltinType::List)
            .unwrap()
            .len(),
        3
    );
    let buffer = collection::sequence(
        BuiltinType::Inline,
        vec![Value::Integer(Integer::new(Primitive::U8, 9).unwrap()); 4],
    );
    let OperationResult::Complete {
        value, arguments, ..
    } = h.call("net.read_into", vec![n.clone(), s.clone(), buffer])
    else {
        panic!()
    };
    assert_eq!(value, ok(u64_value(1)));
    assert_eq!(
        collection::elements(&arguments[0].1, BuiltinType::Inline)
            .unwrap()
            .iter()
            .map(|v| match v {
                Value::Integer(i) => i.bits(),
                _ => panic!(),
            })
            .collect::<Vec<_>>(),
        vec![99, 9, 9, 9]
    );
    assert_eq!(
        h.value(
            "net.read",
            vec![n.clone(), s.clone(), list(vec![]), u64_value(10)]
        ),
        ok(u64_value(1))
    );
    assert_eq!(
        h.value("net.read", vec![n, s, list(vec![]), u64_value(10)]),
        ok(u64_value(0))
    );
}
#[test]
fn read_timeout_and_no_timeout_blocked_are_distinct() {
    let (mut h, n, s) = setup(vec![NetStep::Stall]);
    assert!(
        matches!(h.try_call("net.read",vec![n.clone(),s.clone(),list(vec![]),u64_value(1)]), Err(Error::Blocked(waits)) if waits.len() == 1)
    );
    assert_eq!(
        h.value(
            "net.set_read_timeout",
            vec![n.clone(), s.clone(), bounded()]
        ),
        ok(Value::Unit)
    );
    assert_eq!(
        h.value("net.read", vec![n, s, list(vec![]), u64_value(1)]),
        error(v(15, vec![]))
    );
}

#[test]
fn repeated_tls_upgrade_is_invalid_and_consumes_the_interpreter_stream() {
    let mut peer = peer(vec![NetConnection::Open(vec![])]);
    peer.tls = PeerTls::Certificate(vec!["example.com".into()]);
    let mut h = Harness::new(vec![peer], vec![rule("connect_tls")]);
    let n = h.net();
    let s = h.connect(&n);
    let Value::Handle { id, .. } = s else {
        panic!()
    };
    let mut args = vec![n, s, Value::String("example.com".into()), v(0, vec![])];
    assert_eq!(h.value("tls.connect", args.clone()), ok(args[1].clone()));
    args[2] = Value::String("other.com".into());
    assert_eq!(h.value("tls.connect", args), error(v(2, vec![])));
    assert!(
        !h.library
            .simulation
            .observed_state()
            .handles
            .contains_key(&id)
    );
}

#[test]
fn tls_only_grant_can_bound_a_stalled_scripted_handshake() {
    let mut peer = peer(vec![NetConnection::Open(vec![NetStep::Stall])]);
    peer.tls = PeerTls::Certificate(vec!["example.com".into()]);
    let mut h = Harness::new(vec![peer], vec![rule("connect_tls")]);
    let n = h.net();
    let s = h.connect(&n);
    let Value::Handle { id, .. } = &s else {
        panic!()
    };
    let id = *id;
    for name in ["net.set_read_timeout", "net.set_write_timeout"] {
        assert_eq!(
            h.value(name, vec![n.clone(), s.clone(), bounded()]),
            ok(Value::Unit)
        );
    }
    assert!(matches!(
        h.value("net.peer_addr", vec![n.clone(), s.clone()]),
        Value::Variant { variant: 0, .. }
    ));
    assert_eq!(
        h.value(
            "tls.connect",
            vec![n, s, Value::String("example.com".into()), v(0, vec![])]
        ),
        error(v(15, vec![]))
    );
    assert!(
        !h.library
            .simulation
            .observed_state()
            .handles
            .contains_key(&id)
    );
}
#[test]
fn explicit_and_implicit_close_record_once_in_order() {
    let (mut h, n, s) = setup(vec![]);
    let id = if let Value::Handle { id, .. } = s {
        id
    } else {
        panic!()
    };
    assert_eq!(h.value("net.close", vec![n, s]), Value::Unit);
    let mut cx = OperationContext {
        trace_mode: TraceMode::Effects,
        anchor: None,
        configuration: &h.run,
        sources: &h.sources,
        stdout: &mut h.stdout,
        trace: &mut h.trace,
    };
    h.library.drop_handle("Stream", id, &mut cx).unwrap();
    assert_eq!(
        h.library
            .simulation
            .events
            .iter()
            .filter(|e| e.operation == "net.close")
            .count(),
        1
    );
}
#[test]
fn replay_rejects_changed_written_bytes_and_rolls_back() {
    let (mut h, n, s) = setup(vec![NetStep::Expect(b"ok".to_vec())]);
    h.value("net.write_all", vec![n, s, Value::Bytes(b"ok".to_vec())]);
    let events = h.library.simulation.events.clone();
    let mut run = h.run.clone();
    run.inputs = events;
    let mut library = Library::new(&run, h.library.simulation.config.clone()).unwrap();
    let io = library.simulation.grant_root("Io").unwrap();
    // Replay checks all prior acquisitions as well as every byte written.
    let ty = h.types.push(Type::unit());
    let mut cx = OperationContext {
        trace_mode: TraceMode::Effects,
        anchor: None,
        configuration: &run,
        sources: &h.sources,
        stdout: &mut h.stdout,
        trace: &mut h.trace,
    };
    let mut call = |name: &str, args: &[Value]| {
        library.call(
            OperationCall {
                path: &hirsel_syntax::DefPath::core(name),
                arguments: args,
                generics: &[],
                types: &h.types,
                result_type: ty,
                site: Span::default(),
            },
            &mut cx,
        )
    };
    let Some(OperationResult::Complete { value, .. }) = call("io.net", &[io]).unwrap() else {
        panic!()
    };
    let n = unok(value);
    let Some(OperationResult::Complete { value, .. }) = call(
        "net.connect",
        &[
            n.clone(),
            Value::String("example.com".into()),
            p(80),
            unbounded(),
        ],
    )
    .unwrap() else {
        panic!()
    };
    let s = unok(value);
    assert!(
        call(
            "net.write_all",
            &[n.clone(), s.clone(), Value::Bytes(b"no".to_vec())]
        )
        .is_err()
    );
    assert!(call("net.write_all", &[n, s, Value::Bytes(b"ok".to_vec())]).is_ok());
    library.simulation.finish_replay().unwrap();
}

#[test]
fn cancellation_is_before_connect_or_bytes_and_denial_does_not_consume_it() {
    use hirsel_interp::library::simulation::Fault;
    let mut h = Harness::new(
        vec![peer(vec![NetConnection::Open(vec![NetStep::Expect(
            b"ok".to_vec(),
        )])])],
        vec![rule("connect")],
    );
    let n = h.net();
    h.library
        .simulation
        .config
        .faults
        .insert("net.connect".into(), [Fault::Canceled(0)].into());
    assert_eq!(
        h.value(
            "net.connect",
            vec![
                n.clone(),
                Value::String("denied.com".into()),
                p(80),
                unbounded()
            ]
        ),
        denied()
    );
    assert_eq!(
        h.value(
            "net.connect",
            vec![
                n.clone(),
                Value::String("example.com".into()),
                p(80),
                unbounded()
            ]
        ),
        error(v(1, vec![v(0, vec![])]))
    );
    let s = h.connect(&n);
    h.library
        .simulation
        .config
        .faults
        .insert("net.write_all".into(), [Fault::Canceled(0)].into());
    assert_eq!(
        h.value(
            "net.write_all",
            vec![n.clone(), s.clone(), Value::Bytes(b"wrong".to_vec())]
        ),
        error(v(1, vec![v(0, vec![])]))
    );
    assert_eq!(
        h.value("net.write_all", vec![n, s, Value::Bytes(b"ok".to_vec())]),
        ok(Value::Unit)
    );
}
#[test]
fn a_partial_write_fault_keeps_the_accepted_prefix() {
    use hirsel_interp::library::simulation::Fault;
    let (mut h, n, s) = setup(vec![
        NetStep::Expect(b"abcdef".to_vec()),
        NetStep::Send(b"ok".to_vec()),
    ]);
    h.library.simulation.config.faults.insert(
        "net.write_all".into(),
        [Fault::Fault {
            code: 42,
            partial: 3,
        }]
        .into(),
    );
    assert_eq!(
        h.value(
            "net.write_all",
            vec![n.clone(), s.clone(), Value::Bytes(b"abcdef".to_vec())]
        ),
        error(v(17, vec![u32_value(42)]))
    );
    assert_eq!(
        h.value(
            "net.write_all",
            vec![n.clone(), s.clone(), Value::Bytes(b"def".to_vec())]
        ),
        ok(Value::Unit)
    );
    assert_eq!(
        h.value("net.read", vec![n, s, list(vec![]), u64_value(10)]),
        ok(u64_value(2))
    );
}
#[test]
fn peer_address_uses_literal_or_first_resolution_and_never_invents_an_ip() {
    for addresses in [vec![], vec!["::1".into(), "127.0.0.1".into()]] {
        let mut peer = peer(vec![NetConnection::Open(vec![])]);
        peer.resolves_to = addresses.clone();
        let mut h = Harness::new(vec![peer], vec![rule("connect")]);
        let n = h.net();
        let s = h.connect(&n);
        let result = h.value("net.peer_addr", vec![n, s]);
        if addresses.is_empty() {
            assert_eq!(result, error(v(3, vec![])));
        } else {
            let address = unok(result);
            let Value::Record { fields, .. } = address else {
                panic!()
            };
            assert!(matches!(
                &fields[0],
                Some(Value::Variant { variant: 1, .. })
            ));
        }
    }
}
#[test]
fn duplicate_simulation_peers_refuse_and_distinct_ports_are_allowed() {
    let mut h = Harness::new(vec![peer(vec![])], vec![]);
    let mut config = h.library.simulation.config.clone();
    config
        .net
        .as_mut()
        .unwrap()
        .peers
        .as_mut()
        .unwrap()
        .push(peer(vec![]));
    assert!(
        matches!(Library::new(&h.run,config.clone()),Err(Error::InvalidTir(detail)) if detail.contains("entries 0 and 1"))
    );
    config.net.as_mut().unwrap().peers.as_mut().unwrap()[1].port = 81;
    Library::new(&h.run, config).unwrap();
    let _ = h.net();
}

#[test]
fn closed_writes_return_broken_pipe_without_consuming_partial_faults() {
    use hirsel_interp::library::simulation::Fault;
    for operation in ["net.write", "net.write_all"] {
        let (mut h, n, s) = setup(vec![]);
        assert_eq!(
            h.value("net.shutdown", vec![n.clone(), s.clone(), v(1, vec![])]),
            ok(Value::Unit)
        );
        h.library.simulation.config.faults.insert(
            operation.into(),
            [Fault::Fault {
                code: 42,
                partial: 1,
            }]
            .into(),
        );
        assert_eq!(
            h.value(operation, vec![n, s, Value::Bytes(b"x".to_vec())]),
            error(v(14, vec![]))
        );
        assert_eq!(h.library.simulation.config.faults[operation].len(), 1);
    }
}
#[test]
fn return_cleanup_records_real_stream_closes_in_the_requested_reverse_order() {
    use crate::support;
    use hirsel_tir::Terminator;
    let mut h = Harness::new(
        vec![peer(vec![
            NetConnection::Open(vec![]),
            NetConnection::Open(vec![]),
        ])],
        vec![rule("connect")],
    );
    let net = h.net();
    let first = h.connect(&net);
    let second = h.connect(&net);
    let ids = [&second, &first].map(|s| match s {
        Value::Handle { id, .. } => *id,
        _ => panic!(),
    });
    let mut b = support::Builder::new();
    let resource = || TypeKind::Named {
        definition: hirsel_syntax::DefPath::core("Stream"),
        arguments: vec![].into(),
    };
    let one = b.parameter(resource());
    let two = b.parameter(resource());
    let unit = b.unit();
    let cleanup = b.cleanup(vec![support::drop(two), support::drop(one)]);
    b.block(
        Terminator::Return {
            value: support::take(unit),
            cleanup,
        },
        7,
    );
    let sources = support::sources();
    let run = RunConfiguration {
        entry: support::path("main"),
        hash_seed: 0,
        inputs: vec![],
    };
    let execution = run_with_runtime(
        &mut support::Provider {
            definitions: vec![b.definition("main")],
            queries: vec![],
        },
        &sources,
        &run,
        vec![first, second],
        Limits::default(),
        &mut h.library,
        &aggregate::RuntimeTypes::default(),
    )
    .unwrap();
    assert!(matches!(execution.outcome, Outcome::Returned { .. }));
    let closed: Vec<_> = h
        .library
        .simulation
        .events
        .iter()
        .filter(|e| e.operation == "net.close")
        .map(|e| u64::from_le_bytes(e.input.clone().try_into().unwrap()))
        .collect();
    assert_eq!(closed, ids);
}
