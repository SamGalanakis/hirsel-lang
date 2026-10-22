//! Ordinary native Free admission. The existing F0 framing carries the full
//! production run binding; pinned file ownership and replay stay on the same
//! launch path as cooperative runs. No test environment channel enables this.
use crate::launch::Recording;
use crate::stop::Stop;
use crate::tape_wire::{Wire, decode};
use crate::tasks::free::{record, replay};
use hirsel_run_schema::RunOutcome;
use std::sync::Arc;

// Staged until the full F1 qualification matrix is complete. Runtime unit
// tests exercise the ordinary configured launch path with production wiring.
pub(crate) const ENABLED: bool = cfg!(test);

pub(crate) enum Session {
    Recording {
        recorder: Arc<record::Recording>,
        namespace_coupled: bool,
    },
    Replay {
        driver: Arc<replay::Replay>,
        end: Option<String>,
        namespace_coupled: bool,
    },
}

fn binding(bytes: &[u8]) -> Result<(Recording, bool), String> {
    let bytes = bytes
        .strip_prefix(b"F1B1")
        .ok_or("unsupported Free admission binding")?;
    decode(bytes)
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
    ) -> Result<Option<Self>, String> {
        use hirsel_run_schema::RecordingLevel;
        let level = crate::host_launch::recording_level(admitted.run(), path.is_some())?;
        match (level, path) {
            (RecordingLevel::None, _) => Ok(None),
            (_, Some(path)) => {
                let (file, header, _) = crate::host_launch::header(io, admitted, path, outputs)?;
                let namespace_coupled = io.namespace_coupled();
                let mut binding = b"F1B1".to_vec();
                header.encode(&mut binding);
                namespace_coupled.encode(&mut binding);
                record::Recording::open(file, &binding, level == RecordingLevel::Durable)
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

    pub(crate) fn replay(bytes: &[u8]) -> Result<Self, String> {
        header(bytes)?;
        let tape = hirsel_free::tape::parse(bytes)?;
        let (_, namespace_coupled) = binding(&tape.binding)?;
        // Validate terminal bytes before any task or user callback can run.
        if let Some(end) = &tape.end {
            terminal(&end.outcome)?;
            validate_receipt(&tape, &end.outcome)?;
        }
        let driver = replay::Replay::new(bytes).map_err(|e| e.to_string())?;
        Ok(Self::Replay {
            driver,
            end: tape.end.map(|e| e.outcome),
            namespace_coupled,
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
                .stop_with(
                    |sweep| terminal_bytes("native-outcome-v2", outcome, sweep),
                    None,
                )
                .map_err(|e| format!("Free terminal recording: {e}"))?;
        }
        Ok(())
    }

    pub(crate) fn finish_replay(&self) -> Result<Option<Terminal>, String> {
        let Self::Replay { driver, end, .. } = self else {
            return Ok(None);
        };
        // This is the same strict consume-every-record/progress-at-cut check
        // used by the runtime replay proofs, before restoring the public outcome.
        driver.finish().map_err(|e| e.to_string())?;
        end.as_deref().map(terminal).transpose()
    }
}

pub(crate) enum Terminal {
    Stop(Stop),
    Outcome(RunOutcome),
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

fn terminal(text: &str) -> Result<Terminal, String> {
    let (kind, bytes) = terminal_payload(text)?;
    match kind {
        "native-stop-v1" => decode(&bytes).map(Terminal::Stop),
        "native-outcome-v1" => decode(&bytes).map(Terminal::Outcome),
        "native-stop-v2" => decode::<(Stop, Receipt)>(&bytes).map(|(stop, _)| Terminal::Stop(stop)),
        "native-outcome-v2" => {
            decode::<(RunOutcome, Receipt)>(&bytes).map(|(outcome, _)| Terminal::Outcome(outcome))
        }
        _ => Err("unsupported Free terminal encoding".into()),
    }
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
    use hirsel_free::{phase::Phase, record::Record};
    let (kind, bytes) = terminal_payload(text)?;
    if !kind.ends_with("-v2") {
        return Ok(());
    }
    let receipt = match kind {
        "native-stop-v2" => decode::<(Stop, Receipt)>(&bytes)?.1,
        "native-outcome-v2" => decode::<(RunOutcome, Receipt)>(&bytes)?.1,
        _ => return Err("unsupported terminal receipt".into()),
    };
    let mut expected: Receipt = Default::default();
    for (path, log) in &tape.logs {
        let records = hirsel_free::record::decode(log)?;
        for record in &records {
            if let Record::Prepared { op, id } = record {
                if records
                    .iter()
                    .any(|r| matches!(r, Record::Completion { op: done, .. } if done == op))
                {
                    continue;
                }
                let progress = tape.cut.get(path).ok_or("request absent from cut")?;
                if progress.op != *op {
                    return Err("unfinished request before another operation".into());
                }
                if progress.phase == Phase::Entered {
                    expected.0.push(*id);
                } else {
                    expected.1.push(*id);
                }
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
