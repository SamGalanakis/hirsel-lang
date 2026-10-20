//! Scripted streaming children (docs/spec/core-library.md#processes, scripted runs): every
//! provider plays `test.process` and process.script@1 children with [`ScriptChild`], and
//! the connected stages of a spawned pipeline with [`ScriptGroup`].
use crate::net::byte_literal;
use crate::process::{ChildRun, ChildStep, ExitStatus, GroupOutput, ProcessFailure};
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

/// Who writes a scripted child's input: the program, or a group's previous stage.
#[derive(Clone, Copy, Debug)]
enum Writer<'a> {
    Program,
    Stage(&'a str),
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
        script.progress(Writer::Program);
        script
    }
    /// Play every step that needs no more input: the Test report of an `Expect` the
    /// bytes `writer` wrote contradict, if one does.
    fn progress(&mut self, writer: Writer<'_>) -> Option<String> {
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
                        return Some(mismatch(&self.program, expected, wrote, writer));
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
        self.feed(data, Writer::Program)
    }
    fn feed(
        &mut self,
        data: &[u8],
        writer: Writer<'_>,
    ) -> Result<Result<(), ProcessFailure>, String> {
        if data.is_empty() {
            return Ok(Ok(()));
        }
        if self.exit.is_some() {
            return Ok(Err(ProcessFailure::InputClosed));
        }
        self.written.extend_from_slice(data);
        match self.progress(writer) {
            Some(report) => Err(report),
            None => Ok(Ok(())),
        }
    }
    /// The program closed the child's input: the Test report when an `Expect` is unmet.
    pub fn close_stdin(&self) -> Option<String> {
        self.close_input(Writer::Program)
    }
    fn close_input(&self, writer: Writer<'_>) -> Option<String> {
        match (self.exit, self.steps.get(self.step)) {
            (None, Some(ChildStep::Expect(expected))) if !self.failed => Some(unmet(
                &self.program,
                expected,
                &self.written[self.consumed..],
                writer,
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

/// The Test report of a scripted child whose `Expect` its input contradicts, with its
/// trailing newline.
fn mismatch(program: &str, expected: &[u8], wrote: &[u8], writer: Writer<'_>) -> String {
    let (expected, wrote) = (byte_literal(expected), byte_literal(wrote));
    match writer {
        Writer::Program => {
            format!(
                "test failure: child {program} expected {expected} but the program wrote {wrote}\n"
            )
        }
        Writer::Stage(stage) => format!(
            "test failure: child {program} expected {expected} but its input stage {stage} wrote {wrote}\n"
        ),
    }
}

/// The Test report of a scripted child's input closed at an unmet `Expect`.
fn unmet(program: &str, expected: &[u8], wrote: &[u8], writer: Writer<'_>) -> String {
    let (expected, wrote) = (byte_literal(expected), byte_literal(wrote));
    match writer {
        Writer::Program => format!(
            "test failure: child {program} expected {expected} but the program closed its input after writing {wrote}\n"
        ),
        Writer::Stage(stage) => format!(
            "test failure: child {program} expected {expected} but its input stage {stage} closed its output after writing {wrote}\n"
        ),
    }
}

/// One spawned scripted pipeline group (`proc.spawn_pipeline`): its stages play as spawned
/// children, connected. The program writes the first stage's input and reads the last
/// stage's output; the bytes every other stage prints are written to the next stage's input
/// as soon as they are available, and the end of a stage closes the next stage's input. Each
/// stage's standard error is captured, bounded by `max_stderr`: a stage past it ends every
/// stage, as a kill does, and the group's wait is `OutputLimit()`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScriptGroup {
    stages: Vec<ScriptChild>,
    /// Whether each stage's input is open.
    open: Vec<bool>,
    stderr: Vec<Vec<u8>>,
    max_stderr: u64,
    over_limit: bool,
    /// A report already failed the run: later closes report nothing more.
    failed: bool,
}

impl ScriptGroup {
    /// The group of nonempty `stages`, and the Test report of an `Expect` that a stage's
    /// first output contradicts.
    pub fn new(stages: Vec<ChildRun>, max_stderr: u64) -> (Self, Option<String>) {
        let count = stages.len();
        let mut group = Self {
            stages: stages.into_iter().map(ScriptChild::new).collect(),
            open: vec![true; count],
            stderr: vec![vec![]; count],
            max_stderr,
            over_limit: false,
            failed: false,
        };
        let report = group.settle();
        (group, report)
    }
    /// Move every stage's new output on, in stage order, so each stage's input holds what
    /// the stage before it printed: the Test report of the first `Expect` that contradicts
    /// it, or that the end of its input stage leaves unmet.
    fn settle(&mut self) -> Option<String> {
        for i in 0..self.stages.len() {
            let stderr: Vec<u8> = self.stages[i].stderr.drain(..).flatten().collect();
            self.stderr[i].extend_from_slice(&stderr);
            if self.stderr[i].len() as u64 > self.max_stderr && !self.over_limit {
                self.over_limit = true;
                self.kill(Signal::Kill);
            }
            if i + 1 == self.stages.len() {
                break;
            }
            let stdout: Vec<u8> = self.stages[i].stdout.drain(..).flatten().collect();
            let (upstream, downstream) = self.stages.split_at_mut(i + 1);
            let writer = Writer::Stage(&upstream[i].program);
            // An ended stage discards what it is written, as a closed pipe does.
            if let Err(report) = downstream[0].feed(&stdout, writer) {
                return self.report(report);
            }
            if upstream[i].exit.is_some() && self.open[i + 1] {
                self.open[i + 1] = false;
                if let Some(report) = downstream[0].close_input(writer) {
                    return self.report(report);
                }
            }
        }
        None
    }
    fn report(&mut self, report: String) -> Option<String> {
        (!std::mem::replace(&mut self.failed, true)).then_some(report)
    }
    /// `group.write` of `data` to the first stage's open input, as [`ScriptChild::write`].
    pub fn write(&mut self, data: &[u8]) -> Result<Result<(), ProcessFailure>, String> {
        let written = match self.stages[0].feed(data, Writer::Program) {
            Ok(written) => written,
            Err(report) => {
                self.failed = true;
                return Err(report);
            }
        };
        match self.settle() {
            Some(report) => Err(report),
            None => Ok(written),
        }
    }
    /// The program closed the first stage's input: the Test report of an `Expect` that
    /// leaves unmet, in that stage or (once it ends) a later one.
    pub fn close_stdin(&mut self) -> Option<String> {
        if std::mem::replace(&mut self.open[0], false)
            && !self.failed
            && let Some(report) = self.stages[0].close_input(Writer::Program)
        {
            return self.report(report);
        }
        self.settle()
    }
    /// At most `max` (positive) bytes of the next chunk of the last stage's output.
    pub fn read(&mut self, max: u64) -> ScriptRead {
        let last = self.stages.len() - 1;
        self.stages[last].read(ChildStream::Stdout, max)
    }
    /// `group.kill`: every stage ends at once without checking an `Expect`.
    pub fn kill(&mut self, signal: Signal) {
        for stage in &mut self.stages {
            stage.kill(signal);
        }
    }
    /// `proc.wait_group` after the program's input closed with no report: every stage's
    /// status and captured standard error, or `OutputLimit()` when a stage passed the bound.
    pub fn output(&self) -> Result<GroupOutput, ProcessFailure> {
        if self.over_limit {
            return Err(ProcessFailure::OutputLimit);
        }
        Ok(GroupOutput {
            statuses: self
                .stages
                .iter()
                .map(|stage| stage.exit.expect("every stage of a closed group ended"))
                .collect(),
            stderr: self.stderr.clone(),
        })
    }
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
