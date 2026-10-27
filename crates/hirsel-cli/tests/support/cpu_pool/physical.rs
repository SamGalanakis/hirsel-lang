use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    process::Command,
};

fn checked(output: std::process::Output) -> std::process::Output {
    assert!(
        output.status.success(),
        "{:?}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}
fn overlap(text: &str, width: usize) -> bool {
    let mut events = vec![];
    let mut present = BTreeMap::new();
    let mut intervals = BTreeMap::new();
    let mut workers = BTreeSet::new();
    for line in text.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        assert_eq!(fields.len(), 7, "{line}");
        assert_eq!(fields[0], "cpu");
        let worker: usize = fields[2].parse().unwrap();
        let cpu: u32 = fields[3].parse().unwrap();
        let start: u64 = fields[4].parse().unwrap();
        let end: u64 = fields[5].parse().unwrap();
        let progress: u64 = fields[6].parse().unwrap();
        assert!(progress > 0 && end > start && worker < width);
        workers.insert(worker);
        intervals.insert((worker, start), (end, progress));
        events.push((start, 1, worker, cpu));
        events.push((end, 0, worker, cpu));
    }
    assert_eq!(workers.len(), width, "every worker must make CPU progress");
    events.sort_unstable();
    for (time, enter, worker, cpu) in events {
        if enter == 1 {
            present.insert(worker, (cpu, time));
        } else {
            present.remove(&worker);
        }
        if present.len() == width
            && present
                .values()
                .map(|(cpu, _)| *cpu)
                .collect::<BTreeSet<_>>()
                .len()
                == width
        {
            for (worker, (cpu, start)) in &present {
                let (end, progress) = intervals[&(*worker, *start)];
                eprintln!("P W{width} worker={worker} CPU={cpu} [{start},{end}] cpu_ns={progress}");
            }
            return true;
        }
    }
    false
}
fn run_width(width: usize) {
    run_width_policy(width, "tasks.free@1");
    run_width_policy(width, "tasks.cooperative@1");
}
fn run_width_policy(width: usize, provider: &str) {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let source = include_str!("physical.hir");
    let iterations = if width == 1 {
        3_000_000u64
    } else {
        30_000_000u64
    };
    let mut expected = vec![];
    for q in 1..=width as u64 {
        let mut x = q;
        for i in 0..iterations {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(i);
            x ^= x >> 29;
        }
        expected.push(format!("{x}u64"));
    }
    let source = source
        .replace("3000000u64", &format!("{iterations}u64"))
        .replace("WIDTH64", &format!("{width}u64"))
        .replace("WIDTH", &format!("{width}u32"))
        .replace("EXPECTED", &format!("[{}]", expected.join(",")));
    fs::write(root.join("main.hir"), source).unwrap();
    let manifest = root.join("hirsel.json");
    fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"cpu_physical","version":"1"},
        "sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    let grants = root.join("grants.json");
    fs::write(&grants, serde_json::json!({"io":{"dirs":[],"tasks":provider,"cpu":{"provider":"cpu.scoped@1",
        "max_workers":width,"max_domains":1,"max_physical_bytes":1073741824u64,"max_batch_bytes":8388608}}}).to_string()).unwrap();
    for backend in ["llvm", "cranelift"] {
        let executable = root.join(backend);
        let cli = env!("CARGO_BIN_EXE_hirsel");
        checked(
            Command::new(cli)
                .args(["build", "--fuel", "--backend", backend])
                .arg(&manifest)
                .arg("-o")
                .arg(&executable)
                .output()
                .unwrap(),
        );
        let probe = root.join("probe.txt");
        let mut child = Command::new(&executable)
            .arg("--hirsel-grants")
            .arg(&grants)
            .env("HIRSEL_RT_CPU_PROBE", &probe)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        // The exact-CPU executor owns this cpuset. Place only this test's fresh
        // workers on its distinct admitted CPUs, so OS wake affinity cannot keep
        // every runnable worker on the creating CPU during the finite control.
        let mut allowed = unsafe { std::mem::zeroed::<libc::cpu_set_t>() };
        assert_eq!(
            unsafe { libc::sched_getaffinity(0, std::mem::size_of_val(&allowed), &mut allowed) },
            0
        );
        let cpus: Vec<_> = (0..libc::CPU_SETSIZE as usize)
            .filter(|&cpu| unsafe { libc::CPU_ISSET(cpu, &allowed) })
            .collect();
        assert_eq!(cpus.len(), width);
        let mut pinned = BTreeSet::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while pinned.len() < width && std::time::Instant::now() < deadline {
            if let Ok(entries) = fs::read_dir(format!("/proc/{}/task", child.id())) {
                for entry in entries.flatten() {
                    let Ok(name) = fs::read_to_string(entry.path().join("comm")) else {
                        continue;
                    };
                    let Some(worker) = name
                        .trim()
                        .strip_prefix("hirsel-cpu-")
                        .and_then(|n| n.parse::<usize>().ok())
                    else {
                        continue;
                    };
                    if worker >= width || pinned.contains(&worker) {
                        continue;
                    }
                    let tid = entry.file_name().to_string_lossy().parse::<i32>().unwrap();
                    let mut selected = unsafe { std::mem::zeroed::<libc::cpu_set_t>() };
                    unsafe { libc::CPU_SET(cpus[worker], &mut selected) };
                    if unsafe {
                        libc::sched_setaffinity(tid, std::mem::size_of_val(&selected), &selected)
                    } == 0
                    {
                        pinned.insert(worker);
                    }
                }
            }
            if child.try_wait().unwrap().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        checked(child.wait_with_output().unwrap());
        assert_eq!(
            pinned.len(),
            width,
            "all fresh workers must receive the exact admitted placement"
        );
        let samples = fs::read_to_string(&probe).unwrap();
        assert!(
            overlap(&samples, width),
            "no switch-free CPU overlap at W={width}\n{samples}"
        );
        eprintln!(
            "CPU {provider} {backend} P({width}): overlapping switch-free intervals on {width} distinct CPUs with positive CPU progress"
        );
    }
}
