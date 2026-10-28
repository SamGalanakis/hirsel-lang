fn cpu_interpreter_receipt_cuts(width: usize) {
    use hirsel_rt::io::{Event, Request};
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let jobs = (1..=2 * width)
        .map(|n| format!("{n}u64"))
        .collect::<Vec<_>>()
        .join(",");
    let expected = (1..=2 * width)
        .map(|n| format!("{}u64", n + 7))
        .collect::<Vec<_>>()
        .join(",");
    let source = include_str!("interpreter.hir")
        .replace("WIDTH", &format!("{width}u32"))
        .replace("JOBS", &format!("[{jobs}]"))
        .replace("EXPECTED", &format!("[{expected}]"))
        .replace("  let _ = out.print(io.stdout(rt)?, \"cpu-scalar-ok\\n\")\n", "")
        .replace("    failure.check(rows ==", "    if repeat == 0u64 { match rows { Ok(values) => failure.check(values == EXPECTED_ROWS, Bounds()), Error(CapacityLost()) => (), Error(Canceled(_)) => (), _ => failure.check(false, Arithmetic()) } } else { failure.check(rows ==")
        .replace("), Bounds())\n  }", "), Bounds()) }\n  }")
        .replace("EXPECTED_ROWS", &format!("[{expected}]"));
    fs::write(root.join("main.hir"), source).unwrap();
    let manifest = root.join("hirsel.json");
    fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"cpu_receipt_cuts","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    let grants = root.join("grants.json");
    fs::write(&grants, serde_json::json!({"io":{"dirs":[],"tasks":"tasks.cooperative@1","cpu":{"provider":"cpu.scoped@1","max_workers":width,"max_domains":1,"max_physical_bytes":1073741824u64,"max_batch_bytes":8388608}}}).to_string()).unwrap();
    let baseline = root.join("baseline.tape");
    checked(
        cpu_interpreter_cli(width, true)
            .arg("run")
            .arg(&manifest)
            .arg("--grants")
            .arg(&grants)
            .arg("--record")
            .arg(&baseline)
            .output()
            .unwrap(),
    );
    let (header, parsed) =
        hirsel_rt::tape_binary::Tape::read(fs::read(&baseline).unwrap().into()).unwrap();
    let original = parsed.events().unwrap();
    let controls: Vec<_> = original
        .iter()
        .enumerate()
        .filter(|(_, event)| matches!(event.request, Request::CpuControl { .. }))
        .collect();
    assert_eq!(controls.len(), 2 * (3 + 2 * width));
    let first = controls[0].0;
    let leave = controls[2 + 2 * width].0;
    assert!(
        original[first..=leave].iter().all(|event| matches!(
            event.request,
            Request::CpuControl { .. } | Request::CpuSummary(_)
        )),
        "one contiguous CPU batch"
    );
    let set_status = |event: &mut Event, status: u32| {
        let Request::CpuControl { outcome, .. } = &mut event.request else {
            unreachable!()
        };
        *outcome = status;
    };
    for backend in ["llvm", "cranelift"] {
        let executable = root.join(backend);
        checked(
            cpu_interpreter_cli(width, true)
                .args(["build", "--fuel", "--validate-stages", "--backend", backend])
                .arg(&manifest)
                .arg("-o")
                .arg(&executable)
                .output()
                .unwrap(),
        );
        let partial = root.join("scripted.tape");
        let tir = root.join("tir.phases");
        let events = root.join("native.events");
        let mut cells = 0;
        for status in [8, 16, 17, 18, 19] {
            for (control, &(cut, _)) in controls[..3 + 2 * width].iter().enumerate() {
                let claimed = if cut == leave {
                    2 * width
                } else {
                    control.saturating_sub(2)
                };
                let mut chosen = original[..=cut].to_vec();
                set_status(chosen.last_mut().unwrap(), status);
                if cut != first && cut != leave {
                    // Already claimed children complete before the controller
                    // leaves. Keep their independently recorded mapper values,
                    // while removing controls and results for unclaimed jobs.
                    chosen.extend(original[cut + 1..leave].iter().filter(|event| {
                        matches!(&event.request, Request::CpuSummary(summary) if summary.ordinal < claimed as u64)
                    }).cloned());
                    let mut leaving = original[leave].clone();
                    set_status(&mut leaving, status);
                    chosen.push(leaving);
                }
                chosen.extend_from_slice(&original[leave + 1..]);
                let mut bytes = vec![];
                {
                    let mut writer =
                        hirsel_rt::tape_binary::Writer::new(&mut bytes, &header).unwrap();
                    for event in chosen {
                        writer.append(&event).unwrap();
                    }
                    writer
                        .finish_with_outcome(parsed.outcome.as_ref().unwrap())
                        .unwrap();
                }
                fs::write(&partial, bytes).unwrap();
                checked(
                    cpu_interpreter_cli(width, true)
                        .arg("run")
                        .arg(&manifest)
                        .arg("--replay")
                        .arg(&partial)
                        .env("HIRSEL_TIR_CPU_PHASES", &tir)
                        .output()
                        .unwrap(),
                );
                checked(
                    Command::new(&executable)
                        .arg("--hirsel-replay")
                        .arg(&partial)
                        .arg("--hirsel-trace-events")
                        .arg(&events)
                        .output()
                        .unwrap(),
                );
                let native = cpu_ordered_boundaries(&fs::read(&events).unwrap());
                let expected = native
                    .iter()
                    .map(|event| format!("{event:?}\n"))
                    .collect::<String>();
                let actual = fs::read_to_string(&tir).unwrap();
                let a: Vec<_> = actual.lines().collect();
                let b: Vec<_> = expected.lines().collect();
                for (index, (a, b)) in a.iter().zip(&b).enumerate() {
                    assert_eq!(
                        a, b,
                        "W{width} {backend} status {status} cut {cut} event {index}"
                    );
                }
                assert_eq!(
                    a.len(),
                    b.len(),
                    "W{width} {backend} status {status} cut {cut}"
                );
                let next = native
                    .iter()
                    .enumerate()
                    .filter(|(_, event)| {
                        matches!(
                            event.phase,
                            hirsel_interp::library::CpuPhase::Control(
                                hirsel_interp::library::CpuControl { action: 4, .. },
                                _
                            )
                        )
                    })
                    .nth(1)
                    .unwrap()
                    .0;
                let prefix = &native[..next];
                assert_eq!(
                    prefix
                        .iter()
                        .filter(|event| matches!(
                            event.phase,
                            hirsel_interp::library::CpuPhase::Spawn(_)
                        ))
                        .count(),
                    claimed
                );
                assert_eq!(
                    prefix
                        .iter()
                        .filter(|event| matches!(
                            event.phase,
                            hirsel_interp::library::CpuPhase::End(_)
                        ))
                        .count(),
                    claimed
                );
                assert_eq!(
                    prefix
                        .iter()
                        .filter(|event| matches!(
                            event.phase,
                            hirsel_interp::library::CpuPhase::Rest
                        ))
                        .count(),
                    claimed
                );
                cells += 1;
            }
        }
        eprintln!(
            "Ordered W{width} {backend}: {cells} withdrawal/cancellation receipt cuts, TIR/native canonical boundaries equal and fresh reentry succeeds"
        );
    }
}
