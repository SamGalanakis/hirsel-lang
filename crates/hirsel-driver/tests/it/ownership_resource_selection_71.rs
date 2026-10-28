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

#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn source_resource_selection_preserves_the_active_and_empty_arms_of_owned_options() {
    super::ownership_owned_call_49::source_calls(
        r#"
resource type Token {}
drop(x) {}
fn choose(take token:Token,keep:bool)->Option[Token] {
 if keep { Some(take token) } else { None() }
}
pub fn main()->Result[(),u64] {
 let first=Token()
 let second=Token()
 let present=choose(take first,true)
 let absent=choose(take second,false)
 let Some(received)=take present else { return Error(1u64) }
 let None()=take absent else { return Error(2u64) }
 let moved=take received
 Ok(())
}
"#,
        2,
    );
}

#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn scalar_channel_traffic_preserves_parent_and_child_resource_lifetimes() {
    for tasks in ["tasks.cooperative@1", "tasks.free@1"] {
        super::ownership_owned_call_49::source_calls_granted_with_log(
            r#"
resource type Token {}
drop(x) {}
pub fn main(rt:Io)->Result[(),GrantError] {
 let tasks=io.tasks(rt)?
 let outside=Token()
 task.scope(tasks,fn(s:Scope) {
  let (tx,rx):(Sender[u64],Receiver[u64])=channel.open(tasks)
  let child=task.async(s,fn() {
   let token=Token()
   let mut receiver=take rx
   let first=channel.receive(tasks,mut receiver)
   let second=channel.receive(tasks,mut receiver)
   let Ok(a)=take first else { failure.check(false,Bounds())
return }
   let Ok(b)=take second else { failure.check(false,Bounds())
return }
   failure.check(a==7u64 && b==9u64,Bounds())
  })
  let mut sender=take tx
  let _=channel.send(tasks,mut sender,7u64)
  let _=channel.send(tasks,mut sender,9u64)
  let _=task.await(s,take child)
 })
 Ok(())
}
"#,
            2,
            0,
            &serde_json::json!({"io":{"tasks":tasks,"dirs":[]}}).to_string(),
            |log| {
                use hirsel_run_schema::ownership_trace::native::Row;
                let created = log
                    .rows
                    .iter()
                    .filter_map(|row| match row {
                        Row::QueueCreated { queue, layout, .. } => Some((*queue, *layout)),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                assert_eq!(created.len(), 1, "the actual channel creation is absent");
                let (queue, layout) = created[0];
                let sends = log
                    .rows
                    .iter()
                    .filter_map(|row| match row {
                        Row::QueueSent {
                            queue: q,
                            layout: l,
                            item,
                            status: 0,
                            ..
                        } if (*q, *l) == (queue, layout) => Some(*item),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                let received = log
                    .rows
                    .iter()
                    .filter_map(|row| match row {
                        Row::QueueReceived {
                            queue: q,
                            layout: l,
                            item,
                            status: 0,
                            ..
                        } if (*q, *l) == (queue, layout) => Some(*item),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                assert_eq!(sends, [0, 1]);
                assert_eq!(received, [0, 1]);
                assert_eq!(log.rows.iter().filter(|row| matches!(row, Row::QueueDetached { queue: q, layout: l, first: 2, end: 2, .. } if (*q,*l) == (queue,layout))).count(), 1);
            },
        );
    }
}

#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn empty_resource_channel_endpoints_do_not_own_their_potential_payloads() {
    for policy in ["tasks.cooperative@1", "tasks.free@1"] {
        super::ownership_owned_call_49::source_calls_granted(
            r#"
resource type Token {}
drop(x) {}
pub fn main(rt:Io)->Result[(),GrantError] {
 let tasks=io.tasks(rt)?
 let token=Token()
 let (tx,rx):(Sender[Token],Receiver[Token])=channel.open(tasks)
 Ok(())
}
"#,
            1,
            0,
            &serde_json::json!({"io":{"tasks":policy,"dirs":[]}}).to_string(),
        );
    }
}
