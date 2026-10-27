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
