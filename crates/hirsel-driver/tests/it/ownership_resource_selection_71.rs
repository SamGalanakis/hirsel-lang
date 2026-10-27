//! Source resource diagnostics preserve ordinary owned data alongside Resources.
#[test]
fn source_resource_selection_keeps_owned_strings_out_of_resource_lineage() {
    super::ownership_owned_call_49::source_calls(
        r#"
resource type Token {}
drop(x) {}
fn echo(take text:String)->String { take text }
pub fn main()->Result[(),u64] {
 let first=Token()
 let text=echo("ordinary owned data")
 let second=Token()
 let moved=take text
 Ok(())
}
"#,
        2,
    );
}

#[test]
fn source_resource_selection_keeps_scalar_loop_replacement_out_of_resource_lineage() {
    super::ownership_owned_call_49::source_calls(
        r#"
resource type Token {}
drop(x) {}
pub fn main()->Result[(),()] {
 let mut i=0u64
 while i<8u64 {
  let token=Token()
  i=i+1u64
 }
 Ok(())
}
"#,
        8,
    );
}

#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn source_resource_selection_keeps_owned_data_fields_inside_each_resource_lifetime() {
    super::ownership_owned_call_49::source_calls(
        r#"
resource type Token { data:String }
drop(x) {}
pub fn main()->Result[(),u64] {
 let first=Token(data:"equal owned data")
 let second=Token(data:"equal owned data")
 let moved=take first
 Ok(())
}
"#,
        2,
    );
}

#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn source_resource_selection_proves_mutex_data_and_owned_task_results_have_no_resource_credit() {
    super::ownership_owned_call_49::source_calls_granted(
        r#"
resource type Token {}
drop(x) {}
pub fn main(rt:Io)->Result[(),GrantError] {
 let tasks=io.tasks(rt)?
 let board=mutex.create(tasks,unique 7u64)
 task.scope(tasks,fn(s:Scope) {
  let child=task.async(s,fn()->String {
   let token=Token()
   let value=mutex.with(tasks,board,fn(mut n:unique u64)->u64 uses {} {n})
   failure.check(value==7u64,Bounds())
   "ordinary owned result"
  })
  let received=task.await(s,take child)
 })
 Ok(())
}
"#,
        1,
        0,
        r#"{"io":{"tasks":"tasks.cooperative@1","dirs":[]}}"#,
    );
}

#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn source_resource_selection_keeps_a_resource_across_a_named_data_callback() {
    super::ownership_owned_call_49::source_calls_granted(
        r#"
resource type Token {}
drop(x) {}
fn read(mut n:unique u64)->u64 uses {} {n}
pub fn main(rt:Io)->Result[(),GrantError] {
 let tasks=io.tasks(rt)?
 let board=mutex.create(tasks,unique 7u64)
 let token=Token()
 let value=mutex.with(tasks,board,read)
 failure.check(value==7u64,Bounds())
 Ok(())
}
"#,
        1,
        0,
        r#"{"io":{"tasks":"tasks.cooperative@1","dirs":[]}}"#,
    );
}
