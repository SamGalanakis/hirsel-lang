// Runtime slot words qualify generations within a run. The recorded spawn
// path identifies the same semantic job across different physical schedules.
fn cpu_job_stage_rows(text: &str) -> Result<BTreeMap<String, Vec<String>>, String> {
    let mut jobs = BTreeMap::<String, Vec<String>>::new();
    let mut identities = BTreeMap::new();
    let mut paths = BTreeMap::new();
    let mut batches = BTreeMap::new();
    let mut batch_paths = BTreeMap::new();
    let mut ordinals = BTreeSet::new();
    for line in text.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 6 {
            return Err(format!("invalid job row: {line}"));
        }
        let values = fields[1..5]
            .iter()
            .map(|s| s.parse::<u64>())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| format!("invalid job identity: {line}"))?;
        let [task, batch, width, ordinal] = values[..] else {
            unreachable!()
        };
        if task >> 32 == 0
            || batch >> 32 == 0
            || !matches!(width, 1 | 4 | 8 | 16)
            || ordinal >= 2 * width
        {
            return Err(format!("invalid generation or ordinal: {line}"));
        }
        let mut path = fields[5]
            .split('.')
            .map(|s| s.parse::<u32>())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| format!("invalid spawn path: {line}"))?;
        let spawn = path
            .pop()
            .ok_or_else(|| format!("empty spawn path: {line}"))?;
        let base = u64::from(spawn)
            .checked_sub(ordinal)
            .ok_or_else(|| format!("invalid batch path: {line}"))?;
        let parent = path
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(".");
        let batch_path = format!("{parent}/{base}");
        if let Some(previous) = batches.insert(batch, batch_path.clone()) {
            if previous != batch_path {
                return Err(format!("batch generation changed identity: {line}"));
            }
        }
        if let Some(previous) = batch_paths.insert(batch_path.clone(), batch) {
            if previous != batch {
                return Err(format!("batch path changed generation: {line}"));
            }
        }
        let identity = (batch, width, ordinal, fields[5].to_owned());
        if let Some(previous) = identities.insert(task, identity.clone()) {
            if previous != identity {
                return Err(format!("job identity changed: {line}"));
            }
        } else if !ordinals.insert((batch, ordinal)) {
            return Err(format!("ordinal claimed by two generations: {line}"));
        }
        if let Some(previous) = paths.insert(fields[5].to_owned(), task) {
            if previous != task {
                return Err(format!("spawn path changed generation: {line}"));
            }
        }
        let stages = jobs.entry(fields[5].to_owned()).or_default();
        let valid = match stages.len() {
            0 => fields[0] == "generation",
            1 => fields[0] == "entry",
            2 => fields[0] == "claim",
            3 => matches!(fields[0], "withdrawal" | "transfer"),
            4 => fields[0] == "retire",
            _ => false,
        };
        if !valid {
            return Err(format!(
                "out-of-order or duplicate stage: {line}, after {stages:?}"
            ));
        }
        stages.push(format!("{} {width} {ordinal} {batch_path}", fields[0]));
    }
    if jobs.values().any(|stages| stages.len() != 5) {
        return Err(format!("incomplete jobs: {jobs:?}"));
    }
    Ok(jobs)
}

fn cpu_job_stages(path: &std::path::Path) -> BTreeMap<String, Vec<String>> {
    cpu_job_stage_rows(&fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn cpu_job_stage_oracle_rejects_broken_runtime_orders_and_generations() {
    let valid = "generation 4294967297 4294967296 4 0 0\nentry 4294967297 4294967296 4 0 0\nclaim 4294967297 4294967296 4 0 0\ntransfer 4294967297 4294967296 4 0 0\nretire 4294967297 4294967296 4 0 0\n";
    assert_eq!(cpu_job_stage_rows(valid).unwrap().len(), 1);
    assert!(cpu_job_stage_rows(&valid.replace("transfer", "withdrawal")).is_ok());
    assert_eq!(
        cpu_job_stage_rows(valid),
        cpu_job_stage_rows(
            &valid
                .replace("4294967297", "4294967301")
                .replace("4294967296", "4294967300")
        )
    );
    assert!(cpu_job_stage_rows(&valid.replace("transfer", "retire")).is_err());
    assert!(cpu_job_stage_rows(&valid.replace("transfer", "claim")).is_err());
    assert!(cpu_job_stage_rows(&valid.replace("retire 4294967297", "retire 8589934593")).is_err());
    assert!(cpu_job_stage_rows(&valid.replace("claim 4294967297", "claim 4294967298")).is_err());
    assert!(
        cpu_job_stage_rows(&valid.replace("retire 4294967297 4294967296 4 0 0\n", "")).is_err()
    );
    let reused = valid
        .replace("4294967297", "8589934593")
        .replace("4294967296", "8589934592")
        .replace(" 0\n", " 1\n");
    assert_eq!(
        cpu_job_stage_rows(&format!("{valid}{reused}"))
            .unwrap()
            .len(),
        2
    );
    let duplicate = valid
        .replace("4294967297", "8589934593")
        .replace(" 0\n", " 1\n");
    assert!(cpu_job_stage_rows(&format!("{valid}{duplicate}")).is_err());
}

fn cpu_job_withdrawal(width: usize) {
    let _compiler = QUAL_COMPILER.lock().unwrap_or_else(|e| e.into_inner());
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let source = include_str!("failed.hir").replace("tasks, 1,", &format!("tasks, {width}u32,"));
    let manifest = qual_manifest(root, &source);
    let grants = root.join("grants.json");
    fs::write(&grants, qual_grants(width).to_string()).unwrap();
    for backend in ["llvm", "cranelift"] {
        let executable = qual_build(root, &manifest, backend);
        let tape = root.join("withdrawal.tape");
        let probe = root.join("withdrawal.probe");
        let mut live = Command::new(&executable);
        live.arg("--hirsel-grants")
            .arg(&grants)
            .arg("--hirsel-record")
            .arg(&tape)
            .env("HIRSEL_RT_CPU_PROBE", &probe);
        checked(qual_native_output(&mut live, root));
        let jobs = cpu_job_stages(&root.join("withdrawal.probe.jobs"));
        assert!(
            jobs.values()
                .any(|stages| stages[3].starts_with("withdrawal "))
        );
        assert!(
            jobs.values()
                .any(|stages| stages[3].starts_with("transfer ")),
            "the later successful batch must retire a transferred result"
        );
        let mut replay = Command::new(&executable);
        replay
            .arg("--hirsel-replay")
            .arg(&tape)
            .env("HIRSEL_RT_CPU_PROBE", &probe)
            .env("HIRSEL_RT_WORKERS", "1");
        checked(qual_native_output(&mut replay, root));
        assert_eq!(cpu_job_stages(&root.join("withdrawal.probe.jobs")), jobs);
        assert!(fs::read(&probe).unwrap().is_empty());
        assert!(
            fs::read(root.join("withdrawal.probe.lifecycle"))
                .unwrap()
                .is_empty()
        );
        eprintln!(
            "Free {backend} W{width} withdrawal and transfer, {} retired job generations; one-permit replay matched",
            jobs.len()
        );
    }
}
