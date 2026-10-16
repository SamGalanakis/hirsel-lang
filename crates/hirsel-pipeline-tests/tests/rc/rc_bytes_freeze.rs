//! Typed byte freezing transfers the owner without a runtime dispatch.

#[test]
fn empty_static_shared_and_grown_lists_keep_their_bytes_and_credits() {
    for (body, expected) in [
        (
            "let xs: List[u8] = []\n let bs = bytes.from_list(take xs)\n bytes.len(bs)",
            0,
        ),
        (
            "let xs: List[u8] = [255u8, 0u8]\n let bs = bytes.from_list(take xs)\n bytes.len(bs) + if bs[0] == 255u8 && bs[1] == 0u8 { 255u64 } else { 0u64 }",
            257,
        ),
        (
            "let mut xs: List[u8] = [65u8, 66u8]\n let bs = bytes.from_list(copy xs)\n list.push(mut xs, 67u8)\n bytes.len(bs) + if bs[0] == 65u8 && bs[1] == 66u8 { 66u64 } else { 0u64 } + list.len(xs)",
            71,
        ),
        (
            "let mut xs: List[u8] = []\n list.reserve(mut xs, 9)\n list.push(mut xs, 255u8)\n list.push(mut xs, 0u8)\n let bs = bytes.from_list(take xs)\n bytes.len(bs) + if bs[0] == 255u8 && bs[1] == 0u8 { 255u64 } else { 0u64 }",
            257,
        ),
    ] {
        super::rc_flat_native::audited(&format!("fn main() -> u64 {{ {body} }}"), expected);
    }
}
