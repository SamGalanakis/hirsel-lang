//! Native queue destruction follows checked descriptor completions, including
//! FIFO heads and equal zero-sized Resource fields in the same stored image.
use hirsel_run_schema::ownership_trace::native::Row;

fn queued(source: &str, sent: u64, items: &[u64], resources_per_item: usize) {
    for policy in ["tasks.cooperative@1", "tasks.free@1"] {
        super::ownership_owned_call_49::source_calls_granted_with_log(
            source,
            2,
            0,
            &serde_json::json!({"io":{"tasks":policy,"dirs":[]}}).to_string(),
            |log| {
                let created = log
                    .rows
                    .iter()
                    .filter_map(|row| match row {
                        Row::QueueCreated { queue, layout, .. } => Some((*queue, *layout)),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                assert_eq!(created.len(), 1);
                let (queue, layout) = created[0];
                let detached = log
                    .rows
                    .iter()
                    .filter_map(|row| match row {
                        Row::QueueDetached {
                            queue: actual,
                            first,
                            end,
                            ..
                        } if *actual == queue => Some((*first, *end)),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                assert_eq!(detached, [(items[0], sent)]);
                let begin = log
                    .rows
                    .iter()
                    .filter_map(|row| match row {
                        Row::QueueCleanupBegin {
                            queue: actual,
                            layout: payload,
                            item,
                            ..
                        } => {
                            assert_eq!((*actual, *payload), (queue, layout));
                            Some(*item)
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                assert_eq!(begin, items);
                let dropped = log
                    .rows
                    .iter()
                    .filter_map(|row| match row {
                        Row::QueueCleanupDrop {
                            queue: actual,
                            item,
                            ..
                        } => {
                            assert_eq!(*actual, queue);
                            Some(*item)
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                let expected = items
                    .iter()
                    .flat_map(|item| std::iter::repeat_n(*item, resources_per_item))
                    .collect::<Vec<_>>();
                assert_eq!(dropped, expected);
                let end = log
                    .rows
                    .iter()
                    .filter_map(|row| match row {
                        Row::QueueCleanupEnd {
                            queue: actual,
                            item,
                            ..
                        } => {
                            assert_eq!(*actual, queue);
                            Some(*item)
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                assert_eq!(end, items);
            },
        );
    }
}

#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn receiver_close_retires_two_equal_queued_resources_on_four_engines() {
    queued(
        r#"
resource type Token {}
drop(x) {}
pub fn main(rt:Io)->Result[(),GrantError] {
 let tasks=io.tasks(rt)?
 let (tx,rx):(Sender[Token],Receiver[Token])=channel.open(tasks)
 let mut sender=take tx
 let first=Token()
 let second=Token()
 let _=channel.send(tasks,mut sender,take first)
 let _=channel.send(tasks,mut sender,take second)
 release(take rx)
 Ok(())
}
"#,
        2,
        &[0, 1],
        1,
    );
}

#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn receiver_close_preserves_the_remaining_fifo_item_after_a_resource_receive() {
    queued(
        r#"
resource type Token {}
drop(x) {}
pub fn main(rt:Io)->Result[(),GrantError] {
 let tasks=io.tasks(rt)?
 let (tx,rx):(Sender[Token],Receiver[Token])=channel.open(tasks)
 let mut sender=take tx
 let mut receiver=take rx
 let first=Token()
 let second=Token()
 let _=channel.send(tasks,mut sender,take first)
 let _=channel.send(tasks,mut sender,take second)
 let received=channel.receive(tasks,mut receiver)
 let Ok(token)=take received else { failure.check(false,Bounds())
 return Ok(()) }
 release(take token)
 release(take receiver)
 Ok(())
}
"#,
        2,
        &[1],
        1,
    );
}

#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn receiver_close_distinguishes_equal_resource_fields_at_the_same_physical_offset() {
    queued(
        r#"
resource type Token {}
drop(x) {}
pub fn main(rt:Io)->Result[(),GrantError] {
 let tasks=io.tasks(rt)?
 let (tx,rx):(Sender[(Token,Token)],Receiver[(Token,Token)])=channel.open(tasks)
 let mut sender=take tx
 let first=Token()
 let second=Token()
 let payload=(take first,take second)
 let _=channel.send(tasks,mut sender,take payload)
 release(take rx)
 Ok(())
}
"#,
        1,
        &[0],
        2,
    );
}
