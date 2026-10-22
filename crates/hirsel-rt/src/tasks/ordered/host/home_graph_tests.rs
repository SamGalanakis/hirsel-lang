use super::*;
use crate::io::{Adapter, AdapterError, Completion, Ready, Request};
use crate::io_keys::{Key, KeySet, Kind, Mode, Registry};
use crate::ordered::Policy;
use std::{cell::RefCell, rc::Rc};

struct Graph {
    registry: Registry,
    base: u64,
    fault: u8,
    attempt: bool,
    opens: u8,
    run: *mut crate::context::RunShared,
}
struct Home(Rc<RefCell<Graph>>);
impl Adapter for Home {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn keys(&self, request: &Request) -> Option<KeySet> {
        let mut graph = self.0.borrow_mut();
        let _role = unsafe { role::IdleGuard::new(graph.run) };
        if graph.fault == 4 && graph.attempt && matches!(request, Request::Connect { .. }) {
            return Some(KeySet::new(vec![(Key(7777), Mode::Exclusive)]));
        }
        Some(match request {
            Request::Net { .. } => KeySet::new(vec![(crate::io_keys::ROOT, Mode::Shared)]),
            _ => graph.registry.keys(request),
        })
    }
    fn domains(&self, request: &Request) -> Vec<(Key, u64, String)> {
        let mut graph = self.0.borrow_mut();
        let _role = unsafe { role::IdleGuard::new(graph.run) };
        if let Request::Connect { net, .. } = request {
            let ports = graph.registry.net_ports(*net).unwrap();
            vec![(ports, *net, "ports".into())]
        } else {
            Vec::new()
        }
    }
    fn identity(&self, handle: u64) -> Option<(Key, Kind)> {
        let graph = self.0.borrow();
        let _role = unsafe { role::IdleGuard::new(graph.run) };
        if graph.fault == 1 && handle == graph.base + 3 {
            return None;
        }
        if graph.fault == 2 && graph.opens > 1 && handle == graph.base + 3 {
            return Some((Key(9000), Kind::File));
        }
        graph.registry.identity(handle)
    }
    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        let mut graph = self.0.borrow_mut();
        let base = graph.base;
        Ok(match request {
            Request::Directory { .. } => {
                graph.registry.resource(base + 1, Kind::File);
                Completion::Handle(Ok(base + 1))
            }
            Request::Create { .. } => {
                graph.registry.resource(base + 2, Kind::File);
                Completion::Handle(Ok(base + 2))
            }
            Request::Open { .. } => {
                graph.opens += 1;
                graph.registry.alias(base + 2, base + 3);
                Completion::Handle(Ok(base + 3))
            }
            Request::FileRead { file, .. } => {
                assert!(graph.registry.identity(file).is_some());
                Completion::Bytes(Ok(vec![0, 255]))
            }
            Request::Net { .. } => {
                graph.registry.resource(base + 4, Kind::Other);
                Completion::Handle(Ok(base + 4))
            }
            Request::Connect { .. } => {
                if !graph.attempt {
                    graph.attempt = true;
                    return Err(AdapterError::Blocked);
                }
                graph.registry.resource(base + 5, Kind::Other);
                Completion::NetHandle(Ok(base + 5))
            }
            Request::NetSplit { stream, .. } => {
                graph.registry.split(stream, base + 6, base + 7);
                if graph.fault == 3 {
                    graph
                        .registry
                        .bind(base + 7, Key(9999), Kind::Half { stream: Key(8888) });
                }
                Completion::Halves(Ok((base + 6, base + 7)))
            }
            Request::NetRead { .. } => Completion::NetBytes(Ok(vec![1, 2])),
            Request::NetWrite { bytes, .. } => Completion::Count(Ok(bytes.len() as u64)),
            Request::Close { .. } => Completion::Unit(Ok(())),
            _ => panic!("unexpected {request:?}"),
        })
    }
    fn ready(&mut self) -> Result<Ready, AdapterError> {
        Ok(Ready::Every)
    }
    fn wait(&mut self) -> Result<bool, AdapterError> {
        Ok(true)
    }
}

fn handle(value: Result<Completion, ()>) -> u64 {
    let Ok(Completion::Handle(Ok(handle))) = value else {
        panic!("handle")
    };
    handle
}

#[test]
fn home_declared_graph_keeps_file_aliases_and_parked_split_directions_independent_of_native_keys() {
    use super::super::events::{
        CanonicalOutcome,
        host_keys::{Resource, ResourceKind},
    };
    let mut reference = None;
    for width in [1, 2, 4, 8] {
        for reorder in [
            hirsel_run_config::Reorder::Off,
            hirsel_run_config::Reorder::Auto,
        ] {
            let base = u64::from(width) * 1000;
            let mut registry = Registry::default();
            // Physical resource creation outside this run's visible graph must
            // not enter canonical names or depend on native key allocation.
            for index in 0..width {
                registry.resource(base + 100 + u64::from(index), Kind::Other);
            }
            let graph = Rc::new(RefCell::new(Graph {
                registry,
                base,
                fault: 0,
                attempt: false,
                opens: 0,
                run: std::ptr::null_mut(),
            }));
            let setup = graph.clone();
            let mut run = crate::host::BodyRun::new(width, true);
            run.adapter = Some(Box::new(crate::launch::Shared(Rc::new(RefCell::new(
                Home(graph),
            )))));
            unsafe { run.use_physical_dispatch() };
            let (outcome, trace) = run.run_traced(
                move |ctx| unsafe {
                    let mut policy = Policy::admit(&Default::default(), None);
                    policy.reorder = reorder;
                    engine::enable(ctx, policy);
                    setup.borrow_mut().run = (*ctx).run_ptr();
                    super::super::events::enable(ctx);
                    super::super::events::initial_host(ctx, Some(1), None);
                },
                move |ctx| unsafe {
                    let _role = role::Guard::new(ctx);
                    let directory = handle(crate::io::capability_call(
                        ctx,
                        Request::Directory {
                            io: 1,
                            path: String::new(),
                        },
                    ));
                    let file = handle(crate::io::capability_call(
                        ctx,
                        Request::Create {
                            directory,
                            path: "a".into(),
                        },
                    ));
                    let alias = handle(crate::io::capability_call(
                        ctx,
                        Request::Open {
                            directory,
                            path: "a".into(),
                        },
                    ));
                    for file in [file, alias] {
                        assert_eq!(
                            crate::io::capability_call(
                                ctx,
                                Request::FileRead {
                                    directory,
                                    file,
                                    max: 2
                                }
                            ),
                            Ok(Completion::Bytes(Ok(vec![0, 255])))
                        );
                    }
                    let net = handle(crate::io::capability_call(ctx, Request::Net { io: 1 }));
                    let Ok(Completion::NetHandle(Ok(stream))) = crate::io::capability_call(
                        ctx,
                        Request::Connect {
                            net,
                            host: "peer".into(),
                            port: 1,
                            timeout: crate::net::NetTimeout::Unbounded,
                        },
                    ) else {
                        panic!("stream")
                    };
                    let Ok(Completion::Halves(Ok((read, write)))) =
                        crate::io::capability_call(ctx, Request::NetSplit { net, stream })
                    else {
                        panic!("split")
                    };
                    assert_eq!(
                        crate::io::capability_call(
                            ctx,
                            Request::NetRead {
                                net,
                                stream: read,
                                max: 2
                            }
                        ),
                        Ok(Completion::NetBytes(Ok(vec![1, 2])))
                    );
                    assert_eq!(
                        crate::io::capability_call(
                            ctx,
                            Request::NetWrite {
                                net,
                                stream: write,
                                bytes: vec![9]
                            }
                        ),
                        Ok(Completion::Count(Ok(1)))
                    );
                    let log = engine::driver(ctx).events.as_ref().unwrap();
                    assert_eq!(
                        log.host_bindings[&log.host_handles[&file]].resource,
                        log.host_bindings[&log.host_handles[&alias]].resource
                    );
                    let parent = log.host_bindings[&log.host_handles[&stream]]
                        .resource
                        .clone();
                    for half in [read, write] {
                        assert_eq!(
                            log.host_bindings[&log.host_handles[&half]].kind,
                            ResourceKind::Half {
                                stream: parent.clone()
                            }
                        );
                    }
                    assert_ne!(
                        log.host_bindings[&log.host_handles[&read]].resource,
                        log.host_bindings[&log.host_handles[&write]].resource
                    );
                    assert_eq!(log.host_polls.len(), 1);
                    assert!(log.canonical().unwrap().iter().any(|event| matches!(
                        event.outcome,
                        CanonicalOutcome::HostResourceCreated
                    )));
                    assert!(
                        log.host_bindings
                            .values()
                            .all(|row| row.resource != Resource::Coupled)
                    );
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK));
            assert_eq!(outcome.pending, 0);
            let bytes = trace.unwrap().bytes().to_vec();
            assert!(
                std::str::from_utf8(&bytes)
                    .unwrap()
                    .contains("\"kind\":\"domain\"")
            );
            if let Some(reference) = &reference {
                assert_eq!(&bytes, reference);
            } else {
                reference = Some(bytes);
            }
        }
    }
}

#[test]
fn home_declared_graph_missing_discovery_or_conflicting_parent_refuses_export_without_changing_results()
 {
    for fault in [1, 2, 3, 4] {
        let graph = Rc::new(RefCell::new(Graph {
            registry: Registry::default(),
            base: 100,
            fault,
            attempt: fault != 4,
            opens: 0,
            run: std::ptr::null_mut(),
        }));
        let setup = graph.clone();
        let mut run = crate::host::BodyRun::new(2, true);
        run.adapter = Some(Box::new(crate::launch::Shared(Rc::new(RefCell::new(
            Home(graph),
        )))));
        unsafe { run.use_physical_dispatch() };
        let (outcome, trace) = run.run_traced(
            move |ctx| unsafe {
                engine::enable(ctx, Policy::admit(&Default::default(), None));
                setup.borrow_mut().run = (*ctx).run_ptr();
                super::super::events::enable(ctx);
                super::super::events::initial_host(ctx, Some(1), None);
            },
            move |ctx| unsafe {
                let _role = role::Guard::new(ctx);
                let directory = handle(crate::io::capability_call(
                    ctx,
                    Request::Directory {
                        io: 1,
                        path: String::new(),
                    },
                ));
                handle(crate::io::capability_call(
                    ctx,
                    Request::Create {
                        directory,
                        path: "a".into(),
                    },
                ));
                if fault <= 2 {
                    assert_eq!(
                        crate::io::capability_call(
                            ctx,
                            Request::Open {
                                directory,
                                path: "a".into()
                            }
                        ),
                        Ok(Completion::Handle(Ok(103)))
                    );
                    if fault == 2 {
                        assert_eq!(
                            crate::io::capability_call(
                                ctx,
                                Request::Open {
                                    directory,
                                    path: "a".into()
                                }
                            ),
                            Ok(Completion::Handle(Ok(103)))
                        );
                    }
                } else {
                    let net = handle(crate::io::capability_call(ctx, Request::Net { io: 1 }));
                    let Ok(Completion::NetHandle(Ok(stream))) = crate::io::capability_call(
                        ctx,
                        Request::Connect {
                            net,
                            host: "peer".into(),
                            port: 1,
                            timeout: crate::net::NetTimeout::Unbounded,
                        },
                    ) else {
                        panic!("stream")
                    };
                    assert_eq!(
                        crate::io::capability_call(ctx, Request::NetSplit { net, stream }),
                        Ok(Completion::Halves(Ok((106, 107))))
                    );
                }
                assert!(
                    engine::driver(ctx)
                        .events
                        .as_ref()
                        .unwrap()
                        .canonical()
                        .is_err()
                );
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert!(trace.is_err());
    }
}
