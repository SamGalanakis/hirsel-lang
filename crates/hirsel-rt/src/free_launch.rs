//! Ordinary native Free admission. The existing F0 framing carries the full
//! production run binding; pinned file ownership and replay stay on the same
//! launch path as cooperative runs. No test environment channel enables this.
use crate::launch::Recording;
use crate::stop::Stop;
use crate::tape_wire::{Wire, decode};
use crate::tasks::free::{record, replay};
use hirsel_run_schema::RunOutcome;
use std::sync::Arc;

pub(crate) enum Session {
    Recording {
        recorder: Arc<record::Recording>,
        namespace_coupled: bool,
    },
    Replay {
        driver: Arc<replay::Replay>,
        end: Option<String>,
        namespace_coupled: bool,
        projection: std::sync::Mutex<Option<hirsel_free::projection::Projection>>,
    },
}

pub(crate) fn binding(bytes: &[u8]) -> Result<(Recording, bool), String> {
    if bytes.starts_with(b"F1B2") {
        return Err("Free recording runtime ABI version mismatch".into());
    }
    let bytes = bytes
        .strip_prefix(b"F1B4")
        .ok_or("unsupported Free admission binding")?;
    let mut input = crate::tape_wire::Decode(bytes);
    let abi = u32::decode(&mut input)?;
    if abi != crate::ABI_VERSION {
        return Err("Free recording runtime ABI version mismatch".into());
    }
    decode(input.0)
}

pub(crate) fn header(bytes: &[u8]) -> Result<Recording, String> {
    let tape = hirsel_free::tape::parse(bytes)?;
    let (header, _) = binding(&tape.binding)?;
    let run = header.bound_run().map_err(|e| e.to_string())?;
    if run.record().io.as_ref().and_then(|io| io.tasks.as_deref()) != Some("tasks.free@1") {
        return Err("Free tape binds another Tasks provider".into());
    }
    Ok(header)
}

impl Session {
    pub(crate) fn record(
        io: &mut crate::io_real::RealIo,
        admitted: &hirsel_run_config::AdmittedRun,
        path: Option<&std::path::Path>,
        outputs: &[(&str, &std::fs::File)],
        observe: bool,
    ) -> Result<Option<Self>, String> {
        use hirsel_run_schema::RecordingLevel;
        let level = crate::host_launch::recording_level(admitted.run(), path.is_some())?;
        match (level, path) {
            (RecordingLevel::None, _) => {
                if observe {
                    Ok(Some(Self::Recording {
                        recorder: record::Recording::observed().map_err(|e| e.to_string())?,
                        namespace_coupled: io.namespace_coupled(),
                    }))
                } else {
                    Ok(None)
                }
            }
            (_, Some(path)) => {
                let (file, header, _) = crate::host_launch::header(io, admitted, path, outputs)?;
                let namespace_coupled = io.namespace_coupled();
                let mut binding = b"F1B4".to_vec();
                crate::ABI_VERSION.encode(&mut binding);
                header.encode(&mut binding);
                namespace_coupled.encode(&mut binding);
                record::Recording::open(file, &binding, level == RecordingLevel::Durable, observe)
                    .map(|recorder| Self::Recording {
                        recorder,
                        namespace_coupled,
                    })
                    .map(Some)
                    .map_err(|e| format!("Free recording: {e}"))
            }
            _ => unreachable!("validated recording destination"),
        }
    }

    /// Admit a replay of `bytes`; only an `observe`d one keeps the events of the
    /// projection [`Session::finish_replay`] reports.
    pub(crate) fn replay(bytes: &[u8], observe: bool) -> Result<Self, String> {
        header(bytes)?;
        let tape = hirsel_free::tape::parse(bytes)?;
        let (_, namespace_coupled) = binding(&tape.binding)?;
        // Validate terminal bytes before any task or user callback can run.
        if let Some(end) = &tape.end {
            let terminal = terminal(&end.outcome)?;
            validate_root_outcome(&tape, &terminal)?;
            validate_receipt(&tape, &end.outcome)?;
        }
        let driver = replay::Replay::open(bytes, observe).map_err(|e| e.to_string())?;
        Ok(Self::Replay {
            driver,
            end: tape.end.map(|e| e.outcome),
            namespace_coupled,
            projection: Default::default(),
        })
    }

    /// # Safety
    /// Sole-root admission, before workers start or code runs.
    pub(crate) unsafe fn install(&self, ctx: *mut crate::TaskCtx) {
        unsafe {
            let namespace_coupled = match self {
                Self::Recording {
                    namespace_coupled, ..
                }
                | Self::Replay {
                    namespace_coupled, ..
                } => *namespace_coupled,
            };
            (*ctx)
                .run_ref()
                .free_namespace_coupled
                .store(namespace_coupled, std::sync::atomic::Ordering::Relaxed);
            match self {
                Self::Recording { recorder, .. } => recorder.root(ctx),
                Self::Replay { driver, .. } => driver.root(ctx),
            }
        }
    }

    pub(crate) fn replay_driver(&self) -> Option<&Arc<replay::Replay>> {
        match self {
            Self::Replay { driver, .. } => Some(driver),
            _ => None,
        }
    }

    pub(crate) fn finish_recording(&self, outcome: &RunOutcome) -> Result<(), String> {
        if let Self::Recording { recorder, .. } = self {
            recorder
                .recorder
                .end_with(
                    |sweep| terminal_bytes("native-outcome-v2", outcome, sweep),
                    None,
                )
                .map_err(|e| format!("Free terminal recording: {e}"))?;
        }
        Ok(())
    }

    pub(crate) fn report_recording(
        &self,
        report: &mut Option<crate::free_replay::Report>,
    ) -> Result<(), String> {
        if let (Self::Recording { recorder, .. }, Some(report)) = (self, report) {
            report.write(
                &recorder
                    .projection()
                    .ok_or("missing live Free projection")?,
            )?;
        }
        Ok(())
    }

    pub(crate) fn finish_replay(
        &self,
        report: &mut Option<crate::free_replay::Report>,
    ) -> Result<Option<Terminal>, String> {
        let Self::Replay {
            driver,
            end,
            projection: saved,
            ..
        } = self
        else {
            return Ok(None);
        };
        // This is the same strict consume-every-record/progress-at-cut check
        // used by the runtime replay proofs, before restoring the public outcome.
        let projection = driver.check().map_err(|e| e.to_string())?;
        if let Some(report) = report {
            report.write(
                projection
                    .as_ref()
                    .ok_or("Free projection report of an unobserved replay")?,
            )?;
        }
        *saved.lock().unwrap() = projection;
        end.as_deref().map(terminal).transpose()
    }

    pub(crate) fn canonical_trace(&self) -> Result<crate::tasks::ordered::events::Trace, String> {
        let projection = match self {
            Self::Recording { recorder, .. } => recorder.projection(),
            Self::Replay { projection, .. } => projection.lock().unwrap().clone(),
        }
        .ok_or("missing complete Free canonical evidence")?;
        crate::tasks::ordered::events::Trace::free(&projection)
    }
}

pub(crate) enum Terminal {
    Stop(Stop),
    Outcome(RunOutcome),
    Failed(RunOutcome),
}

fn terminal_payload(text: &str) -> Result<(&str, Vec<u8>), String> {
    let (kind, payload) = text
        .split_once('/')
        .ok_or("Free terminal has no typed cause")?;
    if payload.len() % 2 != 0 {
        return Err("Free terminal has odd hex length".into());
    }
    let bytes = payload
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |c: u8| match c {
                b'0'..=b'9' => Some(c - b'0'),
                b'a'..=b'f' => Some(c - b'a' + 10),
                _ => None,
            };
            Ok((digit(pair[0]).ok_or("invalid terminal hex")? << 4)
                | digit(pair[1]).ok_or("invalid terminal hex")?)
        })
        .collect::<Result<Vec<u8>, String>>()?;
    Ok((kind, bytes))
}

pub(crate) fn terminal(text: &str) -> Result<Terminal, String> {
    let (kind, bytes) = terminal_payload(text)?;
    match kind {
        "native-stop-v1" => decode(&bytes).map(Terminal::Stop),
        "native-outcome-v1" => decode(&bytes).map(Terminal::Outcome),
        "native-stop-v2" => decode::<(Stop, Receipt)>(&bytes).map(|(stop, _)| Terminal::Stop(stop)),
        "native-outcome-v2" => {
            decode::<(RunOutcome, Receipt)>(&bytes).map(|(outcome, _)| Terminal::Outcome(outcome))
        }
        "native-failure-v2" => {
            let (outcome, _) = decode::<(RunOutcome, Receipt)>(&bytes)?;
            if !matches!(outcome, RunOutcome::Failed { .. }) {
                return Err("internal Free failure needs a failed outcome".into());
            }
            Ok(Terminal::Failed(outcome))
        }
        _ => Err("unsupported Free terminal encoding".into()),
    }
}

/// Render the restored public outcome using the same observation fields as a
/// completed native entry. Cut parking may precede its final status return, so
/// the launcher cannot read a completed result or failure chain from that fiber.
pub(crate) fn replay_observation(
    program: &crate::program::Program,
    outcome: &RunOutcome,
) -> Result<String, String> {
    use std::fmt::Write;
    let mut text = String::new();
    match outcome {
        RunOutcome::Returned(_) => {
            writeln!(text, "status {}", crate::STATUS_OK).unwrap();
            // The observation reader projects only the public exit code. The
            // inactive Result payload contains no replay-visible value.
            let mut result = vec![0u8; program.result_size as usize];
            let code = outcome.exit_code() as u8;
            if program.return_kind == 1 || program.return_kind == 2 {
                result[program.success_offset as usize] = code;
            } else if code != 0 {
                result[0] = 1;
            }
            writeln!(text, "result {}", hirsel_run_schema::hex(&result)).unwrap();
            writeln!(text, "exit_code {code}").unwrap();
        }
        RunOutcome::Failed { primary, causes } => {
            let table = program
                .failure_table
                .ok_or("replayed failure has no bound program table")?;
            writeln!(text, "status {}", crate::STATUS_FAILURE).unwrap();
            for failure in std::iter::once(primary).chain(causes) {
                let record = table
                    .record(failure)
                    .ok_or("replayed failure differs from bound program table")?;
                writeln!(
                    text,
                    "failure {} {} {:016x} {:016x}",
                    record.kind, record.rule, record.site, record.operation_site
                )
                .unwrap();
            }
        }
        _ => {}
    }
    Ok(text)
}

// Called only on the owning task before its continuation is abandoned.
pub(crate) fn failure(ctx: *mut crate::TaskCtx) -> RunOutcome {
    let table = unsafe { (*ctx).run_ref().failure_table }.expect("admitted failure table");
    let chain = unsafe { (*ctx).failure.chain(&[]) };
    let mut records = chain.iter().map(|(record, _)| table.failure(record));
    RunOutcome::Failed {
        primary: records.next().expect("a primary failure"),
        causes: records.collect(),
    }
}

/// # Safety
/// The root owns its completed words; the owning role is held.
pub(crate) unsafe fn root_end(ctx: *mut crate::TaskCtx, status: u8, words: &[u128]) {
    if !record::on(ctx) && !unsafe { (*ctx).run_ref().free_replay } {
        return;
    }
    let outcome = if status == crate::STATUS_FAILURE {
        failure(ctx)
    } else {
        assert_eq!(status, crate::STATUS_OK);
        let (kind, offset) = unsafe { (*ctx).run_ref().free_return };
        let bytes = unsafe {
            std::slice::from_raw_parts(words.as_ptr().cast::<u8>(), std::mem::size_of_val(words))
        };
        let code = if kind == 1 {
            bytes[offset as usize]
        } else if bytes[..4] != [0; 4] {
            1
        } else if kind == 2 {
            bytes[offset as usize]
        } else {
            0
        };
        RunOutcome::Returned(hirsel_run_schema::Returned::from_exit_code(code))
    };
    let mut bytes = Vec::new();
    outcome.encode(&mut bytes);
    drop(outcome);
    unsafe { crate::stop::check(ctx, record::end_observed(ctx, bytes)) };
}

/// A normally completed root writes its canonical public outcome at Done.
/// Reconcile that independently recorded evidence before any replay can emit
/// effects. Stops and internal failures can cut before root completion and
/// therefore have no such equality requirement.
fn validate_root_outcome(
    tape: &hirsel_free::tape::Tape,
    terminal: &Terminal,
) -> Result<(), String> {
    use hirsel_free::{
        phase::Phase,
        record::{Decoder, Record},
    };
    let Terminal::Outcome(outcome @ (RunOutcome::Returned(_) | RunOutcome::Failed { .. })) =
        terminal
    else {
        return Ok(());
    };
    let differs = || "Free terminal outcome differs from root cut".to_string();
    let path = Vec::new();
    let progress = tape.cut.get(&path).ok_or_else(differs)?;
    if progress.op == 0 || progress.phase != Phase::Done {
        return Err(differs());
    }
    let mut observations = Vec::new();
    for record in Decoder::new(tape.logs.get(&path).ok_or_else(differs)?) {
        if let Record::Observation { op, bytes } = record?
            && op == progress.op
        {
            observations.push(bytes);
        }
    }
    let [bytes] = observations.as_slice() else {
        return Err(differs());
    };
    if decode::<RunOutcome>(bytes)? != *outcome {
        return Err(differs());
    }
    Ok(())
}

type Receipt = (Vec<u64>, Vec<u64>);

pub(crate) fn terminal_bytes<T: Wire>(
    kind: &str,
    cause: &T,
    sweep: &hirsel_free::claim::Sweep,
) -> String {
    let mut bytes = Vec::new();
    cause.encode(&mut bytes);
    let mut withdrawn = sweep.withdrawn.clone();
    let mut in_doubt = sweep.in_doubt.clone();
    withdrawn.sort_unstable();
    in_doubt.sort_unstable();
    (withdrawn, in_doubt).encode(&mut bytes);
    format!("{kind}/{}", hirsel_run_schema::hex(&bytes))
}

// The cut itself names every admitted request state. Check the persisted
// receipt independently before any replay code starts, including stopped waits.
fn validate_receipt(tape: &hirsel_free::tape::Tape, text: &str) -> Result<(), String> {
    use hirsel_free::{
        phase::Phase,
        record::{Decoder, Record},
    };
    let (kind, bytes) = terminal_payload(text)?;
    if !kind.ends_with("-v2") {
        return Ok(());
    }
    let receipt = match kind {
        "native-stop-v2" => decode::<(Stop, Receipt)>(&bytes)?.1,
        "native-outcome-v2" | "native-failure-v2" => decode::<(RunOutcome, Receipt)>(&bytes)?.1,
        _ => return Err("unsupported terminal receipt".into()),
    };
    let mut expected: Receipt = Default::default();
    for (path, log) in &tape.logs {
        let mut prepared = Vec::new();
        let mut completed = std::collections::BTreeSet::new();
        for record in Decoder::new(log) {
            match record? {
                Record::Prepared { op, id } => prepared.push((op, id)),
                Record::Completion { op, .. } => {
                    completed.insert(op);
                }
                _ => {}
            }
        }
        for (op, id) in prepared {
            if completed.contains(&op) {
                continue;
            }
            let progress = tape.cut.get(path).ok_or("request absent from cut")?;
            if progress.op != op {
                return Err("unfinished request before another operation".into());
            }
            if progress.phase == Phase::Entered {
                expected.0.push(id);
            } else {
                expected.1.push(id);
            }
        }
    }
    expected.0.sort_unstable();
    expected.1.sort_unstable();
    // Preparation precedes Entered publication. A stop can withdraw an owned
    // request whose Entered phase is absent from the cut; it can never claim
    // that request, so every in-doubt identity must be recoverable from the log.
    if receipt.0.windows(2).any(|w| w[0] >= w[1])
        || receipt.1.windows(2).any(|w| w[0] >= w[1])
        || receipt.0.iter().any(|id| receipt.1.contains(id))
        || expected.0.iter().any(|id| !receipt.0.contains(id))
        || expected.1 != receipt.1
    {
        return Err("terminal request receipt differs from cut".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod network_tests;

#[cfg(test)]
mod tls_tests;

#[cfg(test)]
mod namespace_tests;

#[cfg(test)]
mod refusal_tests;

#[cfg(test)]
mod lifetime_tests;

#[cfg(test)]
mod runtime50_tests;
