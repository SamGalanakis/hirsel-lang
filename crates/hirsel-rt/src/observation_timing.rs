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
    io::{self, BufWriter, Read, Seek, SeekFrom, Write},
    path::PathBuf,
    time::Instant,
};

pub(crate) struct TimingJournal {
    inner: Box<dyn Journal + Send>,
    tape: PathBuf,
    output: BufWriter<File>,
    digest_offset: u64,
    position: u64,
    confirmed: u64,
    count: u64,
    failed: bool,
    epoch: Instant,
    saved: bool,
}
impl TimingJournal {
    pub(crate) fn new(
        inner: Box<dyn Journal + Send>,
        tape: PathBuf,
        output: File,
        capture: String,
    ) -> Self {
        let prefix = format!(
            "{{\"schema\":\"hirsel-observation-timing\",\"version\":1,\"evidence\":{{\"capture\":\"{capture}\",\"sha256\":\""
        );
        let digest_offset = prefix.len() as u64;
        let mut output = BufWriter::new(output);
        // Reserve the fixed-width digest, then stream samples straight to the sink.
        // A sidecar remains incomplete until save patches the digest and closes JSON.
        let failed = write!(
            output,
            "{prefix}{}\"}},\"clock\":\"host-monotonic-ns\",\"samples\":[",
            "0".repeat(64)
        )
        .is_err();
        let position =
            digest_offset + 64 + b"\"},\"clock\":\"host-monotonic-ns\",\"samples\":[".len() as u64;
        Self {
            inner,
            tape,
            output,
            digest_offset,
            position,
            confirmed: position,
            count: 0,
            failed,
            epoch: Instant::now(),
            saved: false,
        }
    }
    fn sample(&mut self, ns: u64) -> io::Result<()> {
        let sample = format!(
            "{}{{\"ordinal\":{},\"ns\":{ns}}}",
            if self.count == 0 { "" } else { "," },
            self.count
        );
        self.output.write_all(sample.as_bytes())?;
        self.position += sample.len() as u64;
        self.count += 1;
        Ok(())
    }
    // Diagnostic storage failure must never become an application completion or
    // prevent a journal barrier. A missing/partial sidecar provides no timing.
    fn save(&mut self) -> io::Result<()> {
        if self.saved || self.failed {
            return Ok(());
        }
        self.saved = true;
        let digest = tape_digest(&mut File::open(&self.tape)?)?;
        self.output.flush()?;
        // Durable events become evidence only after a successful journal barrier.
        // Discard unconfirmed samples on interrupted runs without decoding payloads.
        self.output.get_ref().set_len(self.confirmed)?;
        self.output.seek(SeekFrom::Start(self.digest_offset))?;
        self.output
            .write_all(hirsel_run_schema::hex(&digest).as_bytes())?;
        self.output.seek(SeekFrom::Start(self.confirmed))?;
        self.output.write_all(b"]}\n")?;
        self.output.flush()
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
        self.inner
            .prepare(prepared)
            .inspect_err(|_| self.failed = true)
    }
    fn event(&mut self, id: Option<u64>, event: &Event) -> io::Result<()> {
        let ns = u64::try_from(self.epoch.elapsed().as_nanos()).ok();
        self.inner
            .event(id, event)
            .inspect_err(|_| self.failed = true)?;
        // More than 584 years of recording cannot be represented by v1.
        if !self.failed
            && let Some(ns) = ns
        {
            self.failed = self.sample(ns).is_err();
            if !self.inner.durable() {
                self.confirmed = self.position;
            }
        }
        Ok(())
    }
    fn withdraw(&mut self, id: u64) -> io::Result<()> {
        self.inner.withdraw(id).inspect_err(|_| self.failed = true)
    }
    fn barrier(&mut self) -> io::Result<()> {
        self.inner.barrier().inspect_err(|_| self.failed = true)?;
        self.confirmed = self.position;
        Ok(())
    }
    fn pending(&self) -> usize {
        self.inner.pending()
    }
    fn end(&mut self, outcome: &RunOutcome) -> io::Result<()> {
        self.inner
            .end(outcome)
            .inspect_err(|_| self.failed = true)?;
        self.confirmed = self.position;
        let _ = self.save();
        Ok(())
    }
}

// Rust's io::copy stack-buffer loop (rust 1a2c4797ef25,
// library/alloc/src/io/copy/generic.rs:232-254, stack_buffer_copy): bounded reads,
// retry interrupted reads, stop at EOF. Feed SHA-256 instead of another writer.
fn tape_digest(input: &mut impl Read) -> io::Result<[u8; 32]> {
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        match input.read(&mut buffer) {
            Ok(0) => return Ok(hash.finalize().into()),
            Ok(n) => hash.update(&buffer[..n]),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
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
        let journal: Box<dyn Journal + Send> = if durable {
            Box::new(DurableJournal::create(file, &header()).unwrap())
        } else {
            Box::new(TapeJournal(RecordingWriter::new(file, &header()).unwrap()))
        };
        let mut io = RealIo::grant(root).unwrap();
        if timing {
            let output = File::create(root.join(format!("{name}.timing"))).unwrap();
            io.send_journal_to(Box::new(TimingJournal::new(
                journal,
                tape.clone(),
                output,
                "fixture".into(),
            )));
        } else {
            io.send_journal_to(journal);
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
        let root_scratch = tempfile::Builder::new()
            .prefix("timing-journal-")
            .tempdir()
            .unwrap();
        let root = root_scratch.path().to_path_buf();
        for durable in [false, true] {
            for finish in [false, true] {
                let off = run(&root, "off", durable, false, finish);
                let on = run(&root, "on", durable, true, finish);
                assert_eq!(off, on, "durable={durable}, finish={finish}");
                assert_eq!(on.1, b"same output\n");
                let text = std::fs::read_to_string(root.join("on.timing")).unwrap();
                let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                assert_eq!(value["samples"].as_array().unwrap().len(), on.2);
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
    fn digest_handles_short_interrupted_reads_and_copy_vectors() {
        struct ShortReader {
            input: std::io::Cursor<Vec<u8>>,
            interrupted: bool,
        }
        impl Read for ShortReader {
            fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
                if !self.interrupted {
                    self.interrupted = true;
                    return Err(io::ErrorKind::Interrupted.into());
                }
                self.interrupted = false;
                let size = bytes.len().min(1337);
                self.input.read(&mut bytes[..size])
            }
        }
        // Rust library/alloctests/tests/io/copy.rs:6-14, copy_copies, plus
        // empty input and a nonzero byte across the hash buffer boundary.
        for size in [0, 4, 1 << 17] {
            let mut bytes = vec![0; size];
            if size > 65536 {
                bytes[65536] = 42;
            }
            let expected: [u8; 32] = Sha256::digest(&bytes).into();
            let mut reader = ShortReader {
                input: std::io::Cursor::new(bytes),
                interrupted: false,
            };
            assert_eq!(tape_digest(&mut reader).unwrap(), expected);
        }
    }

    #[test]
    fn interrupted_durable_sidecar_keeps_only_barrier_confirmed_events() {
        let root_scratch = tempfile::Builder::new()
            .prefix("timing-barrier-")
            .tempdir()
            .unwrap();
        let root = root_scratch.path().to_path_buf();
        std::fs::create_dir_all(&root).unwrap();
        let tape = root.join("tape");
        let journal = DurableJournal::create(File::create(&tape).unwrap(), &header()).unwrap();
        let sidecar = root.join("timing");
        let mut timed = TimingJournal::new(
            Box::new(journal),
            tape.clone(),
            File::create(&sidecar).unwrap(),
            "barrier".into(),
        );
        let event = Event {
            request: Request::Stdout { io: 1 },
            completion: Completion::Handle(Ok(2)),
            fault: None,
        };
        timed.event(None, &event).unwrap();
        timed.barrier().unwrap();
        timed.event(None, &event).unwrap();
        drop(timed);
        let bytes = std::fs::read(tape).unwrap();
        let recovered = crate::journal::recover(&bytes).unwrap();
        assert_eq!(recovered.recording.events.len(), 1);
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(sidecar).unwrap()).unwrap();
        assert_eq!(value["samples"].as_array().unwrap().len(), 1);
        assert_eq!(value["samples"][0]["ordinal"], 0);
        assert_eq!(
            value["evidence"]["sha256"],
            hirsel_run_schema::hex(&Sha256::digest(bytes))
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    // Separate processes make the peak independent of the harness and other tests.
    #[test]
    fn recording_memory_probe() {
        let Ok(count) = std::env::var("HIRSEL_TIMING_MEMORY_REQUESTS") else {
            return;
        };
        let count: usize = count.parse().unwrap();
        let payload: usize = std::env::var("HIRSEL_TIMING_MEMORY_PAYLOAD")
            .unwrap()
            .parse()
            .unwrap();
        let root_scratch = tempfile::Builder::new()
            .prefix("timing-memory-")
            .tempdir()
            .unwrap();
        let root = root_scratch.path().to_path_buf();
        std::fs::create_dir_all(&root).unwrap();
        let tape = root.join("tape");
        let journal =
            TapeJournal(RecordingWriter::new(File::create(&tape).unwrap(), &header()).unwrap());
        let mut timed = TimingJournal::new(
            Box::new(journal),
            tape,
            File::create(root.join("timing")).unwrap(),
            "memory".into(),
        );
        let event = Event {
            request: Request::Print {
                stdout: 2,
                bytes: vec![42; payload],
            },
            completion: Completion::Unit(Ok(())),
            fault: None,
        };
        for _ in 0..count {
            timed.event(None, &event).unwrap();
        }
        timed
            .end(&RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            .unwrap();
        drop(timed);
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
        // SAFETY: getrusage initializes this live output on success.
        assert_eq!(
            unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) },
            0
        );
        let peak = unsafe { usage.assume_init() }.ru_maxrss;
        println!("TIMING_PEAK_RSS_KIB={peak}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recording_peak_memory_is_flat_in_request_count() {
        fn peak(count: usize, payload: usize) -> i64 {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "observation_timing::tests::recording_memory_probe",
                    "--nocapture",
                ])
                .env("HIRSEL_TIMING_MEMORY_REQUESTS", count.to_string())
                .env("HIRSEL_TIMING_MEMORY_PAYLOAD", payload.to_string())
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            String::from_utf8(output.stdout)
                .unwrap()
                .split_once("TIMING_PEAK_RSS_KIB=")
                .expect("memory probe ran")
                .1
                .lines()
                .next()
                .unwrap()
                .trim()
                .parse()
                .unwrap()
        }
        // Catch both payload readback at close and retained per-event timestamps.
        for (small, large, payload) in [(16, 256, 1 << 20), (1024, 2_097_152, 0)] {
            let before = peak(small, payload);
            let after = peak(large, payload);
            println!("payload={payload}: N={small} peak={before} KiB; N={large} peak={after} KiB");
            assert!(
                after <= before + 8 * 1024,
                "recording RSS grows with N: {small} requests={before} KiB, {large}={after} KiB (payload {payload})"
            );
        }
    }

    #[test]
    fn diagnostic_write_failure_does_not_fail_the_journal() {
        let root_scratch = tempfile::Builder::new()
            .prefix("timing-write-failure-")
            .tempdir()
            .unwrap();
        let root = root_scratch.path().to_path_buf();
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
