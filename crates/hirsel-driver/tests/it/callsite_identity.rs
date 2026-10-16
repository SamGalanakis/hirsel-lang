//! Call-site and symbol identity exclude the callee's body revision: editing
//! one callee's body leaves every caller's emitted calls and every link name
//! byte-identical, for a plain and a generic callee alike.
use hirsel_driver::compiler_adapters;
use hirsel_driver::native::{OptLevel, Options, TargetSpec, build, emit_ir};
use hirsel_engine::{
    QueryEngine, check_progress, default_context, default_environment, with_compiler_stack,
};

/// The callees come last, so an edit to their bodies moves no caller source position.
fn program(helper: &str, pass: &str) -> String {
    format!(
        "fn caller(n: u64) -> u64 {{ helper(n) + pass(n) }}
pub fn main(rt: Io, args: Args) -> Result[(), ()] {{
  failure.check(caller(1u64) > 0u64, TextInvariant())
  Ok(())
}}
fn helper(x: u64) -> u64 {{ {helper} }}
fn pass[T](take x: T) -> T {{ {pass} }}
"
    )
}

/// The unoptimized IR of `source`.
fn ir(source: String) -> String {
    with_compiler_stack(move || {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("identity.hir", source).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        let ready = build(&validated, &options).unwrap();
        emit_ir(&ready, OptLevel::None).unwrap()
    })
}

/// The definition of the one function whose link name holds `name` as a segment.
fn function<'a>(ir: &'a str, name: &str) -> &'a str {
    let segment = format!("{}{name}", name.len());
    let mut defines = ir
        .match_indices("\ndefine ")
        .map(|(at, _)| &ir[at + 1..])
        .filter(|rest| {
            let line = rest.lines().next().unwrap();
            line.contains(&segment) && !line.contains("@_H1T")
        });
    let start = defines
        .next()
        .unwrap_or_else(|| panic!("{name} is defined\n{ir}"));
    assert!(defines.next().is_none(), "one definition of {name}\n{ir}");
    &start[..start.find("\n}\n").unwrap() + 2]
}

/// The link name a `define` line gives its function.
fn link_name(function: &str) -> &str {
    let at = function.find("@_H1").unwrap();
    let rest = &function[at + 1..];
    &rest[..rest.find('(').unwrap()]
}

/// Every link name the module defines or declares.
fn link_names(ir: &str) -> Vec<&str> {
    let mut names: Vec<&str> = ir
        .lines()
        .filter(|l| l.starts_with("define ") || l.starts_with("declare "))
        .filter_map(|l| {
            let at = l.find("@_H1")?;
            let rest = &l[at + 1..];
            Some(&rest[..rest.find('(')?])
        })
        .collect();
    names.sort_unstable();
    names
}

#[test]
fn callee_body_edits_leave_caller_calls_and_symbols_unchanged() {
    let before = ir(program("x + 1u64", "x"));
    let after = ir(program("x * 3u64 + 7u64", "let y = x\n  y"));
    for callee in ["helper", "pass"] {
        assert_ne!(
            function(&before, callee),
            function(&after, callee),
            "the edit changes {callee}'s body"
        );
        assert_eq!(
            link_name(function(&before, callee)),
            link_name(function(&after, callee)),
            "{callee}'s symbol"
        );
    }
    let caller = function(&before, "caller");
    for callee in ["helper", "pass"] {
        let symbol = link_name(function(&before, callee));
        assert!(
            caller.contains(&format!("@{symbol}(")),
            "caller calls {symbol}\n{caller}"
        );
    }
    assert_eq!(caller, function(&after, "caller"), "the caller's code");
    assert_eq!(link_names(&before), link_names(&after), "every link name");
}
