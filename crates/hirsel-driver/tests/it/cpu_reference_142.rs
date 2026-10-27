use hirsel_driver::{compiler_adapters, native};
use hirsel_engine::{
    QueryEngine, RunConfiguration, check_progress, default_context, default_environment,
    entry_path, run_observed_checked, run_observed_snapshot, with_compiler_stack,
};

fn grant(workers: u32) -> hirsel_run_schema::CpuGrant {
    hirsel_run_schema::CpuGrant {
        provider: "cpu.scoped@1".into(),
        max_workers: workers,
        max_domains: 1,
        max_physical_bytes: 1 << 30,
        max_batch_bytes: 8 << 20,
    }
}

fn agree(source: &str, cpu: Option<hirsel_run_schema::CpuGrant>, expected: &[u8], campaign: bool) {
    agree_in(source, cpu, expected, campaign, true, false);
}

fn agree_in(
    source: &str,
    cpu: Option<hirsel_run_schema::CpuGrant>,
    expected: &[u8],
    campaign: bool,
    free: bool,
    native: bool,
) {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("cpu.hir", source.to_owned()).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let entry = entry_path(&default_context());
        let fixture = native::Fixture {
            cpu: cpu.clone(),
            tasks_free: free,
            ..Default::default()
        };
        let run = run_observed_snapshot(
            &mut engine,
            &RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            (&fixture).into(),
        )
        .unwrap();
        let reference = native::Observation::of_reference(run.observation.as_ref().unwrap());
        assert_eq!(reference.output, expected);
        assert!(reference.failures.is_empty(), "{reference:?}");
        let checked = run_observed_checked(
            &mut engine,
            &RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            (&fixture).into(),
        )
        .unwrap();
        assert_eq!(
            native::Observation::of_reference(checked.observation.as_ref().unwrap()),
            reference
        );
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let program = native::build(
            &validated,
            &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let sites = native::Sites::new(validated.snapshot());
        let world = native::HeapWorld {
            cpu,
            tasks_free: free,
            ..Default::default()
        };
        let heap = if campaign {
            native::heap_campaign_in(&program, &entry, &sites, &world)
        } else {
            native::run_heap_in(&program, &entry, &sites, &world)
        }
        .unwrap();
        assert_eq!(heap.observation, reference);
        if native {
            let root = tempfile::tempdir().unwrap();
            let mut tools = native::Toolchain::in_tree_diagnostic(
                &root.path().join("runtime"),
                &[&program.target],
            )
            .unwrap();
            tools.execution = native::Execution::Executable;
            for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
                let objects = native::emit_backend_objects(
                    &program,
                    backend,
                    native::OptLevel::Default,
                    native::Partition::Whole,
                    false,
                    None,
                )
                .unwrap();
                let dir = root.path().join(format!("{backend:?}"));
                std::fs::create_dir_all(&dir).unwrap();
                let executable = native::link_fixture_runnable(
                    &program, &entry, &objects, &tools, &dir, &fixture,
                )
                .unwrap();
                let launched = native::launch_fixture(
                    &executable,
                    &program.target,
                    &tools,
                    &dir,
                    true,
                    native::Tape::Live {
                        record: None,
                        revocations: None,
                    },
                    &fixture,
                )
                .unwrap();
                assert_eq!(
                    native::observe(&launched, &program, &sites).unwrap(),
                    reference
                );
            }
        }
        let run = native::fixture_run_record(&program, &entry, &fixture).unwrap();
        assert_eq!(run.record().io.as_ref().unwrap().cpu, fixture.cpu);
        assert_eq!(
            run.record().io.as_ref().unwrap().tasks.as_deref(),
            Some(if free {
                "tasks.free@1"
            } else {
                "tasks.cooperative@1"
            })
        );
    });
}

fn owned_batch(width: u32) {
    let jobs = (1..=2 * width)
        .map(|n| format!("[{n}u64]"))
        .collect::<Vec<_>>()
        .join(",");
    let expected = (1..=2 * width)
        .map(|n| format!("[{}u64]", n + 7))
        .collect::<Vec<_>>()
        .join(",");
    agree(
        &format!(
            r#"
pub fn main(rt: Io) -> Result[(), GrantError] {{
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  for repeat in 0u64..2u64 {{
    let batch = core@1/cpu.map_scoped(tasks, {width}u32,
      fn() -> List[unique List[u64]] uses {{}} {{ [{jobs}] }},
      fn(take job: unique List[u64]) -> unique List[u64] uses {{}} {{ [job[0u64] + 7u64] }})
    match batch {{
      Ok(rows) => failure.check(rows == [{expected}], Bounds()),
      Error(_) => failure.check(false, Bounds()),
    }}
  }}
  let _ = out.print(output, "cpu-owned-ok\n")
  Ok(())
}}
"#
        ),
        Some(grant(width)),
        b"cpu-owned-ok\n",
        width == 1,
    );
}

#[test]
fn free_heap_and_tir_owned_batches_width_1() {
    owned_batch(1);
}
#[test]
fn free_heap_and_tir_owned_batches_width_4() {
    owned_batch(4);
}
#[test]
fn free_heap_and_tir_owned_batches_width_8() {
    owned_batch(8);
}

#[test]
fn heap_admission_refuses_before_preparation_and_bounds_before_mapping() {
    let source = r#"
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  let rows = core@1/cpu.map_scoped(tasks, WIDTH,
    fn() -> List[u64] uses {} { failure.check(PREPARE, Bounds())
      [1u64, 2u64, 3u64] },
    fn(take job: u64) -> u64 uses {} { failure.check(false, Bounds())
      job })
  match rows {
    Error(EXPECTED) => (),
    _ => failure.check(false, Bounds()),
  }
  let _ = out.print(output, "cpu-refused\n")
  Ok(())
}
"#;
    for (width, cpu, prepared, expected) in [
        ("3u32", Some(grant(1)), false, "InvalidWidth()"),
        ("1u32", None, false, "NotGranted()"),
        ("4u32", Some(grant(1)), false, "WidthUnavailable()"),
        ("1u32", Some(grant(1)), true, "BatchTooLarge()"),
    ] {
        agree(
            &source
                .replace("WIDTH", width)
                .replace("PREPARE", if prepared { "true" } else { "false" })
                .replace("EXPECTED", expected),
            cpu,
            b"cpu-refused\n",
            false,
        );
    }
}

#[test]
fn heap_mapper_failure_drains_the_scope_before_reentry() {
    agree(
        r#"
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let caught = task.isolate(tasks, fn() -> () {
    let batch = core@1/cpu.map_scoped(tasks, 1u32,
      fn() -> List[u64] uses {} { [1u64, 2u64] },
      fn(take job: u64) -> Shared[List[u64]] uses {} {
        failure.check(job != 1u64, Bounds())
        shared.freeze([job])
      })
    failure.check(false, Arithmetic())
  })
  failure.check(match caught { Error(_) => true, _ => false }, Arithmetic())
  let after = core@1/cpu.map_scoped(tasks, 1u32,
    fn() -> List[u64] uses {} { [7u64] },
    fn(take job: u64) -> u64 uses {} { job + 1u64 })
  failure.check(after == Ok([8u64]), Arithmetic())
  let _ = out.print(io.stdout(rt)?, "cpu-reentered\n")
  Ok(())
}
"#,
        Some(grant(1)),
        b"cpu-reentered\n",
        false,
    );
}

#[test]
fn cpu_scalar_scopes_agree_under_free_and_ordered_at_widths_1_4_8() {
    for width in [1, 4, 8] {
        let jobs = (1..=2 * width)
            .map(|n| format!("{n}u64"))
            .collect::<Vec<_>>()
            .join(",");
        let expected = (1..=2 * width)
            .map(|n| format!("{}u64", n + 7))
            .collect::<Vec<_>>()
            .join(",");
        let source = include_str!("../../../hirsel-cli/tests/support/cpu_pool/interpreter.hir")
            .replace("WIDTH", &format!("{width}u32"))
            .replace("JOBS", &format!("[{jobs}]"))
            .replace("EXPECTED", &format!("[{expected}]"));
        for free in [true, false] {
            agree_in(
                &source,
                Some(grant(width)),
                b"cpu-scalar-ok\n",
                false,
                free,
                width == 1,
            );
        }
    }
}

#[test]
fn cpu_ordered_unknown_mapper_refuses_before_preparation() {
    for mapper in [
        "let xs = [job]; xs[0u64]",
        "core@1/f64.bits(float.sqrt(core@1/f64.from_bits(job)))",
    ] {
        agree_in(
            &format!(
                r#"
pub fn main(rt: Io) -> Result[(), GrantError] {{
  let tasks = io.tasks(rt)?
  let rows = core@1/cpu.map_scoped(tasks, 1u32,
    fn() -> List[u64] uses {{}} {{ failure.check(false, Bounds()); [1u64] }},
    fn(take job: u64) -> u64 uses {{}} {{ {mapper} }})
  failure.check(rows == Error(MapperNotCertified()), Bounds())
  let _ = out.print(io.stdout(rt)?, "cpu-uncertified\n")
  Ok(())
}}
"#
            ),
            Some(grant(1)),
            b"cpu-uncertified\n",
            false,
            false,
            false,
        );
    }
}

#[test]
fn cpu_equal_owned_jobs_keep_distinct_checked_ownership() {
    agree(
        r#"
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let rows = core@1/cpu.map_scoped(tasks, 4u32,
    fn() -> List[unique List[u64]] uses {} { [[1u64], [1u64], [1u64], [1u64]] },
    fn(take job: unique List[u64]) -> unique List[u64] uses {} { take job })
  failure.check(rows == Ok([[1u64], [1u64], [1u64], [1u64]]), Bounds())
  let _ = out.print(io.stdout(rt)?, "cpu-equal-ok\n")
  Ok(())
}
"#,
        Some(grant(4)),
        b"cpu-equal-ok\n",
        false,
    );
}
