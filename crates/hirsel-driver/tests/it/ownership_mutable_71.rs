//! Mutable calls retain the caller's exact resource storage across writeback.
#[test]
fn mutable_resource_exchange_preserves_two_equal_tokens_on_four_engines() {
    super::ownership_owned_call_49::source_calls(
        r#"
resource type Token {}
drop(x) {}
fn exchange(mut slot:Token,take incoming:Token)->Token {
 replace(mut slot,take incoming)
}
pub fn main()->Result[(),u64] {
 let mut current=Token()
 let previous=exchange(mut current,Token())
 Ok(())
}
"#,
        2,
    );
}

#[test]
fn mutable_resource_projection_and_forwarding_keep_the_callers_prefix() {
    super::ownership_owned_call_49::source_calls(
        r#"
resource type Token {}
drop(x) {}
type Pair {left:Token,right:Token}
fn exchange(mut slot:Token,take incoming:Token)->Token {
 replace(mut slot,take incoming)
}
fn forward(mut slot:Token,take incoming:Token)->Token {
 exchange(mut slot,take incoming)
}
pub fn main()->Result[(),u64] {
 let mut pair=Pair(left:Token(),right:Token())
 let previous=forward(mut pair.left,Token())
 Ok(())
}
"#,
        3,
    );
}

#[test]
fn mutable_resource_failure_after_exchange_retires_displaced_and_written_values() {
    super::ownership_owned_call_49::source_calls_exit(
        r#"
resource type Token {}
drop(x) {}
fn exchange(mut slot:Token,take incoming:Token) {
 let previous=replace(mut slot,take incoming)
 let zero:u8=0u8
 let _=1u8/zero
}
pub fn main()->Result[(),u64] {
 let mut current=Token()
 exchange(mut current,Token())
 Ok(())
}
"#,
        2,
        hirsel_run_schema::exit::FAILED,
    );
}
