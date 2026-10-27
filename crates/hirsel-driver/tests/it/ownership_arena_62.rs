//! Original resource lineage survives compiler-authored arena scope controls.
#[test]
fn native_arena_scope_preserves_outer_resource_on_four_engines() {
    super::ownership_owned_call_49::source_calls(
        "resource type Token { value:u64 }\ndrop(x) {}\nfn body(a:Arena)->u64{7u64}\npub fn main()->Result[(),u64]{\nlet first=Token(value:7u64)\nlet n=arena.with(body)\nlet second=take first\nOk(())\n}\n",
        1,
    );
}
