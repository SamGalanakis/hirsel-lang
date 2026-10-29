include!("compiler.rs");
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    process::Command,
};

include!("job_stages.rs");
include!("physical_host.rs");
include!("physical_exclusive.rs");

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
#[cfg(feature = "native-tsan")]
fn emitted_mapper_race(executable: &std::path::Path, stderr: &str) {
    let mut mapper_pcs = 0;
    for line in stderr
        .lines()
        .filter(|line| line.trim_start().starts_with("#0 "))
    {
        let Some((_, suffix)) = line.split_once("+0x") else {
            continue;
        };
        let offset: String = suffix
            .chars()
            .take_while(|c| c.is_ascii_hexdigit())
            .collect();
        let output = checked(
            Command::new("llvm-addr2line")
                .args(["-f", "-e"])
                .arg(executable)
                .arg(format!("0x{offset}"))
                .output()
                .unwrap(),
        );
        let symbol = String::from_utf8(output.stdout).unwrap();
        if symbol.starts_with("_H1") && symbol.contains("cpu__physical") {
            eprintln!("emitted mapper PC +0x{offset}: {}", symbol.trim());
            mapper_pcs += 1;
        }
    }
    assert!(
        mapper_pcs >= 2,
        "both conflicting accesses must name emitted mapper PCs\n{stderr}"
    );
}
fn overlap(text: &str, width: usize, exclusive: &BTreeSet<usize>) -> bool {
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
        if !exclusive.is_empty() && !exclusive.contains(&(cpu as usize)) {
            continue;
        }
        intervals.insert((worker, start), (end, progress));
        events.push((start, 1, worker, cpu));
        events.push((end, 0, worker, cpu));
    }
    if workers.len() != width {
        eprintln!("every worker must make CPU progress: sampled {workers:?}, expected W{width}");
        return false;
    }
    events.sort_unstable();
    let required = exclusive.len().max(1).min(width);
    for (time, enter, worker, cpu) in events {
        if enter == 1 {
            present.insert(worker, (cpu, time));
        } else {
            present.remove(&worker);
        }
        if present.len() >= required
            && present
                .values()
                .map(|(cpu, _)| *cpu)
                .collect::<BTreeSet<_>>()
                .len()
                >= required
        {
            for (worker, (cpu, start)) in &present {
                let (end, progress) = intervals[&(*worker, *start)];
                eprintln!(
                    "overlap W{width} required={required} worker={worker} CPU={cpu} [{start},{end}] cpu_ns={progress}"
                );
            }
            return true;
        }
    }
    false
}
fn run_width(width: usize) {
    let mut failures = Vec::new();
    for provider in ["tasks.free@1", "tasks.cooperative@1"] {
        // Diagnose both policies even when the host blocks the first one.
        if let Err(failure) = std::panic::catch_unwind(|| run_width_policy(width, provider)) {
            failures.push(failure);
        }
    }
    if let Some(failure) = failures.into_iter().next() {
        std::panic::resume_unwind(failure);
    }
}
fn run_width_policy(width: usize, provider: &str) {
    run_width_policy_backends(width, provider, &["llvm", "cranelift"]);
}
fn run_width_policy_backends(width: usize, provider: &str, backends: &[&str]) {
    run_width_policy_backends_mode(width, provider, backends, false);
}
fn allocating_cpu_source(source: &str) -> String {
    source
        .lines()
        .flat_map(|line| {
            let indent = &line[..line.len() - line.trim_start().len()];
            if line.trim() == "let mut x = job" {
                vec![
                    format!("{indent}let mut out = [job]"),
                    format!("{indent}list.reserve(mut out, 128u64)"),
                    line.to_owned(),
                ]
            } else if line.trim() == "x" {
                vec![
                    format!("{indent}list.push(mut out, x)"),
                    format!("{indent}take out"),
                ]
            } else {
                vec![line.replace(
                    "fn(take job: u64) -> u64",
                    "fn(take job: u64) -> unique List[u64]",
                )]
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        .replace(
            "-> Result[List[u64], CpuPoolError]",
            "-> Result[List[unique List[u64]], CpuPoolError]",
        )
        .replace("Ok([0u64, 1u64])", "Ok([[0u64, 0u64], [1u64, 1u64]])")
}
fn run_width_policy_backends_mode(
    width: usize,
    provider: &str,
    backends: &[&str],
    allocating: bool,
) {
    let private = std::env::var("HIRSEL_CPU_PRIVATE_SCOPE").as_deref() == Ok("inside");
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let source = include_str!("physical.hir");
    let iterations = if cfg!(feature = "native-tsan") {
        3_000_000u64
    } else if width == 1 {
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
        expected.push(if allocating {
            format!("[{q}u64, {x}u64]")
        } else {
            format!("{x}u64")
        });
    }
    let source = if allocating {
        allocating_cpu_source(source)
    } else {
        source.to_owned()
    };
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
    let mut blocked = Vec::new();
    for &backend in backends {
        let executable = root.join(backend);
        checked(
            cpu_compiler()
                .args(["build", "--fuel", "--backend", backend])
                .arg(&manifest)
                .arg("-o")
                .arg(&executable)
                .output()
                .unwrap(),
        );
        let probe = root.join("probe.txt");
        let tape = root.join(format!("{backend}.tape"));
        let mut command = Command::new(&executable);
        command
            .arg("--hirsel-grants")
            .arg(&grants)
            .env("HIRSEL_RT_CPU_PROBE", &probe)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if allocating {
            command.arg("--hirsel-record").arg(&tape);
        }
        #[cfg(feature = "native-tsan")]
        {
            use std::os::unix::process::CommandExt;
            // Only the sanitized child needs TSan's virtual shadow mappings.
            unsafe {
                command.pre_exec(|| {
                    let mut limit = std::mem::zeroed::<libc::rlimit>();
                    if libc::getrlimit(libc::RLIMIT_DATA, &mut limit) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    limit.rlim_cur = limit.rlim_max;
                    if libc::setrlimit(libc::RLIMIT_DATA, &limit) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        #[cfg(feature = "native-tsan")]
        {
            command.env("HIRSEL_TSAN_DOUBLE_CLAIM", "1");
            let mut mutant = command.spawn().unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
            let timed_out = loop {
                if mutant.try_wait().unwrap().is_some() {
                    break false;
                }
                if std::time::Instant::now() >= deadline {
                    mutant.kill().unwrap();
                    break true;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            };
            let output = mutant.wait_with_output().unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("MUTANT {provider} {backend} W{width}\n{stderr}");
            if timed_out {
                eprintln!(
                    "MUTANT {provider} {backend} W{width}: double-claim control timed out at 15s; P not qualified"
                );
                blocked.push(format!(
                    "{provider} {backend}: double-claim control timed out at 15s"
                ));
            } else if std::panic::catch_unwind(|| {
                assert_eq!(output.status.code(), Some(66), "{output:?}");
                assert!(stderr.contains("ThreadSanitizer: data race"), "{stderr}");
                emitted_mapper_race(&executable, &stderr);
            })
            .is_ok()
            {
                eprintln!("MUTANT {provider} {backend} W{width}: exit 66, emitted mapper race");
            } else {
                blocked.push(format!("{provider} {backend}: double-claim control failed"));
            }
            command.env_remove("HIRSEL_TSAN_DOUBLE_CLAIM");
        }
        // Place this test's fresh workers on distinct admitted CPUs. Affinity
        // alone does not establish that the job holds those CPUs exclusively.
        let mut allowed = unsafe { std::mem::zeroed::<libc::cpu_set_t>() };
        assert_eq!(
            unsafe { libc::sched_getaffinity(0, std::mem::size_of_val(&allowed), &mut allowed) },
            0
        );
        let mut cpus: Vec<_> = (0..libc::CPU_SETSIZE as usize)
            .filter(|&cpu| unsafe { libc::CPU_ISSET(cpu, &allowed) })
            .collect();
        assert_eq!(cpus.len(), width);
        let owned = owned_cpus(&cpus);
        let ownership = exclusive_cpus(&cpus);
        let exclusive = ownership.as_ref().cloned().unwrap_or_default();
        if private {
            let owned = owned.as_ref().expect("private delegated cgroup ownership");
            assert_eq!(owned.cpus.len(), width);
            eprintln!(
                "CPU {provider} {backend} W{width}: owned={:?} cgroup={} exclusivity={:?}",
                owned.cpus, owned.group, owned.exclusivity
            );
        }
        cpus.sort_by_key(|cpu| (!exclusive.contains(cpu), *cpu));
        let mut child = command.spawn().unwrap();
        let before = physical_host_snapshot(child.id(), &cpus);
        let mut threads = PhysicalThreads::default();
        let mut pinned = BTreeSet::new();
        let mut slot_threads = BTreeMap::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while pinned.len() < width && std::time::Instant::now() < deadline {
            threads.capture(child.id());
            // The runtime applies each slot's admitted CPU and records the
            // native TID after sched_setaffinity succeeds. Include caller slot
            // zero; on-CPU samples below independently check every placement.
            let placements =
                fs::read_to_string(root.join("probe.txt.placements")).unwrap_or_default();
            for line in placements.lines() {
                let fields: Vec<_> = line.split_whitespace().collect();
                if fields.len() != 4 {
                    continue; // A concurrent writer may not have completed its line.
                }
                assert_eq!(fields[0], "placement");
                let slot: usize = fields[1].parse().unwrap();
                let tid: u32 = fields[2].parse().unwrap();
                let cpu: usize = fields[3].parse().unwrap();
                assert!(slot < width && tid > 0, "{line}");
                assert_eq!(cpu, cpus[slot], "slot {slot} exact admitted placement");
                if let Some(previous) = slot_threads.insert(slot, tid) {
                    assert_eq!(previous, tid, "one native owner per admitted slot");
                }
                pinned.insert(slot);
            }
            if child.try_wait().unwrap().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        // The observer shares these exact CPUs. Sleep until exit instead of
        // preempting a worker every 20 ms inside its switch-free interval.
        let output = child.wait_with_output().unwrap();
        let after = physical_host_snapshot(std::process::id(), &cpus);
        let retired = fs::read_to_string(root.join("probe.txt.threads")).unwrap_or_default();
        let output = checked(output);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.contains("ThreadSanitizer:"), "{stderr}");
        #[cfg(feature = "native-tsan")]
        eprintln!("PRODUCTION {provider} {backend} W{width}: exit 0, no race, values checked");
        if !exclusive.is_empty() {
            assert_eq!(
                exclusive_cpus(&cpus),
                Ok(exclusive.clone()),
                "CPU ownership changed during the run"
            );
        }
        assert_eq!(
            pinned.len(),
            width,
            "all fresh workers must receive the exact admitted placement\n{before}\n{after}\n{threads:?}\n{retired}"
        );
        assert_eq!(
            slot_threads
                .values()
                .copied()
                .collect::<BTreeSet<_>>()
                .len(),
            width
        );
        let samples = fs::read_to_string(&probe).unwrap();
        for line in samples.lines() {
            let fields: Vec<_> = line.split_whitespace().collect();
            let slot: usize = fields[2].parse().unwrap();
            let cpu: usize = fields[3].parse().unwrap();
            assert_eq!(cpu, cpus[slot], "{line}: mapper stayed on its admitted CPU");
        }
        let rejected = fs::read_to_string(root.join("probe.txt.rejected")).unwrap();
        let observed = if private {
            &owned.as_ref().unwrap().cpus
        } else {
            &exclusive
        };
        let full_overlap = overlap(&samples, width, observed);
        if private {
            for line in samples.lines() {
                let cpu: usize = line.split_whitespace().nth(3).unwrap().parse().unwrap();
                assert!(
                    observed.contains(&cpu),
                    "sample outside owned cpuset: {line}"
                );
            }
            assert_eq!(
                owned_cpus(&cpus).unwrap().cpus,
                *observed,
                "owned cpuset changed during the run"
            );
            eprintln!(
                "CPU {provider} {backend} W{width}: owned={}/{} pinned={}/{} samples={} inside_owned=true full_overlap={full_overlap}",
                observed.len(),
                width,
                pinned.len(),
                width,
                samples.lines().count()
            );
        }
        eprintln!(
            "CPU samples accepted={} rejected={}",
            samples.lines().count(),
            rejected.lines().count()
        );
        for line in retired.lines().filter(|line| {
            [
                "worker ",
                "schedstat:",
                "voluntary_ctxt_switches:",
                "nonvoluntary_ctxt_switches:",
            ]
            .iter()
            .any(|prefix| line.starts_with(prefix))
        }) {
            eprintln!("CPU retired {line}");
        }
        if allocating {
            let replay_probe = root.join("allocating-replay.probe");
            let replay = checked(
                Command::new(&executable)
                    .arg("--hirsel-replay")
                    .arg(&tape)
                    .env("HIRSEL_RT_WORKERS", "1")
                    .env("HIRSEL_RT_CPU_PROBE", &replay_probe)
                    .output()
                    .unwrap(),
            );
            assert_eq!(replay.stdout, output.stdout);
            assert_eq!(replay.stderr, output.stderr);
            assert!(fs::read(&replay_probe).unwrap().is_empty());
        }
        if (private || allocating) && exclusive.len() != width {
            blocked.push(format!(
                "{provider} {backend}: {ownership:?}, full_overlap={full_overlap}"
            ));
            continue;
        }
        assert!(
            full_overlap,
            "no switch-free CPU overlap at W={width}, owned={observed:?}\n{before}\n{after}\n{threads:?}\n{retired}\n{samples}\n{rejected}"
        );
        if exclusive.len() == width || width == 1 {
            eprintln!(
                "CPU {provider} {backend} P({width}): switch-free overlap with positive CPU progress, ownership={ownership:?}"
            );
        } else {
            eprintln!(
                "CPU {provider} {backend} W{width}: all workers progressed; required overlap={} on exclusive CPUs {exclusive:?}; P({width}) NOT QUALIFIED, ownership={ownership:?}",
                exclusive.len().max(1)
            );
        }
    }
    assert!(blocked.is_empty(), "P({width}) not qualified: {blocked:?}");
}
