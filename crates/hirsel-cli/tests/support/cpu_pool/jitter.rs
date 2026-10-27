fn check_cpu_lifecycle(path: &std::path::Path, width: usize, jitter: bool, domain_count: usize) {
    let text = fs::read_to_string(path).unwrap();
    let mut live = BTreeMap::new();
    let mut domains = BTreeMap::<u64, BTreeSet<usize>>::new();
    let mut retired = BTreeMap::<u64, BTreeSet<usize>>::new();
    let mut shutdown = BTreeSet::new();
    let mut perturbations = 0;
    let mut runs = 0;
    for line in text.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        assert_eq!(fields.len(), 4, "{line}");
        let domain: u64 = fields[1].parse().unwrap();
        let owner: usize = fields[2].parse().unwrap();
        let value: usize = fields[3].parse().unwrap();
        match fields[0] {
            "start" => {
                assert!(domain > 0 && value < width);
                assert!(live.insert(owner, (domain, false, false)).is_none());
                assert!(domains.entry(domain).or_default().insert(value));
            }
            "park" => {
                let (_, exited, parked) = live.get_mut(&owner).unwrap();
                assert!(!*exited && !*parked, "{line}");
                *parked = true;
            }
            "wake" => {
                let (_, exited, parked) = live.get_mut(&owner).unwrap();
                assert!(!*exited && *parked, "{line}");
                *parked = false;
            }
            "run" => {
                let (_, exited, parked) = live[&owner];
                assert!(!exited && !parked, "{line}");
                runs += 1;
            }
            "exit" => {
                let (_, exited, parked) = live.get_mut(&owner).unwrap();
                assert!(!*exited && !*parked, "{line}");
                *exited = true;
            }
            "retire" => {
                assert_eq!(live.remove(&owner), Some((domain, true, false)), "{line}");
                assert!(retired.entry(domain).or_default().insert(owner));
            }
            "shutdown" => {
                assert!(shutdown.insert(domain));
                assert_eq!(value, width);
                assert_eq!(domains[&domain].len(), width);
                assert_eq!(retired[&domain].len(), width);
                assert!(live.values().all(|(active, _, _)| *active != domain));
            }
            "jitter" => {
                assert!(jitter && domains.contains_key(&domain) && !shutdown.contains(&domain));
                assert!(owner < width && (1..=4).contains(&value));
                perturbations += 1;
            }
            other => panic!("unexpected CPU lifecycle event {other}: {line}"),
        }
    }
    assert!(live.is_empty(), "CPU workers survived shutdown: {text}");
    assert_eq!(
        domains.len(),
        domain_count,
        "each operation constructs a fresh domain"
    );
    assert_eq!(shutdown.len(), domain_count);
    assert!(runs > 0);
    assert_eq!(
        perturbations > 0,
        jitter,
        "mapper admission or checkpoints must execute jitter"
    );
}

fn cpu_values_with_jitter(width: usize) {
    let mut integers = vec![];
    let mut floats = vec![];
    for job in 1..=2 * width as u64 {
        let mut x = job;
        let mut y = f64::from_bits(4607182418800017408 + job);
        for i in 0..128 {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(i);
            x ^= x >> 29;
            y *= 1.0000000000000002;
            y += 0.0000000000000001;
        }
        integers.push(format!("{x}u64"));
        floats.push(format!("{}u64", y.to_bits()));
    }
    let source = || {
        let text = include_str!("jitter.hir");
        text.replace("WIDTH", &format!("{width}u32"))
            .replace("COUNT", &format!("{}u64", 2 * width))
            .replace("EXPECTED_INTS", &format!("[{}]", integers.join(",")))
            .replace("EXPECTED_FLOATS", &format!("[{}]", floats.join(",")))
    };
    cpu_source_with_jitter(width, &source(), 2);
    cpu_source_with_jitter(width, &float_layout_source(width), 32 / width);
}

fn cpu_source_with_jitter(width: usize, source: &str, domain_count: usize) {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    fs::write(root.join("main.hir"), source).unwrap();
    let manifest = root.join("hirsel.json");
    fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"cpu_jitter","version":"1"},
        "sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    let grants_path = root.join("grants.json");
    let cli = env!("CARGO_BIN_EXE_hirsel");
    if width == 1 {
        let grants = serde_json::json!({"io":{"dirs":[],"tasks":"tasks.free@1",
            "cpu":{"provider":"cpu.scoped@1","max_workers":1,"max_domains":1,
            "max_physical_bytes":1073741824u64,"max_batch_bytes":8388608}}});
        checked(
            Command::new(cli)
                .arg("run")
                .arg(&manifest)
                .arg("--grants")
                .arg(grants.to_string())
                .output()
                .unwrap(),
        );
    }
    for backend in ["llvm", "cranelift"] {
        for provider in ["tasks.free@1", "tasks.cooperative@1"] {
            let executable = root.join(format!("{backend}-{provider}"));
            checked(
                Command::new(cli)
                    .args(["build", "--fuel", "--validate-stages", "--backend", backend])
                    .arg(&manifest)
                    .arg("-o")
                    .arg(&executable)
                    .output()
                    .unwrap(),
            );
            let grants = serde_json::json!({"placement":{"cores":1,"reorder":"auto"},
                "io":{"dirs":[],"tasks":provider,"cpu":{"provider":"cpu.scoped@1",
                "max_workers":width,"max_domains":1,"max_physical_bytes":1073741824u64,"max_batch_bytes":8388608}}});
            fs::write(&grants_path, grants.to_string()).unwrap();
            let mut ordered_reference = None;
            for seed in [0, 1, 7] {
                let probe = root.join("cpu.probe");
                let tape = root.join("cpu.tape");
                let trace = root.join("cpu.events");
                let observation = root.join("cpu.observation");
                let ordered = provider == "tasks.cooperative@1";
                let evidence = || {
                    if ordered {
                        fs::read(&trace).unwrap()
                    } else {
                        fs::read_to_string(&observation)
                            .unwrap()
                            .lines()
                            .filter(|line| !line.starts_with("placement "))
                            .collect::<Vec<_>>()
                            .join("\n")
                            .into_bytes()
                    }
                };
                let mut live = Command::new(&executable);
                live.arg("--hirsel-grants")
                    .arg(&grants_path)
                    .arg("--hirsel-record")
                    .arg(&tape)
                    .env("HIRSEL_RT_OBSERVATION", &observation)
                    .env("HIRSEL_RT_CPU_PROBE", &probe)
                    .env("HIRSEL_RT_CPU_JITTER", seed.to_string());
                if ordered {
                    live.arg("--hirsel-trace-events").arg(&trace);
                }
                let live = checked(live.output().unwrap());
                check_cpu_lifecycle(
                    &root.join("cpu.probe.lifecycle"),
                    width,
                    seed != 0,
                    domain_count,
                );
                let events = evidence();
                assert!(!events.is_empty());
                if ordered {
                    let mut recording = hirsel_rt::host_launch::read_recording(&tape).unwrap();
                    let binding = recording.binding.as_mut().unwrap();
                    binding.admitted_at = 0;
                    binding.placement = None;
                    let observed = (events.clone(), recording);
                    if let Some(reference) = &ordered_reference {
                        assert_eq!(&observed, reference, "{backend} W{width} seed={seed}");
                    } else {
                        ordered_reference = Some(observed);
                    }
                }
                let input = fs::read(&tape).unwrap();
                if !ordered {
                    let free = hirsel_free::tape::parse(&input).unwrap();
                    assert!(!free.torn && free.end.is_some());
                    assert!(!free.logs.is_empty() && !free.cut.is_empty());
                }
                let mut replay = Command::new(&executable);
                replay
                    .arg("--hirsel-replay")
                    .arg(&tape)
                    .env("HIRSEL_RT_OBSERVATION", &observation)
                    .env("HIRSEL_RT_WORKERS", "1")
                    .env("HIRSEL_RT_CPU_PROBE", &probe);
                if ordered {
                    replay.arg("--hirsel-trace-events").arg(&trace);
                }
                let replay = checked(replay.output().unwrap());
                assert_eq!(live.stdout, replay.stdout);
                assert_eq!(live.stderr, replay.stderr);
                assert_eq!(
                    evidence(),
                    events,
                    "{backend} {provider} W{width} seed={seed}"
                );
                assert_eq!(fs::read(&tape).unwrap(), input);
                assert!(fs::read(&probe).unwrap().is_empty());
                assert!(
                    fs::read(root.join("cpu.probe.lifecycle"))
                        .unwrap()
                        .is_empty()
                );
            }
            eprintln!(
                "B04/E04 {backend} {provider} W{width}: 3 live/replay cells, {domain_count} fresh domains per live cell"
            );
        }
    }
}

fn float_layout_source(width: usize) -> String {
    // SEM-2 exposes one canonical NaN. Include payloads, signs and signaling
    // encodings at entry, plus both halfway directions and gradual underflow.
    let singles = [
        0u32,
        0x8000_0000,
        0x7fc0_1234,
        0xffc0_4321,
        0x7f80_0001,
        0xff80_0001,
        0x7f80_0000,
        0xff80_0000,
        1,
        0x8000_0001,
        0x007f_ffff,
        0x0080_0000,
        0x7f7f_ffff,
        0xff7f_ffff,
        0x3f80_0001,
        0x3f80_0003,
        0xc079_999a,
        0x4f00_0000,
        0xcf00_0000,
        0x3eaa_aaab,
    ];
    let doubles = [
        0u64,
        0x8000_0000_0000_0000,
        0x7ff8_0000_1234_5678,
        0xfff8_0000_8765_4321,
        0x7ff0_0000_0000_0001,
        0xfff0_0000_0000_0001,
        0x7ff0_0000_0000_0000,
        0xfff0_0000_0000_0000,
        1,
        0x8000_0000_0000_0001,
        0x3690_0000_0000_0000,
        0x3690_0000_0000_0004,
        0x47ef_ffff_f000_0000,
        0x47ef_ffff_efff_ffff,
        0x3ff0_0000_1000_0000,
        0x3ff0_0000_3000_0000,
        (-3.9f64).to_bits(),
        2147483648.0f64.to_bits(),
        (-2147483648.0f64).to_bits(),
        0x3fd5_5555_5555_5555,
    ];
    let bits32 = |x: f32| if x.is_nan() { 0x7fc0_0000 } else { x.to_bits() };
    let bits64 = |x: f64| {
        if x.is_nan() {
            0x7ff8_0000_0000_0000
        } else {
            x.to_bits()
        }
    };
    let mut jobs = vec![];
    let mut expected = [vec![], vec![]];
    for ordinal in 0..32u64 {
        let a = singles[ordinal as usize % singles.len()];
        let b = doubles[ordinal as usize % doubles.len()];
        jobs.push(format!("FloatJob(single: core@1/f32.from_bits({a}u32), double: core@1/f64.from_bits({b}u64), ordinal: {ordinal}u64)"));
        for (repeat, output) in expected.iter_mut().enumerate() {
            let mut single = f32::from_bits(a);
            let mut double = f64::from_bits(b);
            if repeat == 1 {
                for _ in 0..128 {
                    single *= 1.0000001f32;
                    single += 0.00000005f32;
                    double *= 1.0000000000000002f64;
                    double += 0.0000000000000001f64;
                }
            }
            let truncated = if !double.is_finite() {
                "Error(NotFinite)".to_owned()
            } else if double.trunc() < f64::from(i32::MIN) || double.trunc() > f64::from(i32::MAX) {
                "Error(OutOfRange)".to_owned()
            } else {
                format!("Ok({}i32)", double as i32)
            };
            output.push(format!(
                "({}u32, {}u64, {}u64, {}u32, {}u64, {truncated})",
                bits32(single),
                bits64(double),
                bits64(f64::from(single)),
                bits32(double as f32),
                bits64((ordinal + 9007199254740992) as f64)
            ));
        }
    }
    include_str!("jitter_float.hir")
        .replace("WIDTH", &format!("{width}u32"))
        .replace("JOB_COUNT", &format!("{}u64", 2 * width))
        .replace("BATCHES", &format!("{}u64", 16 / width))
        .replace("JOBS", &format!("[{}]", jobs.join(",")))
        .replace("EXPECTED_RAW", &format!("[{}]", expected[0].join(",")))
        .replace("EXPECTED_ROUNDED", &format!("[{}]", expected[1].join(",")))
}
