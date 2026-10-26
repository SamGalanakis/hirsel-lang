//! A retained owning await result remains initialized while the child's rest
//! yields twice and contributes two cleanup failures.
use hirsel_engine::with_compiler_stack;

pub(super) const SOURCE: &str = r#"
resource type Guard { output:Stdout, text:String }
drop(x) {
  let _=out.print(x.output,x.text)
  let zero:u8=0u8
  let _=1u8/zero
}
pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let output=io.stdout(rt)?
  let value=task.scope(tasks,fn(s:Scope)->u64 {
    let joining=task.async(s,fn()->u64 {
      let owned=task.scope(tasks,fn(inner:Scope)->String {
        let child=task.async(inner,fn()->String {
          let first=Guard(output:output,text:"first\n")
          let second=Guard(output:output,text:"second\n")
          "held"
        })
        task.await(inner,take child)
      })
      string.byte_len(owned)
    })
    task.await(s,take joining)
  })
  let _=out.print(output,f"unexpected {value}")
  Ok(())
}
"#;

#[test]
fn owning_await_result_survives_yielding_child_cleanup_and_two_failures() {
    with_compiler_stack(|| {
        let program = super::runtime50_continuation::ready(SOURCE);
        let plans = hirsel_driver::native::continuation::plans(&program);
        hirsel_driver::native::continuation::validate(&program, &plans).unwrap();
        assert!(
            program.units.iter().any(|unit| {
                unit.unit().header.definition.is_core("task.await")
                    && plans.get(unit.unit().header.instance).is_some_and(|p| {
                        p.states.iter().any(|s| {
                            s.runtime.as_ref().is_some_and(|r| {
                                r.kind == hirsel_lir::continuation::RuntimeOpKind::OrderedJoin
                            })
                        })
                    })
            }),
            "owning await did not expose its entered join"
        );
        let observed = super::runtime50_resume::agree_on(
            SOURCE,
            &hirsel_driver::native::Fixture {
                migrate: true,
                ..Default::default()
            },
            "owning await with yielding failed cleanup",
            true,
            &[false],
        );
        assert_eq!(observed.output, b"second\nfirst\n");
        assert!(observed.exit_code.is_none());
        assert_eq!(observed.failures.len(), 2, "{observed:?}");
    });
}
