//! A for loop over a direct List of a dynamic Packed value (MEM-13) walks it in
//! linear time natively: the flat walk carries each element's end as the next
//! one's offset, so optimized IR holds one loop and no walk from the first
//! element.
use hirsel_driver::compiler_adapters;
use hirsel_driver::native::{OptLevel, Options, ReadyProgram, TargetSpec, build, emit_ir};
use hirsel_engine::{
    QueryEngine, check_progress, default_context, default_environment, with_compiler_stack,
};
use std::collections::BTreeMap;

/// `main` pushes `n` alternating elements, then checks `total`'s sum of them.
fn source(n: u64) -> String {
    let expected: u64 = (0..n).map(|i| if i % 2 == 0 { i } else { 3 * i }).sum();
    format!(
        "type Choice = Small(value: u64) | Large(value: u64, extra: u64)

fn total(xs: List[Packed[Choice]]) -> u64 {{
  let mut sum = 0u64
  let mut k = 0u64
  for x in xs {{
    if k % 2u64 == 0u64 {{
      sum = sum + x.Small.value
    }} else {{
      sum = sum + x.Large.value * x.Large.extra
    }}
    k = k + 1u64
  }}
  sum
}}

pub fn main(rt: Io, args: Args) -> Result[(), ()] {{
  let mut xs: List[Packed[Choice]] = []
  for i in 0u64..{n}u64 {{
    if i % 2u64 == 0u64 {{
      list.push(mut xs, Small(value: i))
    }} else {{
      list.push(mut xs, Large(value: i, extra: 3u64))
    }}
  }}
  failure.check(total(xs) == {expected}u64, TextInvariant())
  Ok(())
}}
"
    )
}

fn ready(source: &str, options: &Options) -> ReadyProgram {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine
        .sources
        .add("packed_walk.hir", source.into())
        .unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let input = engine.export_native_input().unwrap();
    let validated = engine.import_native_input(input).unwrap();
    build(&validated, options).unwrap()
}

/// The optimized IR of `total`.
fn total_ir(flat_walk: bool, target: TargetSpec) -> String {
    with_compiler_stack(move || {
        let mut options = Options::new(target);
        options.flat_walk = flat_walk;
        let ir = emit_ir(&ready(&source(16), &options), OptLevel::Speed).unwrap();
        let defines: Vec<&str> = ir.lines().filter(|l| l.starts_with("define")).collect();
        let start = ir
            .lines()
            .position(|l| l.starts_with("define") && l.contains("5totalI") && !l.contains("@_H1T"))
            .unwrap_or_else(|| panic!("total is defined: {defines:#?}"));
        let lines: Vec<&str> = ir.lines().skip(start).collect();
        let end = lines.iter().position(|l| *l == "}").unwrap();
        lines[..=end].join("\n")
    })
}

/// The loops of one function's IR text: its retreating edges in a depth-first
/// walk from the entry block.
fn loops(function: &str) -> usize {
    let mut successors: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut order = vec!["<entry>".to_owned()];
    let mut name = "<entry>".to_owned();
    for raw in function.lines().skip(1) {
        let line = raw.trim();
        if line.is_empty() || line == "}" {
            continue;
        }
        if !raw.starts_with(' ')
            && let Some((label, _)) = line.split_once(':')
        {
            name = label.to_owned();
            order.push(name.clone());
            continue;
        }
        if line.contains(" = phi ") {
            continue;
        }
        let mut rest = line;
        while let Some(at) = rest.find("label %") {
            rest = &rest[at + "label %".len()..];
            let end = rest
                .find(|c: char| !(c.is_alphanumeric() || "._-$".contains(c)))
                .unwrap_or(rest.len());
            successors
                .entry(name.clone())
                .or_default()
                .push(rest[..end].to_owned());
        }
    }
    // 0 unvisited, 1 on the stack, 2 done.
    let mut state: BTreeMap<&str, u8> = BTreeMap::new();
    let mut retreating = 0;
    let mut stack = vec![(order[0].as_str(), 0usize)];
    state.insert(order[0].as_str(), 1);
    while let Some((block, next)) = stack.pop() {
        let outs = successors.get(block).map(Vec::as_slice).unwrap_or(&[]);
        if let Some(to) = outs.get(next) {
            stack.push((block, next + 1));
            match state.get(to.as_str()).copied().unwrap_or(0) {
                0 => {
                    state.insert(to.as_str(), 1);
                    stack.push((to.as_str(), 0));
                }
                1 => retreating += 1,
                _ => {}
            }
        } else {
            state.insert(block, 2);
        }
    }
    retreating
}

#[test]
fn optimized_ir_for_the_loop_holds_no_walk_from_the_first_element() {
    for target in [
        TargetSpec::x86_64_unknown_linux_gnu(),
        TargetSpec::aarch64_unknown_linux_gnu(),
    ] {
        let walked = total_ir(true, target.clone());
        assert_eq!(loops(&walked), 1, "{walked}");
        // Control: without the flat walk, each element is reached by an inner
        // loop from the first, which this count sees.
        let indexed = total_ir(false, target);
        assert!(loops(&indexed) >= 2, "{indexed}");
    }
}
