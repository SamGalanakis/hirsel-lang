//! Await moves existing child result credits into the actual receiving parent.
#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn awaiting_distinct_children_preserves_two_equal_resource_results_on_four_engines() {
    for policy in ["tasks.cooperative@1", "tasks.free@1"] {
        super::ownership_owned_call_49::source_calls_granted(
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
            2,
            0,
            &serde_json::json!({"io":{"tasks":policy,"dirs":[]}}).to_string(),
        );
    }
}
