//! Readable ownership diagnostics and their independently bound JSON sidecar.
//! Called only after the selected engine has quiesced; program streams are
//! separate from the diagnostic writer supplied here.
use hirsel_run_schema::ownership_trace::Trace;
use std::io::Write;
use std::path::Path;

/// Engine events are bounded so a looping program cannot grow the trace
/// without limit; an exhausted bound refuses the trace, never truncates it.
const EVENT_LIMIT: usize = 1 << 20;

/// `hirsel run --trace-ownership`: the reference interpreter runs the admitted
/// source as an ordinary run would, and the run's own outcome decides the exit
/// code. A trace that cannot be completed is a tool failure, reported after
/// that outcome, so a run is never mistaken for a fully traced one.
pub(super) fn run(
    path: &Path,
    launch: hirsel_driver::HostLaunch<'_>,
    max_steps: Option<u64>,
    verify: hirsel_engine::Verify,
    sidecar: &Path,
    json: bool,
) -> u8 {
    let run = match hirsel_driver::ownership_trace::run_resources_on_host(
        path,
        launch,
        max_steps,
        verify,
        false,
        EVENT_LIMIT,
    ) {
        Ok(run) => run,
        Err(error) => return super::run_result_for_path(Err(error), path, json),
    };
    let code = super::run_result_for_path(Ok(run.outcome), path, json);
    let written = run
        .trace
        .map_err(|error| error.to_string())
        .and_then(|trace| {
            write(
                &trace,
                run.program,
                EVENT_LIMIT,
                sidecar,
                &mut std::io::stderr().lock(),
            )
            .map_err(|error| error.to_string())
        });
    match written {
        Ok(()) => code,
        Err(error) => {
            eprintln!("hirsel: ownership trace: {error}");
            hirsel_engine::RunOutcome::Tooling(error).exit_code() as u8
        }
    }
}

pub(super) fn write(
    trace: &Trace,
    program: [u8; 32],
    event_limit: usize,
    destination: &Path,
    diagnostic: &mut dyn Write,
) -> Result<(), Box<dyn std::error::Error>> {
    // Validate the engine's complete stream before publishing any output. In
    // particular a truncated stream must never display a zero-live verdict.
    let analysis = trace.analyze(program, event_limit)?;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let mut file = std::io::BufWriter::new(file);
    serde_json::to_writer(&mut file, trace)?;
    file.flush()?;
    for event in &trace.events {
        writeln!(diagnostic, "{event}")?;
    }
    for live in &analysis.live {
        writeln!(
            diagnostic,
            "live {} ({}) held by {}",
            live.token,
            live.value_type,
            live.owner.as_ref().expect("live lineage has an owner")
        )?;
        for event in &live.events {
            writeln!(diagnostic, "  {event}")?;
        }
    }
    writeln!(
        diagnostic,
        "ownership: {} created, {} dropped, {} live; {}",
        analysis.created,
        analysis.dropped,
        analysis.live.len(),
        destination.display()
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_run_schema::Site;
    use hirsel_run_schema::ownership_trace::{Context, Owner, Storage, TaskPath, Tracker};

    fn leak() -> Trace {
        let mut tracker = Tracker::new(8);
        let context = Context {
            task: TaskPath::default(),
            operation: 4,
            site: Site {
                definition: "demo::main".into(),
                file: "leak.hir".into(),
                line: 7,
                column: 3,
            },
        };
        let owner = |slot| Owner {
            task: Some(TaskPath::default()),
            storage: Storage::Slot { frame: 0, slot },
            path: vec![],
        };
        tracker.create(&context, owner(0), "Credit".into()).unwrap();
        tracker.move_tree(&context, &owner(0), &owner(1)).unwrap();
        tracker.finish([49; 32], true).unwrap()
    }

    #[test]
    fn ownership_output_preserves_machine_lineage_and_renders_the_exact_live_move_chain() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("trace.json");
        let trace = leak();
        let mut human = vec![];
        write(&trace, [49; 32], 8, &destination, &mut human).unwrap();
        let decoded: Trace = serde_json::from_slice(&std::fs::read(&destination).unwrap()).unwrap();
        assert_eq!(decoded, trace);
        let human = String::from_utf8(human).unwrap();
        assert!(
            human.contains("root 4.0 demo::main:7:3 create root#0 (Credit)"),
            "{human}"
        );
        assert!(
            human.contains(
                "move root#0 root/Slot { frame: 0, slot: 0 } -> root/Slot { frame: 0, slot: 1 }"
            ),
            "{human}"
        );
        assert!(
            human.contains("live root#0 (Credit) held by root/Slot { frame: 0, slot: 1 }"),
            "{human}"
        );
        assert!(human.contains("1 created, 0 dropped, 1 live"), "{human}");
    }

    #[test]
    fn ownership_output_refuses_incomplete_or_misbound_streams_before_publication() {
        let dir = tempfile::tempdir().unwrap();
        for (mut trace, program) in [(leak(), [50; 32]), (leak(), [49; 32])] {
            if program == [49; 32] {
                trace.complete = false;
            }
            let destination = dir.path().join("trace.json");
            let mut human = vec![];
            assert!(write(&trace, program, 8, &destination, &mut human).is_err());
            assert!(!destination.exists());
            assert!(human.is_empty());
        }
    }

    #[test]
    fn ownership_output_never_replaces_an_existing_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("trace.json");
        std::fs::write(&destination, b"existing evidence").unwrap();
        let mut human = vec![];
        assert!(write(&leak(), [49; 32], 8, &destination, &mut human).is_err());
        assert_eq!(std::fs::read(destination).unwrap(), b"existing evidence");
        assert!(human.is_empty());
    }
}
