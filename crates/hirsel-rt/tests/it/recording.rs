//! Ordinary completion tapes survive process termination without claiming resumability.
use hirsel_rt::io::{Adapter, AdapterError, Completion, Request, Tree};
use hirsel_rt::io_real::RealIo;
use hirsel_rt::io_sim::SimIo;
use hirsel_rt::launch::{Recording, RecordingWriter, parse_recording, parse_recording_prefix};
use std::collections::BTreeMap;
use std::process::Command;

fn header() -> Recording {
    Recording {
        arguments: vec![],
        run_identity: [0; 32],
        hash_seed: 2,
        random_seed: 3,
        test_seed: 5,
        binding: None,
        tree: Tree::new(BTreeMap::new()).unwrap(),
        events: vec![],
    }
}
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("tape-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
#[test]
fn streamed_prefix_is_visible_before_delivery_and_complete_only_after_finish() {
    let root = scratch("stream");
    let tape = root.join("tape");
    let writer = RecordingWriter::new(std::fs::File::create(&tape).unwrap(), &header()).unwrap();
    let mut io = RealIo::grant(&root).unwrap();
    io.record_to(writer);
    let request = Request::Stdout { io: 1 };
    let completion = io.call(request.clone()).unwrap();
    let text = std::fs::read_to_string(&tape).unwrap();
    let (record, complete) = parse_recording_prefix(&text).unwrap();
    assert!(!complete);
    assert_eq!(record.events[0].request, request);
    assert_eq!(record.events[0].completion, completion);
    assert!(
        parse_recording(&text)
            .unwrap_err()
            .contains("unknown suffix")
    );
    assert!(parse_recording_prefix(&text[..text.len() - 1]).is_err());
    io.finish_recording(&hirsel_run_schema::RunOutcome::Returned(
        hirsel_run_schema::Returned::Ok,
    ))
    .unwrap();
    let text = std::fs::read_to_string(&tape).unwrap();
    assert_eq!(parse_recording(&text).unwrap().events.len(), 1);
    assert!(parse_recording(&text.replace("complete 1", "complete 2")).is_err());
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn invalid_capability_refusals_are_events_and_replay_consumes_them() {
    let root = scratch("refusal");
    let mut io = RealIo::grant(&root).unwrap();
    let requests = [
        Request::Stdout { io: 999 },
        Request::NarrowFile {
            directory: 1,
            file: 44,
        },
    ];
    for request in &requests {
        assert!(matches!(
            io.call(request.clone()),
            Err(AdapterError::InvalidHandle(_))
        ));
    }
    assert_eq!(io.tape().len(), 2);
    assert!(
        io.tape()
            .iter()
            .all(|e| matches!(e.completion, Completion::Refused(_)))
    );
    let mut sim = SimIo::new(header().tree, io.tape().to_vec());
    for request in requests {
        assert!(matches!(
            sim.call(request),
            Err(AdapterError::InvalidHandle(_))
        ));
    }
    sim.finish().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn a_flush_error_is_recorded_before_the_print_returns() {
    struct FlushError;
    impl std::io::Write for FlushError {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::from_raw_os_error(libc::EPIPE))
        }
    }
    let root = scratch("flush");
    let mut io = RealIo::grant_with_output(&root, FlushError).unwrap();
    let Completion::Handle(Ok(stdout)) = io.call(Request::Stdout { io: 1 }).unwrap() else {
        panic!()
    };
    assert!(matches!(
        io.call(Request::Print {
            stdout,
            bytes: b"tail".to_vec()
        }),
        Ok(Completion::Unit(Err(_)))
    ));
    let mut sim = SimIo::new(header().tree, io.tape().to_vec());
    for event in io.tape() {
        assert_eq!(sim.call(event.request.clone()).unwrap(), event.completion);
    }
    sim.finish().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
unsafe extern "C" fn entry(
    ctx: *mut hirsel_rt::TaskCtx,
    _: *mut u8,
    out: *mut u8,
    io: *const u64,
    _: *const u64,
) -> u8 {
    use hirsel_rt::io::{UnitResult, hirsel_rt_v2_io_stdout, hirsel_rt_v2_out_print};
    let mut stdout = hirsel_rt::io::GrantResult::default();
    unsafe { hirsel_rt_v2_io_stdout(ctx, std::ptr::null_mut(), &mut stdout, io) };
    let bytes = unsafe { hirsel_rt::text::allocate_bytes(ctx, b"confirmed") };
    let mut result = UnitResult::of(Ok(()));
    unsafe {
        hirsel_rt_v2_out_print(
            ctx,
            std::ptr::null_mut(),
            &mut result,
            &stdout.value,
            bytes.owner,
            bytes.start,
            bytes.length,
        )
    };
    unsafe {
        hirsel_rt::release::hirsel_rt_v2_release(
            ctx,
            bytes.owner,
            (*bytes.owner).index(),
            std::ptr::null(),
            0,
            0,
        )
    };
    match std::env::var("TAPE_PROBE").unwrap().as_str() {
        "abort" => std::process::abort(),
        "diverge" => {
            let mut extra = hirsel_rt::io::GrantResult::default();
            unsafe { hirsel_rt_v2_io_stdout(ctx, std::ptr::null_mut(), &mut extra, io) };
        }
        "close" => {
            unsafe { hirsel_rt::io::hirsel_rt_v2_io_close(ctx, 99, 0, 0) };
        }
        "seeds" => assert_eq!(unsafe { hirsel_rt::hirsel_rt_v2_hash_begin(ctx) }, 2),
        _ => {}
    }
    unsafe { out.cast::<u32>().write(0) };
    0
}
#[test]
fn process_probe() {
    let Ok(mode) = std::env::var("TAPE_PROBE") else {
        return;
    };
    if mode == "ebadf" {
        unsafe { libc::close(1) };
    }
    if mode == "epipe" {
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        unsafe {
            libc::close(fds[0]);
            libc::dup2(fds[1], 1);
            libc::close(fds[1]);
        }
    }
    let config = if mode == "seeds" {
        hirsel_rt::launch::Config {
            hash_seed: 2,
            random_seed: 3,
            test_seed: 5,
            run_identity: [7; 32],
        }
    } else {
        Default::default()
    };
    let result =
        unsafe { hirsel_rt::launch::hirsel_rt_v2_launch(entry, 4, std::ptr::null(), 0, &config) };
    std::process::exit(result);
}

#[test]
fn launcher_passes_only_the_hash_seed_to_allocation() {
    let root = scratch("seeds");
    let tape = root.with_extension("tape");
    let output = probe("seeds", &root, &tape, false);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let record = parse_recording(&std::fs::read_to_string(&tape).unwrap()).unwrap();
    assert_eq!(
        (
            record.hash_seed,
            record.random_seed,
            record.test_seed,
            record.run_identity
        ),
        (2, 3, 5, [7; 32])
    );
    let replay = probe("seeds", &root, &tape, true);
    assert_eq!(replay.status.code(), Some(0), "{replay:?}");
    std::fs::remove_file(tape).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
fn probe(
    mode: &str,
    root: &std::path::Path,
    tape: &std::path::Path,
    replay: bool,
) -> std::process::Output {
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "recording::process_probe", "--nocapture"])
        .env("TAPE_PROBE", mode)
        .env("HIRSEL_RT_ROOT", root)
        .env(
            if replay {
                "HIRSEL_RT_REPLAY"
            } else {
                "HIRSEL_RT_TAPE"
            },
            tape,
        )
        .output()
        .unwrap()
}
#[test]
fn abort_keeps_completed_events_and_replay_divergence_exits_three_with_stdout() {
    use std::os::unix::process::ExitStatusExt;
    let root = scratch("process");
    // Tape lives outside the grant: the initial snapshot must not include itself.
    let tape = root.with_extension("tape");
    let output = probe("abort", &root, &tape, false);
    assert_eq!(output.status.signal(), Some(libc::SIGABRT));
    let (record, complete) =
        parse_recording_prefix(&std::fs::read_to_string(&tape).unwrap()).unwrap();
    assert!(!complete);
    assert_eq!(record.events.len(), 2);
    assert_eq!(probe("ok", &root, &tape, false).status.code(), Some(0));
    for mode in ["diverge", "close"] {
        let output = probe(mode, &root, &tape, true);
        assert_eq!(output.status.code(), Some(3), "{output:?}");
        assert!(output.stdout.ends_with(b"confirmed"), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("MissingInput(2)"));
    }
    for mode in ["ebadf", "epipe"] {
        assert_eq!(probe(mode, &root, &tape, false).status.code(), Some(0));
        let record = parse_recording(&std::fs::read_to_string(&tape).unwrap()).unwrap();
        assert!(matches!(
            record.events[1].completion,
            Completion::Unit(Err(_))
        ));
        assert_eq!(record.events[1].fault.as_ref().unwrap().partial, 0);
    }
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_file(tape).unwrap();
}

#[test]
fn link_metadata_round_trips_without_flattening_or_admitting_a_collision() {
    let mut record = header();
    record.tree = Tree::new(BTreeMap::from([(
        "target/file".into(),
        b"initial".to_vec(),
    )]))
    .unwrap();
    record.tree.links.insert("alias".into(), "target".into());
    record
        .tree
        .links
        .insert("escape".into(), "../outside".into());
    let text = hirsel_rt::launch::render_recording(&record).unwrap();
    assert_eq!(parse_recording(&text).unwrap(), record);
    let malformed = text.replace("events\n", "link 746172676574 616c696173\nevents\n");
    assert!(
        parse_recording(&malformed)
            .unwrap_err()
            .contains("collision")
    );
    assert!(
        parse_recording(&text.replace(
            "link 616c696173 746172676574",
            "link 2e2e2f616c696173 746172676574"
        ))
        .is_err()
    );
}

/// A production recording binds its admitted run; replay trusts no header field the
/// record does not agree with.
#[test]
fn bound_runs_round_trip_and_refuse_a_header_that_disagrees() {
    use hirsel_rt::launch::{RecordedRun, render_recording};
    use hirsel_run_config::RunConfig;
    use hirsel_run_schema::{LaunchKind, RunRefusal};
    let mut record = RunConfig::empty([1; 32], "replayed@1/main".into(), LaunchKind::Main);
    record.inputs.random_seed = 3;
    record.args = Some(vec!["a".into()]);
    let run = record.validated().unwrap();
    let mut recording = header();
    recording.run_identity = run.identity();
    (
        recording.hash_seed,
        recording.random_seed,
        recording.test_seed,
    ) = (0, 3, 0);
    recording.arguments = vec!["a".into()];
    recording.binding = Some(RecordedRun {
        canonical: run.canonical().to_vec(),
        placement: None,
        admitted_at: -5,
        revocation: None,
        instance: None,
    });
    let text = render_recording(&recording).unwrap();
    let parsed = parse_recording(&text).unwrap();
    assert_eq!(parsed, recording);
    assert_eq!(parsed.bound_run().unwrap(), run);
    let disagrees = |change: fn(&mut Recording)| {
        let mut forged = recording.clone();
        change(&mut forged);
        forged.bound_run()
    };
    for change in [
        (|r: &mut Recording| r.random_seed = 4) as fn(&mut Recording),
        |r| r.arguments.clear(),
    ] {
        assert!(
            matches!(disagrees(change), Err(RunRefusal::Invalid { .. })),
            "{:?}",
            disagrees(change)
        );
    }
    assert!(matches!(
        disagrees(|r| r.run_identity = [0; 32]),
        Err(RunRefusal::Binding { .. })
    ));
    assert!(matches!(
        disagrees(|r| r.binding = None),
        Err(RunRefusal::Invalid { .. })
    ));
    // The record and its admission time come together.
    assert!(parse_recording(&text.replace("admitted_at -5\n", "")).is_err());
    assert!(parse_recording(&text.replace("admitted_at -5\n", "admitted_at -05\n")).is_err());
}

/// A host mapping lends the file's bytes in place. When another writer truncates the file,
/// reads past the new end see zeros instead of SIGBUS and the check reports Truncated,
/// sticky across a regrowth. The tape keeps each view's bytes and the truncation, so
/// replay over the original snapshot returns every completion, and the text tape
/// round-trips.
#[test]
fn mapping_views_and_an_outside_truncation_replay_from_the_tape() {
    use hirsel_rt::io::{IoError, MapView};
    let root = scratch("map");
    let original: Vec<u8> = (0..9000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(root.join("m"), &original).unwrap();
    let mut io = RealIo::grant(&root).unwrap();
    let Completion::Handle(Ok(directory)) = io
        .call(Request::Directory {
            io: 1,
            path: ".".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    let Completion::Handle(Ok(mapped)) = io
        .call(Request::Map {
            directory,
            path: "m".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    let view = Request::MapView { directory, mapped };
    let check = Request::MapCheck { directory, mapped };
    assert_eq!(
        io.call(view.clone()).unwrap(),
        Completion::View(MapView::of(original.clone()))
    );
    assert_eq!(io.call(check.clone()).unwrap(), Completion::Unit(Ok(())));
    std::fs::OpenOptions::new()
        .write(true)
        .open(root.join("m"))
        .unwrap()
        .set_len(10)
        .unwrap();
    let mut cut = original[..10].to_vec();
    cut.resize(original.len(), 0);
    assert_eq!(
        io.call(view.clone()).unwrap(),
        Completion::View(MapView::of(cut))
    );
    assert_eq!(
        io.call(check.clone()).unwrap(),
        Completion::Unit(Err(IoError::Truncated))
    );
    std::fs::write(root.join("m"), &original).unwrap();
    assert_eq!(
        io.call(check).unwrap(),
        Completion::Unit(Err(IoError::Truncated))
    );
    assert_eq!(
        io.call(Request::Close { handle: mapped }).unwrap(),
        Completion::Unit(Ok(()))
    );
    let mut record = header();
    record.tree = Tree::new(BTreeMap::from([("m".into(), original)])).unwrap();
    record.events = io.tape().to_vec();
    let text = hirsel_rt::launch::render_recording(&record).unwrap();
    assert_eq!(parse_recording(&text).unwrap(), record);
    let mut sim = SimIo::new(record.tree.clone(), record.events.clone());
    for event in &record.events {
        assert_eq!(sim.call(event.request.clone()).unwrap(), event.completion);
    }
    sim.finish().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn placement_header_is_diagnostic_validated_and_not_replayed() {
    use hirsel_rt::launch::{RecordedRun, render_recording};
    use hirsel_run_config::{Cores, Placement, RunConfig};
    use hirsel_run_schema::LaunchKind;
    let run = RunConfig::empty([1; 32], "placement@1/main".into(), LaunchKind::Main)
        .validated()
        .unwrap();
    let mut recording = header();
    recording.run_identity = run.identity();
    recording.hash_seed = 0;
    recording.random_seed = 0;
    recording.test_seed = 0;
    recording.arguments.clear();
    recording.binding = Some(RecordedRun {
        canonical: run.canonical().to_vec(),
        admitted_at: 0,
        revocation: None,
        instance: None,
        placement: Some(Placement {
            cores: Cores::Fixed(8),
            metrics: Some("/missing/metrics".into()),
            control: Some("/missing/control".into()),
            ..Default::default()
        }),
    });
    let text = render_recording(&recording).unwrap();
    let parsed = parse_recording(&text).unwrap();
    assert_eq!(parsed, recording);
    assert_eq!(parsed.bound_run().unwrap(), run);
    let line = text
        .lines()
        .find(|line| line.starts_with("placement "))
        .unwrap();
    assert!(parse_recording(&text.replace(line, &format!("{line}\n{line}"))).is_err());
    for json in [r#"{"cores":0}"#, r#"{"unknown":1}"#, r#"{"cores":1.0}"#] {
        let hex: String = json.bytes().map(|b| format!("{b:02x}")).collect();
        assert!(parse_recording(&text.replace(line, &format!("placement {hex}"))).is_err());
    }
}
