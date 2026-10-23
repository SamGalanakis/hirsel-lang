//! Borrowed structural capture views must leave the parent's owner usable.
#[test]
fn recursive_capture_view_preserves_the_parent_owner_after_the_closure() {
    let source = r#"
type Expr = Literal(u64) | Nested(box Expr)
fn score(tree: box Expr) -> u64 {
  match tree { Literal(n) => n, Nested(child) => score(child) }
}
fn main() -> u64 {
  let owner = box Literal(7u64)
  let value = {
    let callback = fn() -> u64 { score(owner) }
    callback()
  }
  value + score(owner)
}
"#;
    assert_eq!(
        crate::check_support::bits(crate::check_support::execute_source(source)),
        14
    );
}
