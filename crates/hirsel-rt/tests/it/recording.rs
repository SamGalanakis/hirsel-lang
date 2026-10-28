//! Ordinary completion tapes survive process termination without claiming resumability.
use hirsel_rt::io::{Adapter, AdapterError, Completion, Event, Request, Tree};
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
fn scratch(name: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("tape-{name}-"))
        .tempdir_in(std::path::Path::new(env!("CARGO_TARGET_TMPDIR")))
        .unwrap()
}
#[test]
fn streamed_prefix_is_visible_before_delivery_and_complete_only_after_finish() {
    let root_scratch = scratch("stream");
    let root = root_scratch.path().to_path_buf();
    let tape = root.join("tape");
    let writer = RecordingWriter::new(std::fs::File::create(&tape).unwrap(), &header()).unwrap();
    let mut io = RealIo::grant(&root).unwrap();
    io.record_to(writer);
    let request = Request::Stdout { io: 1 };
    let completion = io.call(request.clone()).unwrap();
    let text = hirsel_rt::launch::read_recording_text(&tape).unwrap();
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
    let text = hirsel_rt::launch::read_recording_text(&tape).unwrap();
    assert_eq!(parse_recording(&text).unwrap().events.len(), 1);
    assert!(parse_recording(&text.replace("complete 1", "complete 2")).is_err());
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn invalid_capability_refusals_are_events_and_replay_consumes_them() {
    let root_scratch = scratch("refusal");
    let root = root_scratch.path().to_path_buf();
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
    let root_scratch = scratch("flush");
    let root = root_scratch.path().to_path_buf();
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
    let root_scratch = scratch("seeds");
    let root = root_scratch.path().to_path_buf();
    let tape = root.with_extension("tape");
    let output = probe("seeds", &root, &tape, false);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let record = parse_recording(&hirsel_rt::launch::read_recording_text(&tape).unwrap()).unwrap();
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
    let root_scratch = scratch("process");
    let root = root_scratch.path().to_path_buf();
    // Tape lives outside the grant: the initial snapshot must not include itself.
    let tape = root.with_extension("tape");
    let output = probe("abort", &root, &tape, false);
    assert_eq!(output.status.signal(), Some(libc::SIGABRT));
    let (record, complete) =
        parse_recording_prefix(&hirsel_rt::launch::read_recording_text(&tape).unwrap()).unwrap();
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
        let record =
            parse_recording(&hirsel_rt::launch::read_recording_text(&tape).unwrap()).unwrap();
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
    let root_scratch = scratch("map");
    let root = root_scratch.path().to_path_buf();
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
    let decoded = crate::support::tape::recording(&record);
    assert_eq!(decoded, record);
    let mut sim = SimIo::new(decoded.tree, decoded.events.clone());
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
        placement: Some(
            Placement {
                cores: Cores::Fixed(8),
                metrics: Some("/missing/metrics".into()),
                control: Some("/missing/control".into()),
                ..Default::default()
            }
            .into(),
        ),
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

fn settled_cpu_summary() -> hirsel_rt::io::CpuSummary {
    use hirsel_rt::io::{CpuPiece, CpuSummary};
    CpuSummary {
        version: 2,
        retirement: 0,
        ordinal: 1,
        descriptor: 0,
        value_bytes: 8,
        retained_count: 0,
        value: 42u64.to_le_bytes().to_vec(),
        pieces: vec![CpuPiece {
            credit: 0,
            fuel: 3,
            net: 16,
            envelope: vec![(2, 16)],
        }],
        level: 24,
        limit: 1024,
        fuel: 97,
        stop: None,
    }
}

#[test]
fn binary_frames_preserve_arbitrary_bytes_and_reject_every_torn_event() {
    use hirsel_rt::tape_binary::{Tape, Writer};
    let mut bytes = vec![];
    let events = vec![
        Event {
            request: Request::Stdout { io: 1 },
            completion: Completion::Handle(Ok(2)),
            fault: None,
            resources: None,
        },
        Event {
            request: Request::Print {
                stdout: 2,
                bytes: vec![0, 255, 128, b'\n'],
            },
            completion: Completion::Unit(Ok(())),
            fault: None,
            resources: None,
        },
        Event {
            request: Request::CpuSummary(settled_cpu_summary()),
            completion: Completion::Ready,
            fault: None,
            resources: None,
        },
    ];
    {
        let mut writer = Writer::new(&mut bytes, &header()).unwrap();
        for event in &events {
            writer.append(event).unwrap();
        }
        writer.finish().unwrap();
    }
    let (record, tape) = Tape::read(bytes.clone().into()).unwrap();
    assert_eq!(tape.events().unwrap(), events);
    assert!(tape.complete);
    assert_eq!(
        hirsel_rt::launch::parse_recording_bytes(&bytes)
            .unwrap()
            .events,
        events
    );
    let mut replay = SimIo::from_binary(record.tree, tape);
    for event in events {
        if let Request::CpuSummary(summary) = event.request {
            replay.cpu_summary(&summary).unwrap();
        } else {
            assert_eq!(replay.call(event.request).unwrap(), event.completion);
        }
    }
    replay.finish().unwrap();
    for end in 0..bytes.len() {
        assert!(hirsel_rt::launch::parse_recording_bytes(&bytes[..end]).is_err());
    }
    for at in 0..bytes.len() {
        let mut bad = bytes.clone();
        bad[at] ^= 1;
        assert!(hirsel_rt::launch::parse_recording_bytes(&bad).is_err());
    }
}

#[test]
fn bound_scripted_world_refuses_physical_capacity_even_with_consistent_arithmetic() {
    use hirsel_run_config::managed_cores::WorkerPhase;
    use hirsel_run_config::{CapacityAdmission, Cores, Placement, RecordedPlacement, RunConfig};
    use hirsel_run_schema::{IoGrants, LaunchKind, NetGrant};
    let mut config = RunConfig::empty([1; 32], "script@1/main".into(), LaunchKind::Main);
    config.io = Some(IoGrants {
        tasks: Some("tasks.cooperative@1".into()),
        net: Some(NetGrant {
            provider: "net.script@2".into(),
            rules: vec![],
            peers: Some(vec![]),
            listeners: vec![hirsel_run_schema::NetListener {
                address: "/fixture/script.sock".into(),
                port: 0,
                clients: vec![],
            }],
        }),
        ..Default::default()
    });
    let run = config.validated().unwrap();
    let mut recording = header();
    recording.run_identity = run.identity();
    recording.hash_seed = 0;
    recording.random_seed = 0;
    recording.test_seed = 0;
    recording.arguments.clear();
    recording.binding = Some(hirsel_rt::launch::RecordedRun {
        canonical: run.canonical().to_vec(),
        admitted_at: 0,
        revocation: None,
        instance: None,
        placement: Some(RecordedPlacement {
            schema: 1,
            request: Placement {
                cores: Cores::Fixed(4),
                ..Default::default()
            },
            admission: CapacityAdmission::Physical {
                requested: Cores::Fixed(4),
                resolved: 4,
                launched: 4,
                permitted: 4,
                effective: 4,
                worker_phase: WorkerPhase::Running,
            },
        }),
    });
    let encoded = hirsel_rt::launch::render_recording(&recording).unwrap();
    let parsed = hirsel_rt::launch::parse_recording(&encoded).unwrap();
    assert!(
        parsed
            .bound_run()
            .unwrap_err()
            .to_string()
            .contains("scripted world cannot claim physical capacity")
    );
}

#[test]
fn cpu_summary_replay_checks_all_fields_and_never_consumes_a_mismatch() {
    let summary = settled_cpu_summary();
    let mut live = RealIo::grant(scratch("cpu-summary").path()).unwrap();
    live.cpu_summary(&summary).unwrap();
    let mut record = header();
    record.events = live.tape().to_vec();
    let decoded = crate::support::tape::recording(&record);
    assert_eq!(decoded, record);
    let mut replay = SimIo::new(decoded.tree, decoded.events);
    for change in [
        (|s: &mut hirsel_rt::io::CpuSummary| s.version += 1) as fn(&mut hirsel_rt::io::CpuSummary),
        |s| s.retirement += 1,
        |s| s.ordinal += 1,
        |s| s.descriptor += 1,
        |s| s.value_bytes += 1,
        |s| s.retained_count += 1,
        |s| s.value[0] ^= 1,
        |s| s.value.pop().map(|_| ()).unwrap(),
        |s| s.pieces[0].fuel += 1,
        |s| s.pieces[0].net += 1,
        |s| s.pieces[0].envelope[0].1 += 1,
        |s| s.level += 1,
        |s| s.limit += 1,
        |s| s.fuel += 1,
        |s| s.stop = Some(hirsel_run_schema::BudgetKind::Fuel),
    ] {
        let mut changed = summary.clone();
        change(&mut changed);
        assert!(matches!(
            replay.cpu_summary(&changed),
            Err(AdapterError::ReplayMismatch(0) | AdapterError::InvalidCompletion)
        ));
    }
    replay.cpu_summary(&summary).unwrap();
    replay.finish().unwrap();
    assert_eq!(
        replay.cpu_summary(&summary),
        Err(AdapterError::MissingInput(1))
    );
}
