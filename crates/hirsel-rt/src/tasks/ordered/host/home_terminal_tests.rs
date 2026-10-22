use super::*;
use crate::io::{Adapter, AdapterError, Completion, Request};
use std::{
    cell::Cell,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

struct Home {
    run: Rc<Cell<*mut crate::context::RunShared>>,
    dropped: Arc<AtomicUsize>,
    home: std::thread::ThreadId,
    effect: u8,
    written: Arc<Mutex<Vec<u8>>>,
}
impl Drop for Home {
    fn drop(&mut self) {
        assert_eq!(self.home, std::thread::current().id());
        self.dropped.fetch_add(1, Ordering::Release);
    }
}
impl Adapter for Home {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        assert_eq!(self.home, std::thread::current().id());
        match request {
            Request::Stdin { .. } => Ok(Completion::Handle(Ok(12))),
            Request::Stdout { .. } => Ok(Completion::Handle(Ok(13))),
            Request::ReadStdin { stdin: 12, .. } => {
                // The completed bounded callback owns these bytes. Stop wins
                // before call_home_in can reacquire the caller's permit.
                unsafe {
                    (*self.run.get())
                        .control
                        .request_inside(crate::stop::Stop::Requested)
                };
                Ok(Completion::Input(Ok(vec![0, 255, b'x'])))
            }
            Request::Close { handle: 12 } => {
                unsafe {
                    (*self.run.get())
                        .control
                        .request_inside(crate::stop::Stop::Requested)
                };
                Ok(Completion::Unit(Ok(())))
            }
            _ => panic!("unexpected {request:?}"),
        }
    }
    fn print_observed(
        &mut self,
        handle: u64,
        stderr: bool,
        bytes: &[u8],
    ) -> (Result<Completion, AdapterError>, Option<u64>) {
        assert_eq!(self.home, std::thread::current().id());
        assert_eq!((handle, stderr), (13, false));
        self.written.lock().unwrap().extend_from_slice(&bytes[..1]);
        unsafe {
            (*self.run.get())
                .control
                .request_inside(crate::stop::Stop::Requested)
        };
        (
            Ok(Completion::Unit(Err(crate::io::IoError::Fault(1)))),
            if self.effect == 3 { None } else { Some(1) },
        )
    }
}

#[test]
fn ordered_host_terminal_retains_home_completion_before_stopped_permit_reacquisition() {
    home_terminal(0);
}

#[test]
fn ordered_host_terminal_retains_exact_partial_home_output_and_refuses_unknown_prefix() {
    home_terminal(1);
    home_terminal(3);
}

#[test]
fn ordered_host_terminal_implicit_close_uses_observed_owner_before_stop() {
    home_terminal(2);
}

fn home_terminal(effect: u8) {
    let mut reference = None;
    for width in [1, 2, 4, 8] {
        for reorder in [
            hirsel_run_config::Reorder::Off,
            hirsel_run_config::Reorder::Auto,
        ] {
            let pointer = Rc::new(Cell::new(std::ptr::null_mut()));
            let setup = pointer.clone();
            let dropped = Arc::new(AtomicUsize::new(0));
            let written = Arc::new(Mutex::new(Vec::new()));
            let mut run = crate::host::BodyRun::new(width, true);
            run.adapter = Some(Box::new(Home {
                run: pointer,
                dropped: dropped.clone(),
                home: std::thread::current().id(),
                effect,
                written: written.clone(),
            }));
            unsafe { run.use_physical_dispatch() };
            let (outcome, trace) = run.run_traced_after_bounded_stop(
                move |ctx| unsafe {
                    let mut policy = crate::ordered::Policy::admit(&Default::default(), None);
                    policy.reorder = reorder;
                    engine::enable(ctx, policy);
                    super::super::events::enable(ctx);
                    super::super::events::initial_host(ctx, Some(1), None);
                    setup.set((*ctx).run_ptr());
                },
                move |ctx| unsafe {
                    let _role = role::Guard::new(ctx);
                    if effect == 1 || effect == 3 {
                        assert_eq!(
                            crate::io::capability_call(ctx, Request::Stdout { io: 1 }),
                            Ok(Completion::Handle(Ok(13)))
                        );
                        let _ = crate::io::capability_call(
                            ctx,
                            Request::Print {
                                stdout: 13,
                                bytes: vec![0, 255, b'x'],
                            },
                        );
                        panic!("stopped print returned to code")
                    }
                    assert_eq!(
                        crate::io::capability_call(ctx, Request::Stdin { io: 1 }),
                        Ok(Completion::Handle(Ok(12)))
                    );
                    if effect == 2 {
                        crate::io::hirsel_rt_v2_io_close(ctx, 12, 0, 0);
                        panic!("stopped implicit close returned to code")
                    }
                    let _ =
                        crate::io::capability_call(ctx, Request::ReadStdin { stdin: 12, max: 3 });
                    panic!("stopped home completion returned to code")
                },
            );
            assert_eq!(outcome.ended, Err(crate::stop::Stop::Requested));
            assert!(outcome.pending <= 1);
            assert_eq!(dropped.load(Ordering::Acquire), 1);
            if effect == 1 || effect == 3 {
                assert_eq!(*written.lock().unwrap(), [0]);
            }
            if effect == 3 {
                assert!(trace.is_err(), "stopped output cannot infer its prefix");
                continue;
            }
            let trace = trace.expect("home result survives the stopped transfer");
            let rows: Vec<serde_json::Value> = std::str::from_utf8(trace.bytes())
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(
                rows.iter()
                    .filter(|row| row["outcome"]["kind"] == "host_result")
                    .count(),
                2
            );
            assert!(
                rows.iter()
                    .any(|row| row["outcome"]["kind"] == "host_abandoned"
                        && row["outcome"]["phase"] == "result")
            );
            if effect == 1 {
                let outputs: Vec<_> = rows
                    .iter()
                    .filter(|row| row["outcome"]["kind"] == "host_output")
                    .collect();
                assert_eq!(outputs.len(), 1);
                assert_eq!(outputs[0]["outcome"]["bytes"], "00");
            }
            if let Some(reference) = &reference {
                assert_eq!(trace.bytes(), reference);
            } else {
                reference = Some(trace.bytes().to_vec());
            }
        }
    }
}
