//! Diagnostic timing, outside the replay journal and all program-visible state.
//! The epoch/elapsed structure follows tracing-subscriber 9d0e557728ec,
//! src/fmt/time/mod.rs:118-150 (Uptime). Unlike its text formatter we retain
//! integer nanoseconds and bind event ordinals to the exact recording digest.
use crate::io::Event;
use crate::journal::{Journal, Prepared};
use hirsel_run_schema::RunOutcome;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Write},
    path::PathBuf,
    time::Instant,
};

pub(crate) struct TimingJournal {
    inner: Box<dyn Journal>,
    tape: PathBuf,
    output: File,
    capture: String,
    epoch: Instant,
    samples: Vec<u64>,
    saved: bool,
}
impl TimingJournal {
    pub(crate) fn new(
        inner: Box<dyn Journal>,
        tape: PathBuf,
        output: File,
        capture: String,
    ) -> Self {
        Self {
            inner,
            tape,
            output,
            capture,
            epoch: Instant::now(),
            samples: vec![],
            saved: false,
        }
    }
    // Diagnostic storage failure must never become an application completion or
    // prevent a journal barrier. A missing/partial sidecar provides no timing.
    fn save(&mut self) -> io::Result<()> {
        if self.saved {
            return Ok(());
        }
        let bytes = std::fs::read(&self.tape)?;
        let count = if crate::journal::is_journal(&bytes) {
            crate::journal::recover(&bytes)
                .map_err(|e| io::Error::other(e.to_string()))?
                .recording
                .events
                .len()
        } else {
            let end = crate::tape_binary::confirmed_prefix(&bytes).map_err(io::Error::other)?;
            crate::tape_binary::parse_prefix(&bytes[..end])
                .map_err(io::Error::other)?
                .0
                .events
                .len()
        };
        let digest = hirsel_run_schema::hex(&Sha256::digest(&bytes));
        // capture is restricted to safe ASCII by the host tooling configuration.
        let mut text = format!(
            "{{\"schema\":\"hirsel-observation-timing\",\"version\":1,\"evidence\":{{\"capture\":\"{}\",\"sha256\":\"{}\"}},\"clock\":\"host-monotonic-ns\",\"samples\":[",
            self.capture, digest
        );
        for (ordinal, ns) in self.samples.iter().take(count).enumerate() {
            if ordinal != 0 {
                text.push(',');
            }
            use std::fmt::Write;
            write!(text, "{{\"ordinal\":{ordinal},\"ns\":{ns}}}").expect("String write");
        }
        text.push_str("]}\n");
        self.saved = true;
        self.output.write_all(text.as_bytes())?;
        self.output.flush()?;
        Ok(())
    }
}
impl Drop for TimingJournal {
    fn drop(&mut self) {
        let _ = self.save();
    }
}
impl Journal for TimingJournal {
    fn records(&self) -> bool {
        self.inner.records()
    }
    fn durable(&self) -> bool {
        self.inner.durable()
    }
    fn prepare(&mut self, prepared: &Prepared) -> io::Result<()> {
        self.inner.prepare(prepared)
    }
    fn event(&mut self, id: Option<u64>, event: &Event) -> io::Result<()> {
        let ns = u64::try_from(self.epoch.elapsed().as_nanos()).ok();
        self.inner.event(id, event)?;
        // More than 584 years of recording cannot be represented by v1. Stop
        // sampling rather than wrap or invent a timestamp.
        if let Some(ns) = ns {
            self.samples.push(ns);
        }
        Ok(())
    }
    fn withdraw(&mut self, id: u64) -> io::Result<()> {
        self.inner.withdraw(id)
    }
    fn barrier(&mut self) -> io::Result<()> {
        self.inner.barrier()
    }
    fn pending(&self) -> usize {
        self.inner.pending()
    }
    fn end(&mut self, outcome: &RunOutcome) -> io::Result<()> {
        self.inner.end(outcome)?;
        let _ = self.save();
        Ok(())
    }
}

/// Tool configuration only. No field enters grants, a run identity or the tape.
/// Replay never calls this function, and the application environment is explicit.
pub(crate) fn capture_id() -> Option<String> {
    if std::env::var_os("HIRSEL_TIMING").is_some_and(|v| v == "off") {
        return None;
    }
    if let Ok(id) = std::env::var("HIRSEL_TIMING_CAPTURE_ID") {
        return (!id.is_empty()
            && id.len() <= 256
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)))
        .then_some(id);
    }
    crate::io::RandomSource::System
        .draw(16)
        .ok()
        .map(|bytes| hirsel_run_schema::hex(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        io::{Adapter, Completion, Request, Tree},
        io_real::RealIo,
        journal::{DurableJournal, TapeJournal},
        launch::{Recording, RecordingWriter},
    };
    use std::collections::BTreeMap;

    fn header() -> Recording {
        Recording {
            arguments: vec![],
            run_identity: [0; 32],
            hash_seed: 0,
            random_seed: 0,
            test_seed: 0,
            binding: None,
            tree: Tree::new(BTreeMap::new()).unwrap(),
            events: vec![],
        }
    }
    fn run(
        root: &std::path::Path,
        name: &str,
        durable: bool,
        timing: bool,
        finish: bool,
    ) -> (Vec<u8>, Vec<u8>, usize) {
        let tape = root.join(name);
        let file = File::create(&tape).unwrap();
        let journal: Box<dyn Journal> = if durable {
            Box::new(DurableJournal::create(file, &header()).unwrap())
        } else {
            Box::new(TapeJournal(RecordingWriter::new(file, &header()).unwrap()))
        };
        let mut io = RealIo::grant(root).unwrap();
        if timing {
            let output = File::create(root.join(format!("{name}.timing"))).unwrap();
            io.journal_to(Box::new(TimingJournal::new(
                journal,
                tape.clone(),
                output,
                "fixture".into(),
            )));
        } else {
            io.journal_to(journal);
        }
        let mut parent = vec![0; 24];
        parent.extend_from_slice(&[1, 7, 7]);
        for request in [
            Request::Telemetry { io: 1 },
            Request::TelemetryBegin {
                telemetry: 2,
                parent,
                spec: b"spec".to_vec(),
            },
            Request::TelemetryEvent {
                telemetry: 2,
                span: 3,
                payload: b"event".to_vec(),
            },
            Request::TelemetryLog {
                telemetry: 2,
                payload: b"log".to_vec(),
            },
            Request::TelemetryMeasure {
                telemetry: 2,
                payload: b"measure".to_vec(),
            },
            Request::TelemetryFinish {
                telemetry: 2,
                span: 3,
                status: b"ok".to_vec(),
            },
            Request::TelemetryBegin {
                telemetry: 2,
                parent: {
                    let mut p = vec![0; 24];
                    p.extend_from_slice(&[1, 7, 7]);
                    p
                },
                spec: b"abandon".to_vec(),
            },
            Request::Close { handle: 4 },
            Request::Stdout { io: 1 },
            Request::Print {
                stdout: 5,
                bytes: b"same output\n".to_vec(),
            },
        ] {
            let c = io.call(request).unwrap();
            assert!(!matches!(c, Completion::Blocked { .. }));
        }
        if finish {
            io.finish_recording(&RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                .unwrap();
        }
        let stdout = io.stdout().to_vec();
        drop(io);
        let bytes = std::fs::read(tape).unwrap();
        (bytes, stdout, 10)
    }

    #[test]
    fn sidecar_on_and_off_have_identical_ordinary_and_durable_tapes_and_stdout() {
        let root = std::env::temp_dir().join(format!("timing-journal-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        for durable in [false, true] {
            for finish in [false, true] {
                let off = run(&root, "off", durable, false, finish);
                let on = run(&root, "on", durable, true, finish);
                assert_eq!(off, on, "durable={durable}, finish={finish}");
                assert_eq!(on.1, b"same output\n");
                let text = std::fs::read_to_string(root.join("on.timing")).unwrap();
                assert!(text.contains(&hirsel_run_schema::hex(&Sha256::digest(&on.0))));
                assert!(text.contains("\"clock\":\"host-monotonic-ns\""));
                for ordinal in 0..on.2 {
                    assert!(text.contains(&format!("\"ordinal\":{ordinal},")));
                }
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn diagnostic_write_failure_does_not_fail_the_journal() {
        let root =
            std::env::temp_dir().join(format!("timing-write-failure-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let tape = root.join("tape");
        let journal =
            TapeJournal(RecordingWriter::new(File::create(&tape).unwrap(), &header()).unwrap());
        let mut timed = TimingJournal::new(
            Box::new(journal),
            tape.clone(),
            std::fs::OpenOptions::new()
                .write(true)
                .open("/dev/full")
                .unwrap(),
            "fixture".into(),
        );
        timed
            .end(&RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            .unwrap();
        assert!(
            crate::launch::parse_recording(&crate::launch::read_recording_text(tape).unwrap())
                .is_ok()
        );
        drop(timed);
        std::fs::remove_dir_all(root).unwrap();
    }
}
