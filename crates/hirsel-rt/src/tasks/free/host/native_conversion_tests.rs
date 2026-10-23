use super::*;
use crate::io_concurrent::ConcurrentAdapter;
use crate::io_keys::{Key, Kind, NAMESPACE, ROOT};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, Debug)]
enum Case {
    Resolve,
    Read,
    Udp,
    Peer,
    Protocol,
    Env,
    Entries,
    Walk,
    RunnerEntry,
}
struct Payloads;
impl ConcurrentAdapter for Payloads {
    fn keys(&self, request: &Request) -> KeySet {
        KeySet::new(match request {
            Request::Net { .. } => vec![(ROOT, Mode::Shared)],
            Request::Directory { .. } | Request::ReadDirPage { .. } => {
                vec![(NAMESPACE, Mode::Shared)]
            }
            Request::Resolve { .. } => vec![(Key(7), Mode::Shared)],
            Request::EnvGet { .. } => vec![],
            _ => vec![(Key(7), Mode::Exclusive)],
        })
    }
    fn identity(&self, word: u64) -> Option<(Key, Kind)> {
        match word {
            7 => Some((Key(7), Kind::Other)),
            8 => Some((Key(8), Kind::File)),
            _ => None,
        }
    }
    fn net_ports(&self, word: u64) -> Option<Key> {
        (word == 7).then_some(Key(9))
    }
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn call(&self, request: Request) -> Result<Completion, AdapterError> {
        Ok(match request {
            Request::Net { .. } => Completion::Handle(Ok(7)),
            Request::Directory { .. } => Completion::Handle(Ok(8)),
            Request::Resolve { .. } => Completion::Addresses(Ok(vec![
                crate::net::SocketAddr::of(
                    ([127, 0, 0, 1], 80).into()
                );
                8
            ])),
            Request::NetRead { .. } => Completion::NetBytes(Ok(vec![42; 8192])),
            Request::UdpRecv { .. } => Completion::Datagram(Ok((
                vec![42; 8192],
                crate::net::SocketAddr::of(([127, 0, 0, 1], 80).into()),
                false,
            ))),
            Request::TlsPeer { .. } => Completion::Certificates(Ok(Some(vec![42; 8192]))),
            Request::TlsProtocol { .. } => Completion::Protocol(Ok(Some("p".repeat(8192)))),
            Request::EnvGet { .. } => Completion::Env(Ok(Some("e".repeat(8192)))),
            Request::ReadDirPage { .. } => Completion::Entries(Ok(vec![crate::io::DirEntry {
                path: "data".into(),
                kind: crate::io::FileKind::Regular,
            }])),
            _ => panic!("unexpected conversion request"),
        })
    }
}
struct NamesOnly;
impl crate::io::Adapter for NamesOnly {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        panic!("home preflight")
    }
    fn call(&mut self, _: Request) -> Result<Completion, AdapterError> {
        panic!("home effect")
    }
}
struct AtCut(AtomicBool);
impl crate::stop::Terminal for AtCut {
    fn stop(&self, cause: &Stop) -> std::io::Result<()> {
        if matches!(cause, Stop::BudgetExceeded(_)) {
            // This fixture has one task and only synchronous allocation-budget stops.
            // The terminal callback runs before its context/result owner is reclaimed.
            let ctx = crate::stop::running();
            self.0.store(
                !ctx.is_null()
                    && unsafe {
                        (*ctx).native_host_result.is_some() && (*ctx).pending_host.is_none()
                    },
                Ordering::Release,
            );
        }
        Ok(())
    }
}

#[test]
fn bulk_native_abi_conversions_keep_completed_payloads_owned_at_allocation_cuts() {
    for width in [1, 8] {
        for case in [
            Case::Resolve,
            Case::Read,
            Case::Udp,
            Case::Peer,
            Case::Protocol,
            Case::Env,
            Case::Entries,
            Case::Walk,
            Case::RunnerEntry,
        ] {
            let adapter = Arc::new(Payloads);
            let mut admitted = Some(adapter.clone());
            let at_cut = Arc::new(AtCut(AtomicBool::new(false)));
            let mut observer = Some(at_cut.clone());
            let recording =
                super::super::record::Recording::new(b"bulk allocation ownership", false);
            let install = recording.clone();
            let mut run = crate::host::BodyRun::new(width, false);
            run.free = true;
            run.adapter = Some(Box::new(NamesOnly));
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe {
                    super::super::enable(ctx);
                    (*ctx).run().free_root_env = true;
                    crate::runners::configure(
                        ctx,
                        crate::runners::Config {
                            entries: vec![(42, "entry".repeat(2048))],
                            ..Default::default()
                        },
                    );
                    install.root(ctx);
                },
                move |ctx| unsafe {
                    {
                        let _role = role::Guard::new(ctx);
                        let _segment = role::Segment::new(ctx);
                        concurrent::Service::install(ctx, admitted.take().unwrap()).unwrap();
                        native::enable(ctx);
                    }
                    (*ctx).run_ref().control.terminal(observer.take().unwrap());
                    let mut net = crate::io::GrantResult::default();
                    crate::io::hirsel_rt_v2_io_net(ctx, std::ptr::null_mut(), &mut net, &1);
                    let root = crate::paths::names(ctx).paths.intern("");
                    let mut directory = crate::io::HandleResult {
                        tag: 0,
                        padding: 0,
                        value: 0,
                    };
                    crate::io::hirsel_rt_v2_io_dir(
                        ctx,
                        std::ptr::null_mut(),
                        &mut directory,
                        &1,
                        &root,
                    );
                    let name = crate::text::allocate_bytes(ctx, b"name");
                    let mut buffer = crate::text::allocate_bytes(ctx, b"").owner;
                    {
                        let _role = role::Guard::new(ctx);
                        (*ctx).set_budgets(None, Some(1));
                    }
                    let mut word = crate::net_abi::NetWordResult::default();
                    let mut stream = 7;
                    match case {
                        Case::Resolve => {
                            crate::net_abi::hirsel_rt_v2_net_resolve(
                                ctx,
                                std::ptr::null_mut(),
                                &mut word,
                                &net.value,
                                name.owner,
                                name.start,
                                name.length,
                                80,
                            );
                        }
                        Case::Read => {
                            crate::net_abi::hirsel_rt_v2_net_read(
                                ctx,
                                std::ptr::null_mut(),
                                &mut word,
                                &net.value,
                                &mut stream,
                                &mut buffer,
                                8192,
                            );
                        }
                        Case::Udp => {
                            let mut out = crate::net_abi::UdpReceivedResult::default();
                            crate::net_abi::hirsel_rt_v2_udp_recv_from(
                                ctx,
                                std::ptr::null_mut(),
                                &mut out,
                                &net.value,
                                &mut stream,
                                &mut buffer,
                                8192,
                            );
                        }
                        Case::Peer | Case::Protocol => {
                            let mut out = std::mem::MaybeUninit::uninit();
                            if matches!(case, Case::Peer) {
                                crate::tls_abi::hirsel_rt_v2_tls_peer(
                                    ctx,
                                    std::ptr::null_mut(),
                                    out.as_mut_ptr(),
                                    &net.value,
                                    &stream,
                                );
                            } else {
                                crate::tls_abi::hirsel_rt_v2_tls_negotiated_protocol(
                                    ctx,
                                    std::ptr::null_mut(),
                                    out.as_mut_ptr(),
                                    &net.value,
                                    &stream,
                                );
                            }
                        }
                        Case::Env => {
                            let mut out = std::mem::MaybeUninit::uninit();
                            crate::io::hirsel_rt_v2_env_get(
                                ctx,
                                std::ptr::null_mut(),
                                out.as_mut_ptr(),
                                &crate::io::ENV_HANDLE,
                                name.owner,
                                name.start,
                                name.length,
                            );
                        }
                        Case::Entries => {
                            let mut out = std::mem::MaybeUninit::<crate::io::WalkResult>::uninit();
                            crate::io::hirsel_rt_v2_file_read_dir(
                                ctx,
                                std::ptr::null_mut(),
                                out.as_mut_ptr(),
                                &directory.value,
                                &root,
                            );
                        }
                        Case::Walk => {
                            let mut out = std::mem::MaybeUninit::uninit();
                            crate::io::hirsel_rt_v2_file_walk(
                                ctx,
                                std::ptr::null_mut(),
                                out.as_mut_ptr(),
                                &directory.value,
                            );
                        }
                        Case::RunnerEntry => {
                            let mut out = std::mem::MaybeUninit::uninit();
                            crate::runners::hirsel_rt_v2_runner_entry_path(
                                ctx,
                                std::ptr::null_mut(),
                                out.as_mut_ptr(),
                                &42,
                            );
                        }
                    }
                    unreachable!("{case:?} allocation did not stop");
                },
            );
            assert!(
                matches!(outcome.ended, Err(Stop::BudgetExceeded(_))),
                "{width} {case:?} {outcome:?}"
            );
            assert_eq!(outcome.pending, 0);
            assert!(
                at_cut.0.load(Ordering::Acquire),
                "{width} {case:?}: payload lost its task owner before allocation"
            );
            assert_eq!(Arc::strong_count(&adapter), 1);
            assert_eq!(Arc::strong_count(&recording), 1);
        }
    }
}

#[test]
fn native_walk_conversion_sorts_paths_and_preserves_the_iterator_layout() {
    for width in [1, 8] {
        let mut admitted = Some(Arc::new(Payloads));
        let mut run = crate::host::BodyRun::new(width, false);
        run.free = true;
        run.adapter = Some(Box::new(NamesOnly));
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe { super::super::enable(ctx) },
            move |ctx| unsafe {
                {
                    let _role = role::Guard::new(ctx);
                    let _segment = role::Segment::new(ctx);
                    concurrent::Service::install(ctx, admitted.take().unwrap()).unwrap();
                    native::enable(ctx);
                }
                let root = crate::paths::names(ctx).paths.intern("");
                let mut directory = crate::io::HandleResult {
                    tag: 0,
                    padding: 0,
                    value: 0,
                };
                assert_eq!(
                    crate::io::hirsel_rt_v2_io_dir(
                        ctx,
                        std::ptr::null_mut(),
                        &mut directory,
                        &1,
                        &root
                    ),
                    STATUS_OK
                );
                let mut out = std::mem::MaybeUninit::uninit();
                assert_eq!(
                    crate::io::hirsel_rt_v2_file_walk(
                        ctx,
                        std::ptr::null_mut(),
                        out.as_mut_ptr(),
                        &directory.value
                    ),
                    STATUS_OK
                );
                let out = out.assume_init();
                assert_eq!(out.tag, 0);
                let environment = out.value.environment;
                for expected in [Some("data"), None] {
                    let mut step = crate::io::WalkOption::default();
                    assert_eq!(
                        crate::io::hirsel_rt_v2_file_walk_step(
                            ctx,
                            environment,
                            0,
                            0,
                            std::ptr::null_mut(),
                            &mut step
                        ),
                        STATUS_OK
                    );
                    match expected {
                        Some(path) => {
                            assert_eq!(step.tag, 1);
                            assert_eq!(step.value.tag, 0);
                            assert_eq!(crate::paths::names(ctx).path_text(step.value.value), path);
                        }
                        None => assert_eq!(step.tag, 0),
                    }
                }
                crate::release::hirsel_rt_v2_release(
                    ctx,
                    environment,
                    (*environment).index(),
                    std::ptr::null(),
                    0,
                    0,
                );
                assert!((*ctx).native_host_result.is_none());
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(outcome.pending, 0);
    }
}
