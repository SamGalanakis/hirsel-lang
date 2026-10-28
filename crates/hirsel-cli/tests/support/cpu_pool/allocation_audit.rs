fn typed_prefix_allocation_audit(width: usize) {
    let count = 2 * width;
    let template = include_str!("prefix.hir");
    let prepare_line = template
        .lines()
        .position(|line| line.contains("failure.check(i != prefix"))
        .unwrap()
        + 1;
    let source = template
        .replace("PREPARE_LINE", &format!("{prepare_line}u64"))
        .replace("WIDTH", &format!("{width}u32"))
        .replace("COUNT", &format!("{count}u64"))
        .replace("LIMIT", &format!("{}u64", count + 1));
    cpu_owned_allocation_audit(
        width,
        &source,
        "tasks.free@1",
        "Free typed preparation prefixes",
    );
}

fn cpu_owned_allocation_audit(width: usize, source: &str, provider: &str, label: &str) {
    use hirsel_driver::native::{
        self, Backend, OptLevel, Options, Partition, TargetSpec, Toolchain,
    };
    let _compiler = QUAL_COMPILER.lock().unwrap_or_else(|e| e.into_inner());
    hirsel_engine::with_compiler_stack(|| {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path();
        let manifest = qual_manifest(root, source);
        let grants = root.join("grants.json");
        let mut authority = qual_grants(width);
        authority["io"]["tasks"] = serde_json::json!(provider);
        fs::write(&grants, authority.to_string()).unwrap();
        let mut loaded = hirsel_driver::load_program(&manifest).unwrap();
        loaded.check_for_execution().unwrap();
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let program = native::build(&validated, &Options::new(target.clone())).unwrap();
        let toolchain = Toolchain::in_tree_diagnostic(&root.join("runtime"), &[&target]).unwrap();
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let objects = native::emit_backend_objects(
                &program,
                backend,
                OptLevel::Speed,
                Partition::Whole,
                false,
                None,
            )
            .unwrap();
            let executable = native::link_standalone_objects(
                &program,
                &loaded.entry,
                &objects,
                &toolchain,
                root,
            )
            .unwrap();
            let tape = root.join("audit.tape");
            let observation = root.join("audit.observation");
            let mut live = Command::new(&executable);
            live.arg("--hirsel-grants")
                .arg(&grants)
                .arg("--hirsel-record")
                .arg(&tape)
                .env("HIRSEL_RT_DIAGNOSTIC", "1")
                .env("HIRSEL_RT_OBSERVATION", &observation);
            let live = checked(qual_native_output(&mut live, root));
            let observed = qual_observation(&observation);
            assert_eq!(
                observed
                    .iter()
                    .filter(|line| line.starts_with("audit "))
                    .collect::<Vec<_>>(),
                [&"audit 0 0 0 0 0".to_owned()],
                "{backend:?} W{width}: {observed:?}"
            );
            if provider == "tasks.cooperative@1" {
                hirsel_rt::host_launch::read_recording(&tape).unwrap();
            } else {
                let parsed = hirsel_free::tape::parse(&fs::read(&tape).unwrap()).unwrap();
                assert!(!parsed.torn && parsed.end.is_some());
            }
            let probe = root.join("audit.probe");
            let mut replay = Command::new(&executable);
            replay
                .arg("--hirsel-replay")
                .arg(&tape)
                .env("HIRSEL_RT_WORKERS", "1")
                .env("HIRSEL_RT_DIAGNOSTIC", "1")
                .env("HIRSEL_RT_CPU_PROBE", &probe)
                .env("HIRSEL_RT_OBSERVATION", &observation);
            let replay = checked(qual_native_output(&mut replay, root));
            assert_eq!(live.stdout, replay.stdout);
            assert_eq!(live.stderr, replay.stderr);
            assert_eq!(qual_observation(&observation), observed);
            assert!(fs::read(probe).unwrap().is_empty());
            eprintln!(
                "{label} {backend:?} W{width}: audit 0 0 0 0 0 before/after teardown, exact replay"
            );
        }
    });
}

fn ordered_allocating_cleanup_audit(width: usize) {
    let template = r#"
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  for fail in 0u64..2u64 {
    let caught = task.isolate(tasks, fn() -> () {
      let batch = core@1/cpu.map_scoped(tasks, WIDTH,
        fn() -> List[unique List[u64]] uses {} { [[0u64], [1u64], [2u64], [3u64]] },
        fn(take job: unique List[u64]) -> (unique List[u64], unique List[u64]) uses {} {
          let mut left = take job
          list.reserve(mut left, 128u64)
          let right = [left[0] + 1u64]
          if fail == 1u64 && left[0] == 0u64 { failure.check(false, Bounds()) }
          (take left, take right)
        })
      failure.check(match batch {
        Ok(rows) => rows == [([0u64], [1u64]), ([1u64], [2u64]), ([2u64], [3u64]), ([3u64], [4u64])],
        Error(_) => false,
      }, Arithmetic())
    })
    failure.check(match caught {
      Ok(_) => fail == 0u64,
      Error(primary) => fail == 1u64 && primary.site.line == FAILURE_LINE,
    }, Arithmetic())
  }
  Ok(())
}
"#;
    let line = template
        .lines()
        .position(|line| line.contains("failure.check(false, Bounds())"))
        .unwrap()
        + 1;
    let source = template
        .replace("WIDTH", &format!("{width}u32"))
        .replace("FAILURE_LINE", &format!("{line}u64"));
    cpu_owned_allocation_audit(
        width,
        &source,
        "tasks.cooperative@1",
        "Ordered allocating success/failure with two result owners",
    );
}
