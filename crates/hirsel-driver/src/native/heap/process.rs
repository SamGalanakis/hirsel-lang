//! Driver-only conversion between H semantic values and the runtime's process model.
//! All policy and scripted runs live in hirsel-rt; the model never spawns a program.
use super::{Io, IoGrant, io_ok};
use hirsel_heapcheck::{OpOutcome, OpValue};
use hirsel_lir::layout_types::ScalarKind;
use hirsel_rt::io::{Completion as C, Request as R};
use hirsel_rt::process::{ExitStatus, ProcessFailure, ProcessOutcome, ProcessRun};

fn bytes(v: &OpValue) -> Option<Vec<u8>> {
    match v {
        OpValue::Bytes(b) => Some(b.clone()),
        OpValue::Elements {
            scalar: ScalarKind::U8,
            bits,
        } => bits.iter().map(|&b| u8::try_from(b).ok()).collect(),
        _ => None,
    }
}
fn text(v: &OpValue) -> Option<String> {
    String::from_utf8(bytes(v)?).ok()
}
fn strings(v: &OpValue) -> Option<Vec<String>> {
    match v {
        OpValue::Sequence(items) => items.iter().map(text).collect(),
        _ => None,
    }
}
fn word(v: &OpValue) -> Option<u64> {
    match v {
        OpValue::Bits { bits, .. } => u64::try_from(*bits).ok(),
        _ => None,
    }
}
fn variant(v: &OpValue) -> Option<(u32, &OpValue)> {
    match v {
        OpValue::Variant { tag, value } => Some((*tag, value)),
        _ => None,
    }
}
fn v(tag: u32, value: OpValue) -> OpValue {
    OpValue::Variant {
        tag,
        value: Box::new(value),
    }
}
fn failure(f: ProcessFailure) -> OpValue {
    match f {
        ProcessFailure::NotGranted => v(0, OpValue::Unit),
        ProcessFailure::InvalidInput => v(1, OpValue::Unit),
        ProcessFailure::NotFound => v(2, OpValue::Unit),
        ProcessFailure::PermissionDenied => v(3, OpValue::Unit),
        ProcessFailure::OutputLimit => v(4, OpValue::Unit),
        ProcessFailure::Fault(code) => v(
            5,
            OpValue::Bits {
                scalar: ScalarKind::U32,
                bits: code.into(),
            },
        ),
    }
}
fn script(v: &OpValue) -> Option<ProcessRun> {
    let OpValue::Aggregate(fields) = v else {
        return None;
    };
    let [program, args, outcome] = fields.as_slice() else {
        return None;
    };
    let outcome = match variant(outcome)? {
        (0, OpValue::Aggregate(output)) => {
            let [status, stdout, stderr] = output.as_slice() else {
                return None;
            };
            let (tag, code) = variant(status)?;
            let code = word(code)? as u32 as i32;
            ProcessOutcome::Output {
                status: match tag {
                    0 => ExitStatus::Exited(code),
                    1 => ExitStatus::Signaled(code),
                    _ => return None,
                },
                stdout: bytes(stdout)?,
                stderr: bytes(stderr)?,
            }
        }
        (1, error) => ProcessOutcome::Error(match variant(error)? {
            (0, _) => ProcessFailure::NotGranted,
            (1, _) => ProcessFailure::InvalidInput,
            (2, _) => ProcessFailure::NotFound,
            (3, _) => ProcessFailure::PermissionDenied,
            (4, _) => ProcessFailure::OutputLimit,
            (5, code) => ProcessFailure::Fault(word(code)? as u32),
            _ => return None,
        }),
        _ => return None,
    };
    Some(ProcessRun {
        program: text(program)?,
        args: strings(args)?,
        outcome,
    })
}

impl Io {
    pub(super) fn process(
        &mut self,
        module: Option<&str>,
        name: &str,
        args: &[OpValue],
    ) -> Option<OpOutcome> {
        let request = match (module, name, args) {
            (Some("test"), "process", [OpValue::Handle(test), runs])
                if matches!(self.grants.get(test), Some(IoGrant::Test)) =>
            {
                let OpValue::Sequence(runs) = runs else {
                    return None;
                };
                R::TestProcess {
                    test: 1,
                    runs: runs.iter().map(script).collect::<Option<_>>()?,
                }
            }
            (Some("io"), "process", [OpValue::Handle(1)]) => R::Process { io: 1 },
            (Some("proc"), "run", [p, OpValue::Handle(path), argv, stdin, dir, max]) => {
                // The model's Dir handles are H's own; check one here. A scripted world
                // does not consult it, and the model has no host world to start one in.
                match variant(dir)? {
                    (0, _) => {}
                    (1, OpValue::Handle(d))
                        if matches!(self.grants.get(d), Some(IoGrant::Dir { .. })) => {}
                    _ => return Some(OpOutcome::Fail("CAP-2".into())),
                }
                R::ProcessRun {
                    process: self.network_id(p)?,
                    program: self.paths.get(path.checked_sub(1)? as usize)?.clone(),
                    args: strings(argv)?,
                    stdin: bytes(stdin)?,
                    directory: None,
                    max_output: word(max)?,
                }
            }
            _ => return None,
        };
        let completion = match self.network.call(request) {
            Ok(c) => c,
            Err(_) => return Some(OpOutcome::Unsupported),
        };
        Some(OpOutcome::Value(match completion {
            C::Handle(Ok(id)) if module == Some("test") => self.network_handle(id, false),
            C::Handle(Ok(id)) => io_ok(self.network_handle(id, false)),
            C::Handle(Err(_)) => v(1, v(0, OpValue::Unit)),
            C::Process(ProcessOutcome::Output {
                status,
                stdout,
                stderr,
            }) => {
                let (tag, code) = match status {
                    ExitStatus::Exited(code) => (0, code),
                    ExitStatus::Signaled(signal) => (1, signal),
                };
                io_ok(OpValue::Aggregate(vec![
                    v(
                        tag,
                        OpValue::Bits {
                            scalar: ScalarKind::I32,
                            bits: u128::from(code as u32),
                        },
                    ),
                    OpValue::ByteSequence(stdout),
                    OpValue::ByteSequence(stderr),
                ]))
            }
            C::Process(ProcessOutcome::Error(f)) => v(1, failure(f)),
            _ => return Some(OpOutcome::Unsupported),
        }))
    }
}
