//! Go io.go LimitedReader/ReadAll request structure and io_test.go:252-285
//! short/data+EOF/data+error vectors (ebe2feef, BSD-3-Clause (Go); NOTICES).
//! File reads separate data and errors, so scripted tapes use successive events.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_rt::io::{Completion, Event, IoError, Request};
use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/fixtures/file-read-bounded-11/{name}.hir"))
}
fn checked(path: &Path) -> LoadedProgram {
    let mut loaded = load_program(path).unwrap();
    loaded.check_for_execution().unwrap_or_else(|error| {
        if let ToolError::Query {
            error: hirsel_types::QueryError::Diagnostics(diagnostics),
            sources,
            ..
        } = &error
        {
            for diagnostic in diagnostics {
                eprintln!("{}", diagnostic.render_human(sources));
            }
        }
        panic!("{error}")
    });
    loaded
}
fn ready(loaded: &mut LoadedProgram) -> (NativeInput, native::ReadyProgram) {
    let input = loaded.engine.export_native_input().unwrap();
    let validated = loaded.engine.import_native_input(input.clone()).unwrap();
    let ready = native::build(
        &validated,
        &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
    )
    .unwrap();
    (input, ready)
}
fn ok(outcome: RunOutcome) {
    assert!(
        matches!(
            outcome,
            RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
        ),
        "{outcome:?}"
    );
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn prints(events: &[Event]) -> Vec<u8> {
    events
        .iter()
        .flat_map(|event| match &event.request {
            Request::Print { bytes, .. } => bytes.clone(),
            _ => vec![],
        })
        .collect()
}
fn read_tape(path: &Path) -> hirsel_rt::launch::Recording {
    hirsel_rt::launch::parse_recording(&hirsel_rt::launch::read_recording_text(path).unwrap())
        .unwrap()
}
fn replay(path: &Path, tape: &Path, executables: &[PathBuf], expected: &[u8]) {
    let observation = replay_observations(path, tape, Some(20_000_000)).unwrap();
    assert_eq!(observation.output, expected);
    for exe in executables {
        let output = Command::new(exe)
            .arg("--hirsel-replay")
            .arg(tape)
            .output()
            .unwrap();
        assert!(output.status.success(), "{exe:?}: {output:?}");
        assert_eq!(output.stdout, expected);
        assert!(output.stderr.is_empty(), "{output:?}");
    }
}

#[test]
fn controls_max_u64_errors_and_owned_result() {
    with_compiler_stack(|| {
        let path = fixture("controls");
        ok(run_path_with_step_budget(
            &path,
            SimulationConfig::default(),
            Some(1_000_000),
            Verify::On,
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap());
        let mut loaded = checked(&path);
        let (input, program) = ready(&mut loaded);
        let result =
            native::run_heap(&program, &loaded.entry, &native::Sites::new(&input)).unwrap();
        assert_eq!(result.observation.exit_code, Some(0));
        assert!(
            result.counters.peak_live_heap_bytes < 65536,
            "{:?}",
            result.counters
        );
        eprintln!("max-u64 tiny-file allocation audit: {:?}", result.counters);
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
            ok(native::run_path_native_backend_with_validation(
                &path,
                HostLaunch::empty(),
                None,
                backend,
                native::Output::Streams {
                    stdout: &mut Vec::new(),
                    stderr: &mut Vec::new(),
                },
                Verify::On,
                native::Validation::EveryStage,
            )
            .unwrap()
            .unwrap());
        }
    });
}

#[test]
fn filesystem_boundaries_cursor_requests_and_cross_engine_replay() {
    with_compiler_stack(|| {
        let path = fixture("main");
        let mut loaded = checked(&path);
        let (_, program) = ready(&mut loaded);
        let retain = std::env::var_os("FILE_READ_BOUNDED_11_EVIDENCE");
        let base = retain
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_TARGET_TMPDIR")));
        let dir = base.join(format!("file-read-bounded-11-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("data")).unwrap();
        // The launcher exposes target scratch through a symlink; grants need its canonical host root.
        let dir = dir.canonicalize().unwrap();
        let target = native::TargetSpec::x86_64_unknown_linux_gnu();
        let tools = native::Toolchain::in_tree_with_runtime(
            &dir.join("runtime"),
            &[&target],
            native::RuntimeBuild::default(),
        )
        .unwrap();
        let mut executables = vec![];
        // One standalone build per backend, reused for all launch inputs/tapes.
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
            let out = dir.join(format!("{backend:?}"));
            std::fs::create_dir_all(&out).unwrap();
            let objects = native::emit_backend_objects(
                &program,
                backend,
                native::OptLevel::Speed,
                native::Partition::Whole,
                false,
                None,
            )
            .unwrap();
            executables.push(
                native::link_standalone_objects(&program, &loaded.entry, &objects, &tools, &out)
                    .unwrap(),
            );
        }
        let grants = serde_json::json!({"io":{"stdout":"stdout.bytes@1","dirs":[
            {"path":"data","host":dir.join("data"),"access":"read"}]},"args":[]})
        .to_string();
        let mut rows = vec![];
        for size in [0usize, 1, 3, 4, 5, 4095, 4096, 4097] {
            let data: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
            for limit in [size as u64, size.saturating_sub(1) as u64, size as u64 + 1] {
                rows.push((data.clone(), limit, 0usize));
            }
        }
        rows.extend([
            (b"\0\xff\x80x".to_vec(), 4, 0),
            (b"abc".to_vec(), 0, 0),
            (b"skip\0\xfftail".to_vec(), 2, 4),
            (b"skip\0\xff".to_vec(), 2, 4),
            (b"skip\0\xff".to_vec(), 3, 4),
            (b"tiny".to_vec(), u64::MAX, 0),
            (b"x".to_vec(), 0, 1),
            (b"abcd".to_vec(), 4, 0),
            (b"abcd".to_vec(), 5, 0), // scripted short/data+EOF/error seed below
        ]);
        let mut seed = None;
        let mut probe_seed = None;
        let mut tapes = vec![];
        for (row, (data, limit, start)) in rows.iter().enumerate() {
            let name = format!("case-{row}");
            std::fs::write(dir.join("data").join(&name), data).unwrap();
            let available = &data[*start..];
            let success = available.len() as u64 <= *limit;
            let consumed = if success {
                available.len()
            } else {
                *limit as usize + 1
            };
            let expected = format!(
                "{}tail {}\n",
                if success {
                    format!("ok {}\n", hex(available))
                } else {
                    "limit\n".into()
                },
                hex(&available[consumed..available.len().min(consumed + 2)])
            );
            let args: Vec<std::ffi::OsString> = [name, limit.to_string(), start.to_string()]
                .into_iter()
                .map(Into::into)
                .collect();
            for engine in 0..3 {
                let tape = dir.join(format!("row-{row}-engine-{engine}.tape"));
                if engine == 0 {
                    ok(run_path_on_host(
                        &path,
                        HostLaunch::Live {
                            config: LaunchConfig::Grants {
                                input: std::ffi::OsStr::new(&grants),
                                arguments: Some(&args),
                            },
                            record: Some(&tape),
                            revocations: None,
                        },
                        Some(20_000_000),
                        Verify::On,
                    )
                    .unwrap());
                } else {
                    let output = Command::new(&executables[engine - 1])
                        .arg("--hirsel-record")
                        .arg(&tape)
                        .arg("--hirsel-grants")
                        .arg(&grants)
                        .args(&args)
                        .output()
                        .unwrap();
                    assert!(
                        output.status.success(),
                        "row {row} engine {engine}: {output:?}"
                    );
                    assert_eq!(output.stdout, expected.as_bytes());
                    assert!(output.stderr.is_empty(), "{output:?}");
                }
                let record = read_tape(&tape);
                assert_eq!(prints(&record.events), expected.as_bytes());
                // The final read belongs to the caller. Validate each preceding
                // underlying request against remaining budget, including the probe.
                let reads: Vec<_> = record
                    .events
                    .iter()
                    .filter_map(|e| match (&e.request, &e.completion) {
                        (Request::FileRead { max, .. }, Completion::Bytes(Ok(bytes))) => {
                            Some((*max, bytes.len() as u64))
                        }
                        _ => None,
                    })
                    .collect();
                let mut remaining = *limit;
                for (index, (max, count)) in reads[..reads.len() - 1].iter().enumerate() {
                    assert_eq!(
                        *max,
                        if remaining == 0 {
                            1
                        } else {
                            remaining.min(4096)
                        }
                    );
                    assert!(*count <= *max);
                    if remaining > 0 {
                        remaining -= count;
                    }
                    if *count == 0 {
                        assert_eq!(index + 1, reads.len() - 1);
                    }
                }
                assert_eq!(reads.last().unwrap().0, 2);
                if engine == 0 {
                    if row + 2 == rows.len() {
                        probe_seed = Some(record.clone());
                    }
                    if row + 1 == rows.len() {
                        seed = Some(record);
                    }
                }
                tapes.push((tape, expected.clone()));
            }
        }
        // No host files or grants remain when any replay consumer runs.
        std::fs::rename(dir.join("data"), dir.join("retired-data")).unwrap();
        for (tape, expected) in &tapes {
            replay(&path, tape, &executables, expected.as_bytes());
        }
        scripted_short_reads_and_errors(
            &path,
            &dir,
            &executables,
            seed.unwrap(),
            probe_seed.unwrap(),
        );
        eprintln!(
            "{} boundary rows x 3 engines; {} recordings x 3 replay engines; 4 scripted tapes x 3 replay engines",
            rows.len(),
            tapes.len()
        );
        if retain.is_none() {
            std::fs::remove_dir_all(dir).unwrap();
        } else {
            eprintln!("retained evidence: {}", dir.display());
        }
    });
}

fn scripted_short_reads_and_errors(
    path: &Path,
    dir: &Path,
    executables: &[PathBuf],
    seed: hirsel_rt::launch::Recording,
    probe_seed: hirsel_rt::launch::Recording,
) {
    let index = seed
        .events
        .iter()
        .position(|e| matches!(e.request, Request::FileRead { .. }))
        .unwrap();
    // Every short success is followed by another request, including a final EOF.
    let mut short = seed.clone();
    let mut first = short.events[index].clone();
    first.completion = Completion::Bytes(Ok(b"a".to_vec()));
    let mut second = first.clone();
    if let Request::FileRead { max, .. } = &mut second.request {
        *max = 4;
    }
    second.completion = Completion::Bytes(Ok(b"bcd".to_vec()));
    short.events.splice(index..=index, [first, second]);
    let tape = dir.join("short.tape");
    std::fs::write(&tape, hirsel_rt::launch::render_recording(&short).unwrap()).unwrap();
    replay(path, &tape, executables, b"ok 61626364\ntail \n");
    // Preserve original file fault, discard accumulated bytes, and continue
    // from the last successful read. No new runtime fault-injection API.
    for (label, error, prefix, probe) in [
        ("fault-first", IoError::Fault(73), false, false),
        ("fault-after-data", IoError::Fault(91), true, false),
        ("fault-probe", IoError::Fault(129), true, true),
    ] {
        let mut record = if probe {
            probe_seed.clone()
        } else {
            seed.clone()
        };
        let end_reads = record
            .events
            .iter()
            .rposition(|e| matches!(e.request, Request::FileRead { .. }))
            .unwrap();
        let print_index = record
            .events
            .iter()
            .position(|e| matches!(e.request, Request::Print { .. }))
            .unwrap();
        let mut read = record.events[index].clone();
        let mut replacement = vec![];
        if prefix {
            read.completion = Completion::Bytes(Ok(if probe {
                b"abcd".to_vec()
            } else {
                b"ab".to_vec()
            }));
            replacement.push(read.clone());
            if let Request::FileRead { max, .. } = &mut read.request {
                *max = if probe { 1 } else { 3 };
            }
        }
        read.completion = Completion::Bytes(Err(error.clone()));
        read.fault = Some(hirsel_rt::io::Fault {
            error: error.clone(),
            partial: 0,
        });
        replacement.push(read);
        let text = match error {
            IoError::Fault(code) => format!("fault {code}\n"),
            _ => "denied\n".into(),
        };
        let mut print = record.events[print_index].clone();
        if let Request::Print { bytes, .. } = &mut print.request {
            *bytes = text.as_bytes().to_vec();
        }
        replacement.push(print);
        let mut tail = record.events[end_reads].clone();
        let tail_bytes: &[u8] = if probe {
            b""
        } else if prefix {
            b"cd"
        } else {
            b"ab"
        };
        tail.completion = Completion::Bytes(Ok(tail_bytes.to_vec()));
        replacement.push(tail);
        let mut tail_print = record.events[end_reads + 1].clone();
        let expected_tail = format!("tail {}\n", hex(tail_bytes));
        if let Request::Print { bytes, .. } = &mut tail_print.request {
            *bytes = expected_tail.as_bytes().to_vec();
        }
        replacement.push(tail_print);
        record.events.splice(index..=end_reads + 1, replacement);
        let tape = dir.join(format!("{label}.tape"));
        std::fs::write(&tape, hirsel_rt::launch::render_recording(&record).unwrap()).unwrap();
        replay(
            path,
            &tape,
            executables,
            format!("{text}{expected_tail}").as_bytes(),
        );
    }
}

#[test]
fn h_boundary_matrix() {
    with_compiler_stack(|| {
        let path = fixture("boundaries");
        let mut loaded = checked(&path);
        let (input, program) = ready(&mut loaded);
        let result = native::run_heap_in(
            &program,
            &loaded.entry,
            &native::Sites::new(&input),
            &native::HeapWorld {
                steps: Some(20_000_000),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(result.observation.exit_code, Some(0));
        assert_eq!(result.counters.live_heap_bytes, 0);
        eprintln!("H: 33 boundary rows, {:?}", result.counters);
    });
}

#[test]
fn file_must_belong_to_supplied_directory() {
    with_compiler_stack(|| {
        let source = r#"pub fn main(t: Test) -> Result[(), FileError] {
          let d = test.dir(t, [(path.of("input"), b"x")])
          let other = test.dir(t, [])
          let mut f = file.open(d, path.of("input"))?
          let _ = file.read_all_bounded(other, mut f, 4)
          Ok(())
        }"#;
        let mut loaded = load_program_source(&fixture("controls"), Some(source)).unwrap();
        let error = loaded
            .check_for_execution()
            .expect_err("Dir/File mismatch must reject");
        let ToolError::Query {
            error: hirsel_types::QueryError::Diagnostics(diagnostics),
            ..
        } = error
        else {
            panic!("expected relation diagnostic: {error:?}")
        };
        assert!(
            diagnostics.iter().any(|d| d.rule.as_str() == "PROV-1"),
            "{diagnostics:?}"
        );
    });
}
