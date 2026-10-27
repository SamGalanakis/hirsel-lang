fn native_result_cleanup_cuts() {
    let _compiler = QUAL_COMPILER.lock().unwrap_or_else(|e| e.into_inner());
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let manifest = qual_manifest(root, include_str!("cleanup_cuts.hir"));
    let grants = root.join("grants.json");
    let mut authority = qual_grants(1);
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
            3,
            "the caller and both native result paths must be recorded"
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
        eprintln!(
            "Free {backend} native job-result/cleanup: {cuts} byte cuts across {} durable frames replayed plus complete history",
            frames.len() - 1
        );
    }
}
