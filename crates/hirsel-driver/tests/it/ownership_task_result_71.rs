//! Actual result copies and discards preserve existing child Resource credits.
use hirsel_run_schema::ownership_trace::native::Row;

fn results(source: &str, exit: i32, expected: [usize; 5]) {
    for policy in ["tasks.cooperative@1", "tasks.free@1"] {
        super::ownership_owned_call_49::source_calls_granted_with_log(
            source,
            2,
            exit,
            &serde_json::json!({"io":{"tasks":policy,"dirs":[]}}).to_string(),
            |log| {
                let mut counts = [0; 5];
                for row in &log.rows {
                    match row {
                        Row::ObservationLost => panic!("result producer lost its checked binding"),
                        Row::TaskResultHeld { task, layout, .. } => {
                            assert_ne!(*task, 0);
                            assert_ne!(*layout, u32::MAX);
                            counts[0] += 1;
                        }
                        Row::TaskResultJoin { task, child, .. } => {
                            assert_eq!(*task, 0);
                            assert_ne!(*child, 0);
                            counts[1] += 1;
                        }
                        Row::TaskResultMoved { task, child, .. } => {
                            assert_eq!(*task, 0);
                            assert_ne!(*child, 0);
                            counts[2] += 1;
                        }
                        Row::TaskResultDiscarded { task, .. } => {
                            assert_ne!(*task, 0);
                            counts[3] += 1;
                        }
                        Row::TaskResultUnavailable { task, status, .. } => {
                            assert_eq!(*task, 0);
                            assert_eq!(*status, 1);
                            counts[4] += 1;
                        }
                        _ => (),
                    }
                }
                assert_eq!(counts, expected);
            },
        );
    }
}
#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn awaiting_distinct_children_preserves_two_equal_resource_results_on_four_engines() {
    results(
        r#"
resource type Token {}
drop(x) {}
pub fn main(rt:Io)->Result[(),GrantError] {
 let tasks=io.tasks(rt)?
 task.scope(tasks,fn(s:Scope) {
  let first=task.async(s,fn()->Token {Token()})
  let a=task.await(s,take first)
  let second=task.async(s,fn()->Token {Token()})
  let b=task.await(s,take second)
 })
 Ok(())
}
"#,
        0,
        [2, 2, 2, 0, 0],
    );
}
#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn awaiting_one_child_transfers_equal_resource_fields_at_the_same_physical_offset() {
    results(
        r#"
resource type Token {}
drop(x) {}
pub fn main(rt:Io)->Result[(),GrantError] {
 let tasks=io.tasks(rt)?
 task.scope(tasks,fn(s:Scope) {
  let child=task.async(s,fn()->(Token,Token) {(Token(),Token())})
  let pair=task.await(s,take child)
 })
 Ok(())
}
"#,
        0,
        [1, 1, 1, 0, 0],
    );
}
#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn scope_exit_discards_unjoined_resource_results_in_the_actual_child() {
    results(
        r#"
resource type Token {}
drop(x) {}
pub fn main(rt:Io)->Result[(),GrantError] {
 let tasks=io.tasks(rt)?
 task.scope(tasks,fn(s:Scope) {
  let child=task.async(s,fn()->(Token,Token) {(Token(),Token())})
 })
 Ok(())
}
"#,
        0,
        [1, 0, 0, 1, 0],
    );
}
#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn failed_child_cleans_equal_resources_without_creating_a_parent_result_credit() {
    results(
        r#"
resource type Token {}
drop(x) {}
pub fn main(rt:Io)->Result[(),GrantError] {
 let tasks=io.tasks(rt)?
 task.scope(tasks,fn(s:Scope) {
  let child=task.async(s,fn()->Token {
   let first=Token()
   let second=Token()
   failure.check(false,Bounds())
   take first
  })
  let token=task.await(s,take child)
 })
 Ok(())
}
"#,
        hirsel_run_schema::exit::FAILED,
        [0, 1, 0, 0, 1],
    );
}
