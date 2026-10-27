//! Original parent and child identities survive non-leaf Resource cleanup.
#[test]
fn nested_resource_cleanup_preserves_parent_and_equal_children_on_four_engines() {
    super::ownership_owned_call_49::source_calls(
        "resource type Token { value:u64 }\ndrop(x) {}\nresource type Pair { first:Token, second:Token }\ndrop(x) {}\npub fn main()->Result[(),u64]{\nlet pair=Pair(first:Token(value:7u64),second:Token(value:7u64))\nOk(())\n}\n",
        3,
    );
}

#[test]
fn deeper_resource_cleanup_preserves_each_containing_resource_on_four_engines() {
    super::ownership_owned_call_49::source_calls(
        "resource type Token { value:u64 }\ndrop(x) {}\nresource type Pair { first:Token, second:Token }\ndrop(x) {}\nresource type Outer { pair:Pair, last:Token }\ndrop(x) {}\npub fn main()->Result[(),u64]{\nlet outer=Outer(pair:Pair(first:Token(value:7u64),second:Token(value:7u64)),last:Token(value:7u64))\nOk(())\n}\n",
        5,
    );
}

#[test]
fn failed_parent_finalizer_retires_its_equal_children_on_four_engines() {
    super::ownership_owned_call_49::source_calls_exit(
        "resource type Audit { value:u64 }\ndrop(x) {}\nresource type Token { value:u64 }\ndrop(x) {}\nresource type Pair { first:Token, second:Token }\ndrop(x) {\nlet audit=Audit(value:7u64)\nlet zero:u8=0u8\nlet _=1u8/zero\n}\npub fn main()->Result[(),u64]{\nlet pair=Pair(first:Token(value:7u64),second:Token(value:7u64))\nOk(())\n}\n",
        4,
        hirsel_run_schema::exit::FAILED,
    );
}
