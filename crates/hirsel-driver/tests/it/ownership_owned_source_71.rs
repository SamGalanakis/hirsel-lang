//! Explicit owned selection follows the same admitted source on all engines.
#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn source_owned_string_and_byte_leaves_keep_their_original_lifetimes() {
    for literal in ["\"equal\"", "b\"equal\""] {
        super::ownership_owned_call_49::source_owned_leaves(
            &format!(
                "pub fn main()->Result[(),()] {{ let first={literal}\nlet second={literal}\nlet moved=take first\nOk(()) }}"
            ),
            2,
        );
    }
}
