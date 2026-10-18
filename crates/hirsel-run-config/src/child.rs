//! Scripted streaming children (docs/spec/core-library.md#processes, scripted runs): every
//! provider plays `test.process` and process.script@1 children with [`ScriptChild`].
use crate::net::byte_literal;
use crate::process::{ChildRun, ChildStep, ExitStatus, ProcessFailure};
use std::collections::VecDeque;

/// Which output stream of a child a read takes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum ChildStream {
    Stdout,
    Stderr,
}

/// `Signal`, in its docs/spec/core-library.md variant order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Signal {
    Interrupt,
    Terminate,
    Kill,
}
impl Signal {
    /// The signal's Linux number, the one `Signaled` reports.
    pub fn number(self) -> i32 {
        match self {
            Signal::Interrupt => 2,
            Signal::Terminate => 15,
            Signal::Kill => 9,
        }
    }
    /// The signal of variant ordinal `ordinal`.
    pub fn of_ordinal(ordinal: u32) -> Option<Self> {
        [Signal::Interrupt, Signal::Terminate, Signal::Kill]
            .get(ordinal as usize)
            .copied()
    }
}

/// One spawned scripted child. Its steps play in order: `Print` and `PrintErr` make bytes
/// available at once, `Expect` waits for the program's input, and `Exit` (or the end of
/// the steps) ends the child.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScriptChild {
    program: String,
    steps: Vec<ChildStep>,
    step: usize,
    /// Available output, one chunk per step: a read never crosses into the next.
    stdout: VecDeque<Vec<u8>>,
    stderr: VecDeque<Vec<u8>>,
    written: Vec<u8>,
    /// The written bytes earlier `Expect` steps consumed.
    consumed: usize,
    /// The child's status once it ended, by its script or a kill.
    exit: Option<ExitStatus>,
    /// A contradicted `Expect` already failed the run: closing input reports nothing more.
    failed: bool,
}

/// A scripted read: the bytes it delivers (empty at end of stream), or none available yet.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScriptRead {
    Bytes(Vec<u8>),
    Waiting,
}

impl ScriptChild {
    pub fn new(child: ChildRun) -> Self {
        let mut script = Self {
            program: child.program,
            steps: child.steps,
            step: 0,
            stdout: VecDeque::new(),
            stderr: VecDeque::new(),
            written: vec![],
            consumed: 0,
            exit: None,
            failed: false,
        };
        script.progress();
        script
    }
    /// Play every step that needs no more input: the Test report of an `Expect` the
    /// written bytes contradict, if one does.
    fn progress(&mut self) -> Option<String> {
        while self.exit.is_none() {
            match self.steps.get(self.step) {
                None => self.exit = Some(ExitStatus::Exited(0)),
                Some(ChildStep::Print(b)) if !b.is_empty() => self.stdout.push_back(b.clone()),
                Some(ChildStep::PrintErr(b)) if !b.is_empty() => self.stderr.push_back(b.clone()),
                Some(ChildStep::Print(_) | ChildStep::PrintErr(_)) => {}
                Some(ChildStep::Expect(expected)) => {
                    let wrote = &self.written[self.consumed..];
                    if wrote.len() < expected.len() {
                        return None;
                    }
                    let wrote = &wrote[..expected.len()];
                    if wrote != expected.as_slice() {
                        self.failed = true;
                        return Some(child_mismatch(&self.program, expected, wrote));
                    }
                    self.consumed += expected.len();
                }
                Some(ChildStep::Exit(status)) => self.exit = Some(*status),
            }
            self.step += 1;
        }
        None
    }
    /// `child.write` of `data` to open input: accepted, or `InputClosed()` once the child
    /// ended. `Err` is the Test report of a contradicted `Expect` (LANG-9).
    pub fn write(&mut self, data: &[u8]) -> Result<Result<(), ProcessFailure>, String> {
        if data.is_empty() {
            return Ok(Ok(()));
        }
        if self.exit.is_some() {
            return Ok(Err(ProcessFailure::InputClosed));
        }
        self.written.extend_from_slice(data);
        match self.progress() {
            Some(report) => Err(report),
            None => Ok(Ok(())),
        }
    }
    /// The program closed the child's input: the Test report when an `Expect` is unmet.
    pub fn close_stdin(&self) -> Option<String> {
        match (self.exit, self.steps.get(self.step)) {
            (None, Some(ChildStep::Expect(expected))) if !self.failed => Some(child_unmet(
                &self.program,
                expected,
                &self.written[self.consumed..],
            )),
            _ => None,
        }
    }
    /// At most `max` (positive) bytes of the next chunk of `stream`.
    pub fn read(&mut self, stream: ChildStream, max: u64) -> ScriptRead {
        let chunks = match stream {
            ChildStream::Stdout => &mut self.stdout,
            ChildStream::Stderr => &mut self.stderr,
        };
        match chunks.front_mut() {
            Some(chunk) => {
                let n = chunk.len().min(usize::try_from(max).unwrap_or(usize::MAX));
                let bytes: Vec<u8> = chunk.drain(..n).collect();
                if chunk.is_empty() {
                    chunks.pop_front();
                }
                ScriptRead::Bytes(bytes)
            }
            None if self.exit.is_some() => ScriptRead::Bytes(vec![]),
            None => ScriptRead::Waiting,
        }
    }
    /// `child.kill`: any signal ends the script at once without checking an `Expect`; an
    /// ended child keeps its status.
    pub fn kill(&mut self, signal: Signal) {
        self.exit
            .get_or_insert(ExitStatus::Signaled(signal.number()));
    }
    /// The ended child's status: after its input closed with no unmet `Expect`, always
    /// present.
    pub fn status(&self) -> Option<ExitStatus> {
        self.exit
    }
}

/// The Test report of a scripted child whose `Expect` the program's input contradicts,
/// with its trailing newline.
pub fn child_mismatch(program: &str, expected: &[u8], wrote: &[u8]) -> String {
    format!(
        "test failure: child {program} expected {} but the program wrote {}\n",
        byte_literal(expected),
        byte_literal(wrote)
    )
}

/// The Test report of a scripted child's input closed at an unmet `Expect`.
pub fn child_unmet(program: &str, expected: &[u8], wrote: &[u8]) -> String {
    format!(
        "test failure: child {program} expected {} but the program closed its input after writing {}\n",
        byte_literal(expected),
        byte_literal(wrote)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn child(steps: Vec<ChildStep>) -> ScriptChild {
        ScriptChild::new(ChildRun {
            program: "/bin/cat".into(),
            args: vec![],
            steps,
        })
    }

    #[test]
    fn steps_play_until_an_expect_and_reads_never_cross_a_step() {
        let mut c = child(vec![
            ChildStep::Print(b"ready\n".to_vec()),
            ChildStep::Print(b"go".to_vec()),
            ChildStep::Expect(b"ping\n".to_vec()),
            ChildStep::Print(b"pong\n".to_vec()),
            ChildStep::Exit(ExitStatus::Exited(3)),
        ]);
        assert_eq!(
            c.read(ChildStream::Stdout, 100),
            ScriptRead::Bytes(b"ready\n".to_vec())
        );
        assert_eq!(
            c.read(ChildStream::Stdout, 1),
            ScriptRead::Bytes(b"g".to_vec())
        );
        assert_eq!(
            c.read(ChildStream::Stdout, 100),
            ScriptRead::Bytes(b"o".to_vec())
        );
        assert_eq!(c.read(ChildStream::Stdout, 100), ScriptRead::Waiting);
        assert_eq!(c.read(ChildStream::Stderr, 100), ScriptRead::Waiting);
        assert_eq!(c.write(b"pi"), Ok(Ok(())));
        assert_eq!(c.read(ChildStream::Stdout, 100), ScriptRead::Waiting);
        assert_eq!(c.write(b"ng\nextra"), Ok(Ok(())));
        assert_eq!(
            c.read(ChildStream::Stdout, 100),
            ScriptRead::Bytes(b"pong\n".to_vec())
        );
        assert_eq!(c.read(ChildStream::Stdout, 100), ScriptRead::Bytes(vec![]));
        assert_eq!(c.write(b"x"), Ok(Err(ProcessFailure::InputClosed)));
        assert_eq!(c.write(b""), Ok(Ok(())));
        assert_eq!(c.close_stdin(), None);
        assert_eq!(c.status(), Some(ExitStatus::Exited(3)));
    }

    #[test]
    fn a_contradicted_or_unmet_expect_reports_and_a_kill_skips_it() {
        let expect = || child(vec![ChildStep::Expect(b"ping".to_vec())]);
        assert_eq!(
            expect().write(b"pong"),
            Err(
                "test failure: child /bin/cat expected b\"ping\" but the program wrote b\"pong\"\n"
                    .into()
            )
        );
        let mut c = expect();
        assert_eq!(c.write(b"pi"), Ok(Ok(())));
        assert_eq!(
            c.close_stdin(),
            Some("test failure: child /bin/cat expected b\"ping\" but the program closed its input after writing b\"pi\"\n".into())
        );
        c.kill(Signal::Terminate);
        assert_eq!(c.close_stdin(), None);
        assert_eq!(c.status(), Some(ExitStatus::Signaled(15)));
        let mut ended = child(vec![]);
        ended.kill(Signal::Kill);
        assert_eq!(ended.status(), Some(ExitStatus::Exited(0)));
    }
}
