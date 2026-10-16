pub fn program(kind: &str, height: usize) -> String {
    let chain = |part: &str| format!("a{}", part.repeat(height - 2));
    let nested = |open: &str, close: &str| format!("{}a{}", open.repeat(height - 2), close.repeat(height - 2));
    let expression = match kind {
        "binary" => std::iter::repeat_n("a", height - 1).collect::<Vec<_>>().join(" + "),
        "call" => chain("()"),
        "index" => chain("[0u8]"),
        "field" => format!("(a){}", ".b".repeat(height - 3)),
        "try" => chain("?"),
        "unary" => format!("{}a", "! ".repeat(height - 2)),
        "group" => nested("(", ")"),
        "block" => nested("{", "}"),
        "tuple" => nested("(", ",)"),
        "sequence" => nested("[", "]"),
        "match" => nested("match a { _ => ", "}"),
        "else-if" => format!("{}{{ a }}", "if true {} else ".repeat(height - 3)),
        "closure" | "if" | "while" | "for" | "local-fn" => {
            let open = match kind {
                "closure" => "fn() {", "if" => "if true {", "while" => "while true {",
                "for" => "for x in a {", "local-fn" => "fn inner() {", _ => unreachable!(),
            };
            let count = (height - 2) / 2;
            format!("{}{}a{}", open.repeat(count), if height % 2 == 1 { "! " } else { "" }, "}".repeat(count))
        }
        "type-box" => return format!("type T = {}u8", "box ".repeat(height - 1)),
        "type-function" => return format!("type T = {}u8", "fn() -> ".repeat(height - 1)),
        "type-argument" => return format!("type T = {}u8{}", "A[".repeat(height - 1), "]".repeat(height - 1)),
        "pattern-tuple" => return format!("fn f() {{ let {}x{} = a }}", "(".repeat(height - 3), ",)".repeat(height - 3)),
        "pattern-constructor" => return format!("fn f() {{ let {}x{} = a }}", "C(".repeat(height - 3), ")".repeat(height - 3)),
        _ => unreachable!(),
    };
    format!("fn f() {{ {expression} }}")
}

