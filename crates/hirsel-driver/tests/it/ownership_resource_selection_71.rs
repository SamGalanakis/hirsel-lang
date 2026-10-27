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
