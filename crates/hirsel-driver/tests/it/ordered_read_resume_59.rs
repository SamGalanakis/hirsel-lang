//! Read certificates retain exactly the mutable slot and typed ABI result.
use hirsel_engine::with_compiler_stack;
use hirsel_lir::continuation::{Representation, ResumeAction, RuntimeOpKind, RuntimeResultSource};

pub(super) const STDIN: &str = r#"
pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let input=io.stdin(rt)?
  let output=io.stdout(rt)?
  let value=task.scope(tasks,fn(s:Scope)->u64 {
    let child=task.async(s,fn()->u64 {
      let mut bytes:List[u8]=[]
      let read=stdin.read(input,mut bytes,4u64)
      failure.check(read==Ok(3u64),Arithmetic())
      failure.check(bytes==[71u8,0u8,255u8],Arithmetic())
      7u64
    })
    task.await(s,take child)
  })
  let _=out.print(output,f"{value}")
  Ok(())
}
"#;
pub(super) const WATCH: &str = r#"
pub fn main(t:Test) -> Result[(),FileError] {
  let d=test.dir(t,[])
  let mut watch=file.watch(d,path.of(""),true)?
  test.dir_event(t,d,Renamed(path.of("before"),path.of("after")))
  match watcher.next(d,mut watch) {
    Ok(Renamed(a,b))=>test.check(t,path.text(a)=="before" && path.text(b)=="after"),
    _=>test.check(t,false),
  }
  Ok(())
}
"#;
#[test]
fn ordered_reads_prove_mutable_storage_and_both_emitters_import_typed_helpers() {
    with_compiler_stack(|| {
        for (source, kind, helper) in [
            (
                STDIN,
                RuntimeOpKind::OrderedStdinRead,
                "hirsel_rt_v2_ordered_stdin_read",
            ),
            (
                WATCH,
                RuntimeOpKind::OrderedWatchNext,
                "hirsel_rt_v2_ordered_watcher_next",
            ),
        ] {
            let program = super::runtime50_continuation::ready(source);
            let plans = hirsel_driver::native::continuation::plans(&program);
            hirsel_driver::native::continuation::validate(&program, &plans).unwrap();
            let units: Vec<_> = program.units.iter().collect();
            let check = |p: &hirsel_lir::continuation::ContinuationPlans| {
                hirsel_lir::continuation::validate_runtime_operations(
                    &units,
                    &program.table,
                    &program.target,
                    p,
                )
            };
            check(&plans).unwrap();
            let (&id, plan) = plans
                .plans
                .iter()
                .find(|(_, p)| {
                    p.states
                        .iter()
                        .any(|s| s.runtime.as_ref().is_some_and(|r| r.kind == kind))
                })
                .unwrap_or_else(|| {
                    let refused: Vec<_> = program
                        .units
                        .iter()
                        .filter_map(|u| {
                            let u = u.unit();
                            u.tables
                                .calls
                                .iter()
                                .any(|c| match c.callee {
                                    hirsel_lir::ir::Callee::Operation(op) => u.tables.operations
                                        [op.index()]
                                    .definition
                                    .is_core(if kind == RuntimeOpKind::OrderedStdinRead {
                                        "stdin.read"
                                    } else {
                                        "watcher.next"
                                    }),
                                    _ => false,
                                })
                                .then(|| (&u.header.definition, plans.get(u.header.instance)))
                        })
                        .collect();
                    panic!("no retained {kind:?}: {refused:#?}")
                });
            assert_eq!(plan.representation, Representation::Resumable);
            for field in 0..16 {
                let mut bad = plans.clone();
                let state = bad
                    .plans
                    .get_mut(&id)
                    .unwrap()
                    .states
                    .iter_mut()
                    .find(|s| s.runtime.as_ref().is_some_and(|r| r.kind == kind))
                    .unwrap();
                let runtime = state.runtime.as_mut().unwrap();
                match field {
                    0 => runtime.helper = "hirsel_rt_v2_stdin_read".into(),
                    1 => runtime.kind = RuntimeOpKind::OrderedPrint,
                    2 => runtime.results[0].offset = 0,
                    3 => runtime.results[0].bytes += 1,
                    4 => runtime.results[0].align *= 2,
                    5 => runtime.results[1].offset = runtime.results[0].offset,
                    6 => runtime.results[1].bytes += 1,
                    7 => runtime.results[1].align *= 2,
                    8 => {
                        if let RuntimeResultSource::ArgumentSlot { parameter, .. } =
                            &mut runtime.results[1].source
                        {
                            *parameter = 0
                        }
                    }
                    9 => {
                        if let RuntimeResultSource::ArgumentSlot { initial, .. } =
                            &mut runtime.results[1].source
                        {
                            *initial = hirsel_lir::ir::ValueId(u32::MAX)
                        }
                    }
                    10 => {
                        if let RuntimeResultSource::ArgumentSlot { loan, .. } =
                            &mut runtime.results[1].source
                        {
                            *loan = hirsel_lir::ir::LoanId(u32::MAX)
                        }
                    }
                    11 => {
                        if let RuntimeResultSource::ArgumentSlot { address, .. } =
                            &mut runtime.results[1].source
                        {
                            *address = hirsel_lir::ir::AddressId(u32::MAX)
                        }
                    }
                    12 => {
                        if let RuntimeResultSource::ArgumentSlot { slot, .. } =
                            &mut runtime.results[1].source
                        {
                            *slot = hirsel_lir::ir::SlotId(u32::MAX)
                        }
                    }
                    13 => runtime.results.clear(),
                    14 => runtime.results.push(runtime.results[0].clone()),
                    _ => state.action = ResumeAction::RetryUnenteredCall(state.boundary.call),
                }
                assert!(
                    check(&bad).is_err(),
                    "accepted forged {kind:?} field {field}"
                );
            }
            for backend in [
                hirsel_driver::native::Backend::Llvm,
                hirsel_driver::native::Backend::Cranelift,
            ] {
                let objects = hirsel_driver::native::emit_backend_objects(
                    &program,
                    backend,
                    hirsel_driver::native::OptLevel::Default,
                    hirsel_driver::native::Partition::Whole,
                    false,
                    None,
                )
                .unwrap();
                assert!(
                    objects
                        .iter()
                        .flat_map(|o| &o.imports)
                        .any(|s| s.name == helper),
                    "{backend:?} omitted {helper}"
                );
            }
        }
    });
}
#[test]
fn ordered_stdin_and_join_agree_with_checked_h_and_replay() {
    with_compiler_stack(|| {
        super::runtime50_resume::agree_on(
            STDIN,
            &hirsel_driver::native::Fixture {
                stdin: b"G\0\xff".to_vec(),
                migrate: true,
                ..Default::default()
            },
            "retained stdin and join",
            true,
            &[false],
        )
    });
}

#[test]
fn ordered_watch_event_and_owning_cleanup_agree_with_checked_h_and_replay() {
    with_compiler_stack(|| {
        super::runtime50_resume::agree_on(
            WATCH,
            &Default::default(),
            "retained watch rename",
            true,
            &[false],
        )
    });
}
