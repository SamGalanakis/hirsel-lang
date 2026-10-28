fn native_result_cleanup_cuts() {
    native_cleanup_cut_history(1, include_str!("cleanup_cuts.hir"), 3, "success");
}

fn native_cleanup_cut_history(width: usize, source: &str, logs: usize, name: &str) {
    let _compiler = QUAL_COMPILER.lock().unwrap_or_else(|e| e.into_inner());
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let manifest = qual_manifest(root, source);
    let grants = root.join("grants.json");
    let mut authority = qual_grants(width);
    authority["io"]["stdout"] = serde_json::json!("stdout.bytes@1");
    authority["recording"] = serde_json::json!("durable");
    fs::write(&grants, authority.to_string()).unwrap();
    for backend in ["llvm", "cranelift"] {
        let executable = qual_build(root, &manifest, backend);
        let tape = root.join("cleanup.tape");
        let projection = root.join("cleanup.projection");
        checked(
            Command::new(&executable)
                .arg("--hirsel-grants")
                .arg(&grants)
                .arg("--hirsel-record")
                .arg(&tape)
                .env(hirsel_rt::free_replay::Report::VAR, &projection)
                .output()
                .unwrap(),
        );
        let live_projection = fs::read_to_string(&projection).unwrap();
        let bytes = fs::read(&tape).unwrap();
        let complete = hirsel_free::tape::parse(&bytes).unwrap();
        assert!(!complete.torn && complete.end.is_some());
        assert_eq!(
            complete.logs.len(),
            logs,
            "every native result path must be recorded"
        );
        assert!(
            complete
                .cut
                .iter()
                .filter(|(path, _)| !path.is_empty())
                .all(|(_, p)| p.op >= 2)
        );
        let mut at = hirsel_free::tape::MAGIC.len();
        let mut frames = vec![];
        while at < bytes.len() {
            let len = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            frames.push((at, at + 8 + len));
            at += 8 + len;
        }
        assert_eq!(at, bytes.len());
        assert!(
            frames.len() > 3,
            "durable native phases must produce intermediate cuts"
        );
        assert_eq!(bytes[frames.last().unwrap().0 + 8], 3);
        let partial = root.join("partial.tape");
        let probe = root.join("partial.probe");
        let lifecycle = root.join("partial.probe.lifecycle");
        let mut cuts = 0;
        let mut progressed = 0;
        // Replay every byte of every native phase frame, including result
        // publication and cleanup. The parser's committed prefix and native
        // replay must agree for each torn header, payload and checksum.
        for &(start, finish) in &frames[1..] {
            let previous = hirsel_free::tape::parse(&bytes[..start]).unwrap();
            if previous.cut.keys().any(|path| !path.is_empty()) {
                progressed += 1;
            }
            let mut reference = None;
            for end in start..finish {
                fs::write(&partial, &bytes[..end]).unwrap();
                let parsed = hirsel_free::tape::parse(&bytes[..end]).unwrap();
                assert_eq!(parsed.torn, end != start);
                assert_eq!(parsed.logs, previous.logs);
                assert_eq!(parsed.cut, previous.cut);
                assert!(parsed.end.is_none());
                let mut replay = Command::new(&executable);
                replay
                    .arg("--hirsel-replay")
                    .arg(&partial)
                    .env("HIRSEL_RT_WORKERS", "1")
                    .env("HIRSEL_RT_CPU_PROBE", &probe)
                    .env(hirsel_rt::free_replay::Report::VAR, &projection);
                let output = qual_native_output(&mut replay, root);
                let observed = fs::read_to_string(&projection).unwrap();
                assert!(
                    observed.contains(&format!("cut: {:?},", previous.cut)),
                    "cut {end}: {observed}"
                );
                assert!(fs::read(&probe).unwrap().is_empty());
                assert!(
                    fs::read(&lifecycle).unwrap().is_empty(),
                    "a torn-history replay started CPU workers at byte {end}"
                );
                assert!(
                    observed.contains("truncated"),
                    "cut {end}: {output:?}\n{observed}"
                );
                let cell = (output.status.code(), output.stdout, output.stderr, observed);
                if let Some(reference) = &reference {
                    assert_eq!(&cell, reference, "cut {end}");
                } else {
                    reference = Some(cell);
                }
                cuts += 1;
            }
        }
        assert!(
            progressed > 1,
            "cuts must include admitted mapper/result phases"
        );
        let mut replay = Command::new(&executable);
        replay
            .arg("--hirsel-replay")
            .arg(&tape)
            .env("HIRSEL_RT_WORKERS", "1")
            .env("HIRSEL_RT_CPU_PROBE", &probe)
            .env(hirsel_rt::free_replay::Report::VAR, &projection);
        checked(qual_native_output(&mut replay, root));
        assert_eq!(fs::read_to_string(&projection).unwrap(), live_projection);
        assert!(fs::read(&probe).unwrap().is_empty());
        assert!(
            fs::read(&lifecycle).unwrap().is_empty(),
            "a complete-history replay started CPU workers"
        );
        eprintln!(
            "Free {backend} W{width} {name} native job-result/cleanup: {cuts} byte cuts across {} durable frames replayed plus complete history",
            frames.len() - 1
        );
    }
}

fn wide_cleanup_source(width: usize, name: &str) -> String {
    let main = match name {
        "success" => {
            r#"
pub fn main(rt: Io) -> Result[(), AppError] {
  let tasks = io.tasks(rt)?Grant
  let out = io.stdout(rt)?Grant
  {
    let rows = cut_batch(tasks)?Pool
    let _ = out.print(out, "results\n")
    check_rows(rows)
  }
  let _ = out.print(out, "cleaned\n")
  Ok(())
}
"#
        }
        "failure" => {
            r#"
pub fn main(rt: Io) -> Result[(), AppError] {
  let tasks = io.tasks(rt)?Grant
  let out = io.stdout(rt)?Grant
  let caught = task.isolate(tasks, fn() -> () {
    let Ok(rows) = cut_batch(tasks) else { failure.check(false, Arithmetic()); return () }
    check_rows(rows)
    let _ = out.print(out, "results\n")
    failure.check(false, Bounds())
    check_rows(rows)
  })
  failure.check(match caught { Error(primary) => primary.site.line == FAILURE_LINE, _ => false }, Arithmetic())
  let _ = out.print(out, "cleaned\n")
  Ok(())
}
"#
        }
        "cancel" => {
            r#"
pub fn main(rt: Io) -> Result[(), AppError] {
  let tasks = io.tasks(rt)?Grant
  let out = io.stdout(rt)?Grant
  task.scope(tasks, fn(scope: Scope) -> () {
    let (ready, started): (Sender[u32], Receiver[u32]) = channel.open(tasks)
    let (wake, sleep): (Sender[u32], Receiver[u32]) = channel.open(tasks)
    let child = task.async(scope, fn() -> bool {
      let Ok(rows) = cut_batch(tasks) else { failure.check(false, Arithmetic()); return false }
      check_rows(rows)
      let _ = out.print(out, "results\n")
      let mut signal = take ready
      failure.check(channel.send(tasks, mut signal, 1u32) == Ok(()), Arithmetic())
      let mut gate = take sleep
      let canceled = channel.receive(tasks, mut gate) == Error(Canceled(Requested()))
      check_rows(rows)
      canceled
    })
    let mut signal = take started
    failure.check(channel.receive(tasks, mut signal) == Ok(1u32), Arithmetic())
    task.cancel(scope, child)
    failure.check(task.await(scope, take child), Bounds())
    let _ = wake
  })
  let _ = out.print(out, "cleaned\n")
  Ok(())
}
"#
        }
        _ => unreachable!(),
    };
    let source = format!("{}\n{main}", include_str!("wide_cuts.hir"))
        .replace("WIDTH", &format!("{width}u32"))
        .replace("COUNT", &format!("{}u64", 2 * width));
    let failure_line = source
        .lines()
        .position(|line| line.trim() == "failure.check(false, Bounds())")
        .map_or(0, |line| line + 1);
    source.replace("FAILURE_LINE", &format!("{failure_line}u64"))
}

fn wide_cleanup_cuts(width: usize, name: &str) {
    let source = wide_cleanup_source(width, name);
    native_cleanup_cut_history(
        width,
        &source,
        1 + 2 * width + usize::from(name == "cancel"),
        name,
    );
}
