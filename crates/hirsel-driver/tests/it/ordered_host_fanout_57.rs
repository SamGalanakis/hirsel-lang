//! Like the compact custody witness in runtime50_resume, with an entered host
//! marker: a pre-operation resume point alone is not an in-operation leave.
use hirsel_driver::native::{self, Backend, Fixture, OptLevel, Partition, Tape, Toolchain};
use hirsel_engine::*;

const SOURCE: &str = r#"
pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let c=io.monotonic_clock(rt)?
  let now=match clock.monotonic(c) {Ok(at)=>at,Error(_)=>return Ok(())}
  let deadline=match instant.checked_add(c,now,10u64) {Some(at)=>at,None()=>return Ok(())}
  let output=io.stdout(rt)?
  let total=task.scope(tasks,fn(s:Scope)->u64 {
    let mut set:TaskSet[Result[u64,()]] of s=task.set(s)
    for seed in 1u64..CHILDREN {
      task.add(s,mut set,task.async(s,fn()->Result[u64,()] {
        let _=clock.sleep_until(c,deadline)
        Ok(seed)
      }))
    }
    let mut sum=0u64
    if let Ok(rows)=std@1/task.all(s,take set) {for x in rows {sum=sum+x}}
    sum
  })
  let _=out.print(output,f"{total}\n")
  Ok(())
}
"#;

#[test]
fn eight_entered_host_waits_release_stacks_and_replay() {
    fanout(8);
}
#[test]
fn sixteen_entered_host_waits_release_stacks_and_replay() {
    fanout(16);
}
#[test]
fn thirty_two_entered_host_waits_release_stacks_and_replay() {
    fanout(32);
}

fn fanout(count: usize) {
    with_compiler_stack(|| {
        let source = SOURCE.replace("CHILDREN", &format!("{}u64", count + 1));
        let mut engine = crate::source_std::engine();
        let file = engine.sources.add("main.hir", source.into()).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let entry = entry_path(&default_context());
        let fixture = Fixture {
            monotonic: Some(0),
            ..Default::default()
        };
        let reference = run_observed_snapshot(
            &mut engine,
            &RunConfiguration {
                entry: entry.clone(),
                hash_seed: 50,
                inputs: vec![],
            },
            (&fixture).into(),
        )
        .unwrap();
        let expected = native::Observation::of_reference(reference.observation.as_ref().unwrap());
        let input = engine.export_native_input().unwrap();
        let sites = native::Sites::new(&input);
        let validated = engine.import_native_input(input.clone()).unwrap();
        let target = native::TargetSpec::x86_64_unknown_linux_gnu();
        let program = native::build(&validated, &native::Options::new(target.clone())).unwrap();
        let root = tempfile::Builder::new()
            .prefix("ordered-host-fanout57-")
            .tempdir()
            .unwrap();
        let mut tools =
            Toolchain::in_tree_diagnostic(&root.path().join("runtime"), &[&target]).unwrap();
        tools.execution = native::Execution::Executable;
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let objects = native::emit_backend_objects(
                &program,
                backend,
                OptLevel::Default,
                Partition::Whole,
                false,
                None,
            )
            .unwrap();
            for width in [1, 4] {
                let dir = root.path().join(format!("{backend:?}-{width}"));
                std::fs::create_dir_all(&dir).unwrap();
                let runnable = native::link_fixture_runnable(
                    &program, &entry, &objects, &tools, &dir, &fixture,
                )
                .unwrap();
                // Placement is excluded from the semantic binding. Harness
                // admission uses it to exercise the actual physical dispatcher.
                let mut run = native::fixture_run_record(&program, &entry, &fixture)
                    .unwrap()
                    .record()
                    .clone();
                run.placement =
                    Some(serde_json::from_value(serde_json::json!({"cores":width})).unwrap());
                std::fs::write(
                    dir.join("run.json"),
                    run.validated().unwrap().launch_bytes(),
                )
                .unwrap();
                let tape = dir.join("wait.tape");
                for replay in [false, true] {
                    let trace = dir.join(format!("compact-{replay}"));
                    tools.wrapper = vec![
                        "/usr/bin/env".into(),
                        format!("HIRSEL_RT_COMPACT_TRACE={}", trace.display()),
                    ];
                    let cell = format!("{backend:?} width={width} replay={replay}");
                    let launched = native::launch_fixture(
                        &runnable,
                        &target,
                        &tools,
                        &dir,
                        true,
                        if replay {
                            Tape::Replay(&tape)
                        } else {
                            Tape::Live {
                                record: Some(&tape),
                                revocations: None,
                            }
                        },
                        &fixture,
                    )
                    .unwrap_or_else(|error| panic!("{cell}: {error}"));
                    assert_eq!(
                        native::observe(&launched, &program, &sites).unwrap(),
                        expected,
                        "{cell}"
                    );
                    let trace = std::fs::read_to_string(trace).unwrap();
                    check_pending_leaves(&trace, &cell, count, width);
                }
            }
        }
    });
}

fn check_pending_leaves(trace: &str, cell: &str, count: usize, width: u32) {
    use std::collections::BTreeSet;
    let rows: Vec<Vec<&str>> = trace
        .lines()
        .map(|s| s.split_whitespace().collect())
        .collect();
    let mut parked = BTreeSet::new();
    let mut held = BTreeSet::new();
    let mut peak = 0;
    for (index, row) in rows.iter().enumerate() {
        match row.first().copied() {
            Some("runtime") if row.get(4) == Some(&"33") && row.get(5) == Some(&"host-park") => {
                parked.insert(row[1]);
                assert!(
                    rows[index + 1..]
                        .iter()
                        .any(|next| next.first() == Some(&"leave")
                            && next.get(1) == row.get(1)
                            && next.get(4) == Some(&"1")),
                    "{cell}: entered parked frame retained its stack\n{trace}"
                );
            }
            Some("hold") if row.get(1) != Some(&"0") => {
                held.insert(row[1]);
                peak = peak.max(held.len());
            }
            Some("resume" | "activate") => {
                held.remove(row[1]);
            }
            _ => {}
        }
    }
    assert_eq!(
        parked.len(),
        count,
        "{cell}: genuine entered host waits\n{trace}"
    );
    assert!(
        peak <= width as usize,
        "{cell}: held {peak} stacks with width {width}\n{trace}"
    );
    eprintln!(
        "{cell}: genuine entered parked={} peak held={peak}",
        parked.len()
    );
}
