fn replay_width(width: usize) {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    fs::write(
        root.join("main.hir"),
        include_str!("replay.hir")
            .replace("WIDTH", &format!("{width}u32"))
            .replace("COUNT", &format!("{}u64", 2 * width)),
    )
    .unwrap();
    let manifest = root.join("hirsel.json");
    fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"cpu_wide_replay","version":"1"},
        "sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    let grants = root.join("grants.json");
    fs::write(&grants, serde_json::json!({"io":{"dirs":[],"tasks":"tasks.free@1","cpu":{"provider":"cpu.scoped@1",
        "max_workers":width,"max_domains":1,"max_physical_bytes":1073741824u64,"max_batch_bytes":8388608}}}).to_string()).unwrap();
    for backend in ["llvm", "cranelift"] {
        let executable = root.join(backend);
        checked(
            cpu_compiler()
                .args(["build", "--validate-stages", "--backend", backend])
                .arg(&manifest)
                .arg("-o")
                .arg(&executable)
                .output()
                .unwrap(),
        );
        let tape = root.join(format!("{backend}.tape"));
        checked(
            Command::new(&executable)
                .arg("--hirsel-grants")
                .arg(&grants)
                .arg("--hirsel-record")
                .arg(&tape)
                .output()
                .unwrap(),
        );
        let probe = root.join("replay-probe");
        let mut replay = Command::new(&executable);
        replay
            .arg("--hirsel-replay")
            .arg(&tape)
            .env("HIRSEL_RT_CPU_PROBE", &probe)
            .env("HIRSEL_RT_WORKERS", "1");
        use std::os::unix::process::CommandExt;
        unsafe {
            replay.pre_exec(|| {
                let mut allowed = std::mem::zeroed::<libc::cpu_set_t>();
                if libc::sched_getaffinity(0, std::mem::size_of_val(&allowed), &mut allowed) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                let cpu = (0..libc::CPU_SETSIZE as usize)
                    .find(|&cpu| libc::CPU_ISSET(cpu, &allowed))
                    .ok_or_else(|| std::io::Error::other("empty replay affinity"))?;
                let mut selected = std::mem::zeroed::<libc::cpu_set_t>();
                libc::CPU_SET(cpu, &mut selected);
                if libc::sched_setaffinity(0, std::mem::size_of_val(&selected), &selected) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        checked(replay.output().unwrap());
        assert!(
            fs::read_to_string(probe).unwrap().is_empty(),
            "replay cannot execute CPU pool workers"
        );
    }
}

fn write_cpu_recording(path: &std::path::Path, recording: &hirsel_rt::launch::Recording) {
    let header = hirsel_rt::launch::Recording {
        events: vec![],
        ..recording.clone()
    };
    let mut writer =
        hirsel_rt::launch::RecordingWriter::new(fs::File::create(path).unwrap(), &header).unwrap();
    for event in &recording.events {
        writer.append(event).unwrap();
    }
    writer.finish().unwrap();
}

fn ordered_replay_width(width: usize) {
    ordered_replay_width_mode(width, false);
}

fn ordered_allocation_memory_fixture(width: usize, backend: &str, root: &std::path::Path) {
    let root = root.join(format!("memory-{backend}"));
    fs::create_dir(&root).unwrap();
    let mut delay_result = 1u64;
    for i in 0..2_000_000 {
        delay_result = delay_result
            .wrapping_mul(6364136223846793005)
            .wrapping_add(i);
        delay_result ^= delay_result >> 29;
    }
    let source = format!(
        r#"
pub fn main(rt: Io) -> Result[(), GrantError] {{
  let tasks = io.tasks(rt)?
  let rows = core@1/cpu.map_scoped(tasks, {width}u32,
    fn() -> List[u64] uses {{}} {{ [1u64, 2u64] }},
    fn(take job: u64) -> unique List[u64] uses {{}} {{
      let mut out = [job]
      list.reserve(mut out, 128u64)
      if job == 1u64 {{
        list.reserve(mut out, 262144u64)
        let mut delay = job
        for i in 0u64..2000000u64 {{
          delay = int.wrapping_add(int.wrapping_mul(delay, 6364136223846793005u64), i)
          delay = delay ^ (delay >> 29u64)
        }}
        out[0] = (delay ^ {delay_result}u64) + job
      }}
      if job == 2u64 {{
        let mut pressure = [job]
        list.reserve(mut pressure, 262144u64)
        list.push(mut out, pressure[0] + 7u64)
      }} else {{
        list.push(mut out, job + 7u64)
      }}
      take out
    }})
  failure.check(rows == Ok([[1u64, 8u64], [2u64, 9u64]]), Bounds())
  Ok(())
}}
"#
    );
    let manifest = qual_manifest(&root, &source);
    let grants = root.join("grants.json");
    let mut authority = qual_grants(width);
    authority["io"]["tasks"] = serde_json::json!("tasks.cooperative@1");
    authority["placement"] = serde_json::json!({"cores":1,"reorder":"auto"});
    fs::write(&grants, authority.to_string()).unwrap();
    let executable = root.join(backend);
    checked(
        cpu_compiler()
            .args(["build", "--fuel", "--validate-stages", "--backend", backend])
            .arg(&manifest)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap(),
    );
    let tape = root.join("success.tape");
    checked(qual_native_output(
        Command::new(&executable)
            .arg("--hirsel-grants")
            .arg(&grants)
            .arg("--hirsel-record")
            .arg(&tape),
        &root,
    ));
    let recording = hirsel_rt::host_launch::read_recording(&tape).unwrap();
    let summary = recording
        .events
        .iter()
        .find_map(|event| match &event.request {
            hirsel_rt::io::Request::CpuSummary(summary) if summary.ordinal == 1 => Some(summary),
            _ => None,
        })
        .unwrap();
    let job_fuel: u64 = recording
        .events
        .iter()
        .filter_map(|event| match &event.request {
            hirsel_rt::io::Request::CpuSummary(summary) => summary.native.as_ref(),
            _ => None,
        })
        .flat_map(|summary| &summary.pieces)
        .map(|piece| piece.fuel)
        .sum();
    for fueled in [false, true] {
        if fueled {
            authority["budgets"] = serde_json::json!({
                "provider": "budgets.limits@2", "fuel": job_fuel + 65536
            });
        }
        fs::write(&grants, authority.to_string()).unwrap();
        for grant_only in [false, true] {
            eprintln!(
                "original 2 MiB/2 MiB W{width} {backend}, fueled={fueled}, grant_only={grant_only}"
            );
            ordered_allocation_memory_cut(&executable, &root, &grants, summary, grant_only);
        }
    }
}

fn ordered_allocation_memory_cut(
    executable: &std::path::Path,
    root: &std::path::Path,
    grants: &std::path::Path,
    summary: &hirsel_rt::io::CpuSummary,
    grant_only: bool,
) {
    let native = summary.native.as_ref().unwrap();
    assert_eq!(native.pieces.len(), 1);
    let piece = &native.pieces[0];
    let start = native.level.checked_add_signed(-piece.net).unwrap();
    let peak = piece.envelope.last().unwrap().1 as u64;
    // The older job retains a large result while the younger job allocates.
    // Half the younger peak fits above that settled prefix, so its cut is
    // inside job 1 rather than in publisher backing or the older job.
    let limit = start + peak / 2;
    let mut reference = None;
    for reorder in ["off", "auto"] {
        let mut authority: serde_json::Value =
            serde_json::from_slice(&fs::read(grants).unwrap()).unwrap();
        authority["placement"]["reorder"] = serde_json::json!(reorder);
        if grant_only {
            authority["io"]["cpu"]["max_batch_bytes"] = serde_json::json!(limit);
        } else {
            if authority["budgets"].is_null() {
                authority["budgets"] = serde_json::json!({"provider": "budgets.limits@2"});
            }
            authority["budgets"]["memory_bytes"] = serde_json::json!(limit);
        }
        let limited = root.join("memory-cut.json");
        fs::write(&limited, authority.to_string()).unwrap();
        let tape = root.join("memory-cut.tape");
        let events = root.join("memory-cut.events");
        let live = qual_native_output(
            Command::new(executable)
                .env("HIRSEL_RT_BOUNDED_STOP_TRACE", "1")
                .arg("--hirsel-grants")
                .arg(&limited)
                .arg("--hirsel-record")
                .arg(&tape)
                .arg("--hirsel-trace-events")
                .arg(&events),
            root,
        );
        assert_eq!(
            live.status.code(),
            Some(hirsel_run_schema::exit::BUDGET_EXCEEDED),
            "{live:?}"
        );
        let canonical = fs::read(&events).unwrap();
        let rows: Vec<serde_json::Value> = std::str::from_utf8(&canonical)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let terminal = rows
            .iter()
            .find(|row| row["outcome"]["kind"] == "stopped")
            .unwrap();
        assert_eq!(
            terminal["task"],
            serde_json::json!([1]),
            "limit {limit}, start {start}, peak {peak}, grant-only {grant_only}, {reorder}: {native:?}\n{}",
            std::str::from_utf8(&canonical).unwrap()
        );
        let mut recording = hirsel_rt::host_launch::read_recording(&tape).unwrap();
        assert!(
            recording.events.iter().any(|event| matches!(
                &event.request,
                hirsel_rt::io::Request::CpuSummary(summary) if summary.ordinal == 0
            )),
            "job 0 must retire before the selected job 1 cut"
        );
        let binding = recording.binding.as_mut().unwrap();
        binding.admitted_at = 0;
        binding.placement = None;
        let cut = (canonical.clone(), recording);
        if let Some(reference) = &reference {
            assert_eq!(
                &cut, reference,
                "lease and serial stop at the same allocation"
            );
        } else {
            reference = Some(cut);
        }
        let replayed = qual_native_output(
            Command::new(executable)
                .env("HIRSEL_RT_BOUNDED_STOP_TRACE", "1")
                .arg("--hirsel-replay")
                .arg(&tape)
                .arg("--hirsel-trace-events")
                .arg(&events)
                .env("HIRSEL_RT_WORKERS", "1"),
            root,
        );
        assert_eq!(replayed.status.code(), live.status.code(), "{replayed:?}");
        assert_eq!(replayed.stdout, live.stdout);
        assert_eq!(replayed.stderr, live.stderr);
        assert_eq!(fs::read(&events).unwrap(), canonical);
    }
}

fn ordered_replay_width_mode(width: usize, allocating: bool) {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let template = include_str!("physical.hir");
    let source = if allocating {
        allocating_cpu_source(template)
    } else {
        template.to_owned()
    };
    let source = source
        .replace("3000000u64", "2048u64")
        .replace("WIDTH64", &format!("{}u64", 2 * width))
        .replace("WIDTH", &format!("{width}u32"));
    let mut expected = vec![];
    for q in 1..=2 * width as u64 {
        let mut x = q;
        for i in 0..2048 {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(i);
            x ^= x >> 29;
        }
        expected.push(if allocating {
            format!("[{q}u64, {x}u64]")
        } else {
            format!("{x}u64")
        });
    }
    fs::write(
        root.join("main.hir"),
        source.replace("EXPECTED", &format!("[{}]", expected.join(","))),
    )
    .unwrap();
    let manifest = root.join("hirsel.json");
    fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"cpu_ordered_replay","version":"1"},
        "sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    let grants = root.join("grants.json");
    for backend in ["llvm", "cranelift"] {
        let executable = root.join(backend);
        checked(
            cpu_compiler()
                .args(["build", "--fuel", "--validate-stages", "--backend", backend])
                .arg(&manifest)
                .arg("-o")
                .arg(&executable)
                .output()
                .unwrap(),
        );
        let mut reference = None;
        for reorder in ["off", "auto"] {
            fs::write(&grants, serde_json::json!({"placement":{"cores":1,"reorder":reorder},
                "io":{"dirs":[],"tasks":"tasks.cooperative@1","cpu":{"provider":"cpu.scoped@1",
                "max_workers":width,"max_domains":1,"max_physical_bytes":1073741824u64,"max_batch_bytes":8388608}}}).to_string()).unwrap();
            let tape = root.join(format!("{backend}-{reorder}.tape"));
            let trace = root.join("live.events");
            let probe = root.join("ordered.probe");
            checked(
                Command::new(&executable)
                    .env("HIRSEL_RT_CPU_PROBE", &probe)
                    .arg("--hirsel-grants")
                    .arg(&grants)
                    .arg("--hirsel-record")
                    .arg(&tape)
                    .arg("--hirsel-trace-events")
                    .arg(&trace)
                    .output()
                    .unwrap(),
            );
            let jobs = cpu_job_stages(&root.join("ordered.probe.jobs"));
            assert_eq!(jobs.len(), 2 * width);
            let mut recording = hirsel_rt::host_launch::read_recording(&tape).unwrap();
            let summaries: Vec<_> = recording
                .events
                .iter()
                .filter_map(|event| {
                    if let hirsel_rt::io::Request::CpuSummary(summary) = &event.request {
                        Some(summary)
                    } else {
                        None
                    }
                })
                .collect();
            assert_eq!(
                summaries.len(),
                2 * width,
                "{backend} {reorder}: generated summaries"
            );
            if allocating && reorder == "auto" {
                ordered_allocation_memory_fixture(width, backend, root);
            }
            for (ordinal, summary) in summaries.iter().enumerate() {
                assert_eq!(summary.version, 2);
                assert_eq!(summary.ordinal, ordinal as u64);
                let encoded = if allocating {
                    let values: Vec<u64> = expected[ordinal]
                        .trim_matches(['[', ']'])
                        .split(", ")
                        .map(|value| value.trim_end_matches("u64").parse().unwrap())
                        .collect();
                    std::iter::once(values.len() as u64)
                        .chain(values)
                        .flat_map(u64::to_le_bytes)
                        .collect::<Vec<_>>()
                } else {
                    expected[ordinal]
                        .trim_end_matches("u64")
                        .parse::<u64>()
                        .unwrap()
                        .to_le_bytes()
                        .to_vec()
                };
                assert_eq!(summary.value, encoded);
            }
            if (width == 8 || allocating) && reorder == "auto" {
                // Every corrupted tape byte is refused by the real frame codec.
                let bytes = fs::read(&tape).unwrap();
                for at in 0..bytes.len() {
                    let mut bad = bytes.clone();
                    bad[at] ^= 1;
                    assert!(
                        hirsel_rt::launch::parse_recording_bytes(&bad).is_err(),
                        "{backend}: corrupted tape byte {at}"
                    );
                }
                // Recompute framing/checksums so a semantic mismatch must be
                // detected by serial execution, not merely by frame integrity.
                let index = recording
                    .events
                    .iter()
                    .position(|event| {
                        matches!(event.request, hirsel_rt::io::Request::CpuSummary(_))
                    })
                    .unwrap();
                let value_bytes = summaries[0].value.len();
                for byte in 0..value_bytes + if allocating { 16 } else { 15 } {
                    let mut changed = recording.clone();
                    let hirsel_rt::io::Request::CpuSummary(summary) =
                        &mut changed.events[index].request
                    else {
                        unreachable!()
                    };
                    match byte {
                        n if n < value_bytes => summary.value[n] ^= 1,
                        n if n == value_bytes => {
                            summary.value.pop();
                        }
                        n if n == value_bytes + 1 => summary.value.push(0),
                        n if n == value_bytes + 2 => summary.retirement += 1,
                        n if n == value_bytes + 3 => summary.ordinal += 1,
                        n if n == value_bytes + 4 => {
                            summary.stop = Some(hirsel_run_schema::BudgetKind::Fuel)
                        }
                        n if n == value_bytes + 5 => {
                            summary.native.as_mut().unwrap().descriptor += 1
                        }
                        n if n == value_bytes + 6 => {
                            summary.native.as_mut().unwrap().value_bytes += 1
                        }
                        n if n == value_bytes + 7 => {
                            summary.native.as_mut().unwrap().retained_count += 1
                        }
                        n if n == value_bytes + 8 => {
                            let fuel = &mut summary.native.as_mut().unwrap().pieces[0].fuel;
                            if allocating {
                                *fuel += 1;
                            } else {
                                *fuel ^= 1;
                            }
                        }
                        n if n == value_bytes + 9 => {
                            summary.native.as_mut().unwrap().pieces[0].net ^= 1
                        }
                        n if n == value_bytes + 10 => {
                            let envelope = &mut summary.native.as_mut().unwrap().pieces[0].envelope;
                            if let Some((_, peak)) = envelope.last_mut() {
                                *peak += 1;
                            } else {
                                envelope.push((0, 1));
                            }
                        }
                        n if n == value_bytes + 11 => summary.native.as_mut().unwrap().level ^= 1,
                        n if n == value_bytes + 12 => summary.native.as_mut().unwrap().limit ^= 1,
                        n if n == value_bytes + 13 => summary.native.as_mut().unwrap().fuel ^= 1,
                        n if n == value_bytes + 14 => {
                            summary
                                .native
                                .as_mut()
                                .unwrap()
                                .pieces
                                .push(hirsel_rt::io::CpuPiece {
                                    credit: 1,
                                    fuel: 0,
                                    net: 0,
                                    envelope: vec![],
                                })
                        }
                        _ => summary.native.as_mut().unwrap().pieces[0]
                            .envelope
                            .push((0, 1)),
                    }
                    let bad_tape = root.join("corrupted.tape");
                    write_cpu_recording(&bad_tape, &changed);
                    let malformed = allocating && byte == value_bytes + 15;
                    assert_eq!(
                        hirsel_rt::launch::parse_recording_bytes(&fs::read(&bad_tape).unwrap())
                            .is_err(),
                        malformed,
                        "{backend}: corruption {byte} framing"
                    );
                    let outcome = Command::new(&executable)
                        .arg("--hirsel-replay")
                        .arg(&bad_tape)
                        .env("HIRSEL_RT_WORKERS", "1")
                        .output()
                        .unwrap();
                    assert_eq!(
                        outcome.status.code(),
                        Some(hirsel_run_schema::exit::REFUSED),
                        "{backend}: semantic corruption {byte}: {}",
                        String::from_utf8_lossy(&outcome.stderr)
                    );
                    let interpreted = cpu_interpreter_cli(width, !allocating)
                        .arg("run")
                        .arg(&manifest)
                        .arg("--replay")
                        .arg(&bad_tape)
                        .output()
                        .unwrap();
                    if byte <= value_bytes + 4 || malformed {
                        assert_eq!(
                            interpreted.status.code(),
                            Some(hirsel_run_schema::exit::REFUSED),
                            "{backend}: interpreted portable corruption {byte}: {interpreted:?}"
                        );
                    } else {
                        // A newly checksummed native-only mutation is opaque to
                        // the interpreter. Native replay independently checks it.
                        assert!(
                            interpreted.status.success(),
                            "{backend}: interpreted native-only corruption {byte}: {interpreted:?}"
                        );
                    }
                }
            }
            let binding = recording.binding.as_mut().unwrap();
            binding.admitted_at = 0;
            binding.placement = None;
            let live = (fs::read(&trace).unwrap(), recording);
            if let Some(reference) = &reference {
                assert_eq!(&live, reference, "{backend} W{width} {reorder}");
            } else {
                reference = Some(live);
            }
            let replayed = root.join("replay.events");
            let probe = root.join("replay.probe");
            checked(
                Command::new(&executable)
                    .arg("--hirsel-replay")
                    .arg(&tape)
                    .arg("--hirsel-trace-events")
                    .arg(&replayed)
                    .env("HIRSEL_RT_WORKERS", "1")
                    .env("HIRSEL_RT_CPU_PROBE", &probe)
                    .output()
                    .unwrap(),
            );
            assert_eq!(fs::read(&replayed).unwrap(), reference.as_ref().unwrap().0);
            assert_eq!(cpu_job_stages(&root.join("replay.probe.jobs")), jobs);
            assert!(fs::read(&probe).unwrap().is_empty());
            if width == 8 && reorder == "auto" {
                for budget in [1, 1000] {
                    let mut limited: serde_json::Value =
                        serde_json::from_slice(&fs::read(&grants).unwrap()).unwrap();
                    limited["budgets"] = serde_json::json!({
                        "provider": "budgets.limits@2", "fuel": budget
                    });
                    let limited_grants = root.join("limited.json");
                    fs::write(&limited_grants, limited.to_string()).unwrap();
                    let stopped_tape = root.join("stopped.tape");
                    let stopped = Command::new(&executable)
                        .arg("--hirsel-grants")
                        .arg(&limited_grants)
                        .arg("--hirsel-record")
                        .arg(&stopped_tape)
                        .output()
                        .unwrap();
                    assert_eq!(
                        stopped.status.code(),
                        Some(hirsel_run_schema::exit::BUDGET_EXCEEDED),
                        "{backend}: {stopped:?}"
                    );
                    let replayed = Command::new(&executable)
                        .arg("--hirsel-replay")
                        .arg(&stopped_tape)
                        .output()
                        .unwrap();
                    assert_eq!(
                        replayed.status.code(),
                        stopped.status.code(),
                        "{backend}: {replayed:?}"
                    );
                    let interpreted = cpu_interpreter_cli(width, !allocating)
                        .arg("run")
                        .arg(&manifest)
                        .arg("--replay")
                        .arg(&stopped_tape)
                        .output()
                        .unwrap();
                    assert_eq!(
                        interpreted.status.code(),
                        Some(hirsel_run_schema::exit::REFUSED),
                        "{backend}: native CPU fuel cuts are engine-specific: {interpreted:?}"
                    );
                    assert!(
                        String::from_utf8_lossy(&interpreted.stderr).contains("CPU"),
                        "{backend}: {interpreted:?}"
                    );
                }
            }
        }
        eprintln!(
            "D02 {backend} W{width}: serial off == physical auto == one-permit replay, complete canonical trace and projected tape"
        );
    }
}

fn ordered_summary_fields() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    fs::write(
        root.join("main.hir"),
        r#"
pub type AppError = Grant(GrantError) | Pool(CpuPoolError)
pub type CpuRow { small: u8, value: u64 }
pub fn main(rt: Io) -> Result[(), AppError] {
  let tasks = io.tasks(rt) ? Grant
  let rows = core@1/cpu.map_scoped(tasks, 8u32,
    fn() -> List[u64] uses {} { [1u64, 2u64] },
    fn(take job: u64) -> (u8, u64) uses {} { (7u8, job) }) ? Pool
  failure.check(rows == [(7u8, 1u64), (7u8, 2u64)], Bounds())
  let arms = core@1/cpu.map_scoped(tasks, 8u32,
    fn() -> List[u64] uses {} { [1u64, 2u64] },
    fn(take job: u64) -> Result[u64, u8] uses {} {
      if job == 1u64 { Ok(job) } else { Error(3u8) }
    }) ? Pool
  failure.check(arms == [Ok(1u64), Error(3u8)], Bounds())
  let records = core@1/cpu.map_scoped(tasks, 8u32,
    fn() -> List[u64] uses {} { [1u64, 2u64] },
    fn(take job: u64) -> CpuRow uses {} { CpuRow(small: 7u8, value: job) }) ? Pool
  failure.check(records == [CpuRow(small: 7u8, value: 1u64), CpuRow(small: 7u8, value: 2u64)], Bounds())
  Ok(())
}
"#,
    )
    .unwrap();
    let manifest = root.join("hirsel.json");
    fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"cpu_summary_fields","version":"1"},
        "sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    let grants = root.join("grants.json");
    for backend in ["llvm", "cranelift"] {
        let executable = root.join(backend);
        checked(
            cpu_compiler()
                .args(["build", "--fuel", "--validate-stages", "--backend", backend])
                .arg(&manifest)
                .arg("-o")
                .arg(&executable)
                .output()
                .unwrap(),
        );
        let mut reference = None;
        for reorder in ["off", "auto"] {
            fs::write(&grants, serde_json::json!({"placement":{"cores":1,"reorder":reorder},
                "io":{"dirs":[],"tasks":"tasks.cooperative@1","cpu":{"provider":"cpu.scoped@1",
                "max_workers":8,"max_domains":1,"max_physical_bytes":1073741824u64,"max_batch_bytes":8388608}}}).to_string()).unwrap();
            let tape = root.join(format!("{backend}-{reorder}.tape"));
            checked(
                Command::new(&executable)
                    .arg("--hirsel-grants")
                    .arg(&grants)
                    .arg("--hirsel-record")
                    .arg(&tape)
                    .output()
                    .unwrap(),
            );
            let recording = hirsel_rt::host_launch::read_recording(&tape).unwrap();
            let summaries: Vec<_> = recording
                .events
                .iter()
                .filter_map(|event| {
                    if let hirsel_rt::io::Request::CpuSummary(summary) = &event.request {
                        Some(summary.value.clone())
                    } else {
                        None
                    }
                })
                .collect();
            let expected = vec![
                [vec![7], 1u64.to_le_bytes().to_vec()].concat(),
                [vec![7], 2u64.to_le_bytes().to_vec()].concat(),
                [0u32.to_le_bytes().to_vec(), 1u64.to_le_bytes().to_vec()].concat(),
                [1u32.to_le_bytes().to_vec(), vec![3]].concat(),
                [vec![7], 1u64.to_le_bytes().to_vec()].concat(),
                [vec![7], 2u64.to_le_bytes().to_vec()].concat(),
            ];
            assert_eq!(
                summaries, expected,
                "{backend} {reorder}: padding and inactive arms"
            );
            checked(
                cpu_interpreter_cli(8, true)
                    .arg("run")
                    .arg(&manifest)
                    .arg("--replay")
                    .arg(&tape)
                    .output()
                    .unwrap(),
            );
            if reorder == "auto" {
                let portable = root.join(format!("{backend}-portable.tape"));
                checked(
                    cpu_interpreter_cli(8, true)
                        .arg("run")
                        .arg(&manifest)
                        .arg("--grants")
                        .arg(&grants)
                        .arg("--record")
                        .arg(&portable)
                        .output()
                        .unwrap(),
                );
                checked(
                    Command::new(&executable)
                        .arg("--hirsel-replay")
                        .arg(&portable)
                        .output()
                        .unwrap(),
                );
                let mut mutations = 0;
                for (event_index, event) in recording.events.iter().enumerate() {
                    let hirsel_rt::io::Request::CpuSummary(summary) = &event.request else {
                        continue;
                    };
                    for byte in 0..summary.value.len() {
                        let mut changed = recording.clone();
                        let hirsel_rt::io::Request::CpuSummary(summary) =
                            &mut changed.events[event_index].request
                        else {
                            unreachable!()
                        };
                        summary.value[byte] ^= 1;
                        let corrupted = root.join("fields-corrupted.tape");
                        write_cpu_recording(&corrupted, &changed);
                        let native = Command::new(&executable)
                            .arg("--hirsel-replay")
                            .arg(&corrupted)
                            .output()
                            .unwrap();
                        let interpreted = cpu_interpreter_cli(8, true)
                            .arg("run")
                            .arg(&manifest)
                            .arg("--replay")
                            .arg(&corrupted)
                            .output()
                            .unwrap();
                        for (engine, output) in [("native", native), ("TIR", interpreted)] {
                            assert_eq!(
                                output.status.code(),
                                Some(hirsel_run_schema::exit::REFUSED),
                                "{backend} {engine} fields event {event_index} byte {byte}: {output:?}"
                            );
                        }
                        mutations += 1;
                    }
                }
                assert_eq!(
                    mutations, 53,
                    "all tuple, active-arm and record semantic bytes"
                );
            }
            if let Some(reference) = &reference {
                assert_eq!(&summaries, reference);
            } else {
                reference = Some(summaries);
            }
            checked(
                Command::new(&executable)
                    .arg("--hirsel-replay")
                    .arg(&tape)
                    .env("HIRSEL_RT_WORKERS", "1")
                    .output()
                    .unwrap(),
            );
        }
    }
}

fn ordered_allocating_summary_fields() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    fs::write(
        root.join("main.hir"),
        r#"
pub type AppError = Grant(GrantError) | Pool(CpuPoolError)
pub fn main(rt: Io) -> Result[(), AppError] {
  let tasks = io.tasks(rt) ? Grant
  let rows = core@1/cpu.map_scoped(tasks, 8u32,
    fn() -> List[u64] uses {} { [1u64, 2u64] },
    fn(take job: u64) -> (u8, unique List[u64]) uses {} { (7u8, [job]) }) ? Pool
  failure.check(rows == [(7u8, [1u64]), (7u8, [2u64])], Bounds())
  let arms = core@1/cpu.map_scoped(tasks, 8u32,
    fn() -> List[u64] uses {} { [1u64, 2u64] },
    fn(take job: u64) -> Result[unique List[u64], u8] uses {} {
      if job == 1u64 { Ok([job]) } else { Error(3u8) }
    }) ? Pool
  failure.check(arms == [Ok([1u64]), Error(3u8)], Bounds())
  Ok(())
}
"#,
    )
    .unwrap();
    let manifest = root.join("hirsel.json");
    fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"cpu_summary_fields","version":"1"},
        "sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    let grants = root.join("grants.json");
    for backend in ["llvm", "cranelift"] {
        let executable = root.join(backend);
        checked(
            Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .args(["build", "--fuel", "--validate-stages", "--backend", backend])
                .arg(&manifest)
                .arg("-o")
                .arg(&executable)
                .output()
                .unwrap(),
        );
        let mut reference = None;
        for reorder in ["off", "auto"] {
            fs::write(&grants, serde_json::json!({"placement":{"cores":1,"reorder":reorder},
                "io":{"dirs":[],"tasks":"tasks.cooperative@1","cpu":{"provider":"cpu.scoped@1",
                "max_workers":8,"max_domains":1,"max_physical_bytes":1073741824u64,"max_batch_bytes":8388608}}}).to_string()).unwrap();
            let tape = root.join(format!("{backend}-{reorder}.tape"));
            checked(
                Command::new(&executable)
                    .arg("--hirsel-grants")
                    .arg(&grants)
                    .arg("--hirsel-record")
                    .arg(&tape)
                    .output()
                    .unwrap(),
            );
            let recording = hirsel_rt::host_launch::read_recording(&tape).unwrap();
            let summaries: Vec<_> = recording
                .events
                .iter()
                .filter_map(|event| {
                    if let hirsel_rt::io::Request::CpuSummary(summary) = &event.request {
                        Some(summary.value.clone())
                    } else {
                        None
                    }
                })
                .collect();
            let expected = vec![
                [
                    vec![7],
                    1u64.to_le_bytes().to_vec(),
                    1u64.to_le_bytes().to_vec(),
                ]
                .concat(),
                [
                    vec![7],
                    1u64.to_le_bytes().to_vec(),
                    2u64.to_le_bytes().to_vec(),
                ]
                .concat(),
                [
                    0u32.to_le_bytes().to_vec(),
                    1u64.to_le_bytes().to_vec(),
                    1u64.to_le_bytes().to_vec(),
                ]
                .concat(),
                [1u32.to_le_bytes().to_vec(), vec![3]].concat(),
            ];
            assert_eq!(
                summaries, expected,
                "{backend} {reorder}: padding and inactive arms"
            );
            if let Some(reference) = &reference {
                assert_eq!(&summaries, reference);
            } else {
                reference = Some(summaries);
            }
            checked(
                Command::new(&executable)
                    .arg("--hirsel-replay")
                    .arg(&tape)
                    .env("HIRSEL_RT_WORKERS", "1")
                    .output()
                    .unwrap(),
            );
        }
    }
}
