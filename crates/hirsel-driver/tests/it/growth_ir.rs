//! Growth loops in optimized LLVM IR: a private growth call hands its root and
//! must-unique fact back by value, so a push loop over a list it owns carries
//! must-unique as an SSA value and loads only its capacity word (runtime ABI 5),
//! while shared, escaping and over-reserved lists keep their tests.
use hirsel_driver::compiler_adapters;
use hirsel_driver::native::{OptLevel, Options, TargetSpec, build, emit_ir};
use hirsel_engine::{
    QueryEngine, check_progress, default_context, default_environment, with_compiler_stack,
};
use std::collections::{BTreeMap, BinaryHeap};

/// The optimized IR of `main` in `source`, compiled for `target`.
fn main_ir(source: &str, target: TargetSpec) -> String {
    let source = source.to_owned();
    with_compiler_stack(move || {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("growth.hir", source).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let ready = build(&validated, &Options::new(target)).unwrap();
        let ir = emit_ir(&ready, OptLevel::Speed).unwrap();
        let start = ir
            .lines()
            .position(|l| l.starts_with("define") && l.contains("4mainI") && !l.contains("@_H1T"))
            .expect("main is defined");
        let lines: Vec<&str> = ir.lines().skip(start).collect();
        let end = lines.iter().position(|l| *l == "}").unwrap();
        // Name each TBAA access tag by its type's name, so a capacity load is
        // `!tbaa !"sequence capacity"`.
        let metadata: BTreeMap<&str, &str> = ir
            .lines()
            .filter_map(|l| l.strip_prefix('!')?.split_once(" = "))
            .collect();
        let name = |tag: &str| {
            let first = metadata.get(tag)?.strip_prefix("!{!")?;
            let base = first.split(',').next()?;
            metadata.get(base)?.strip_prefix("!{!\"")?.split('"').next()
        };
        lines[..=end]
            .iter()
            .map(|l| match l.split_once("!tbaa !") {
                Some((head, tag)) => match name(tag.trim()) {
                    Some(name) => format!("{head}!tbaa !\"{name}\""),
                    None => (*l).to_owned(),
                },
                None => (*l).to_owned(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    })
}

/// One basic block: what it costs on a path and where it can go.
#[derive(Debug, Default)]
struct Block {
    lines: Vec<String>,
    successors: Vec<String>,
}

impl Block {
    fn count(&self, what: impl Fn(&str) -> bool) -> usize {
        self.lines.iter().filter(|l| what(l)).count()
    }
    fn conditional(&self) -> usize {
        self.count(|l| l.starts_with("br i1 ") || l.starts_with("switch "))
    }
    fn loads(&self) -> usize {
        self.count(|l| l.contains(" = load "))
    }
    /// Loads of a sequence's capacity word.
    fn capacity_loads(&self) -> usize {
        self.count(|l| l.contains(" = load ") && l.ends_with("!tbaa !\"sequence capacity\""))
    }
    /// Calls other than intrinsics, which emit no call.
    fn calls(&self) -> usize {
        self.count(|l| l.contains("call ") && !l.contains("@llvm."))
    }
    /// An element store of any width: a scalar store tagged as a value.
    fn stores_element(&self) -> bool {
        self.lines
            .iter()
            .any(|l| l.starts_with("store ") && l.ends_with("!tbaa !\"scalar value\""))
    }
    fn cost(&self) -> usize {
        self.conditional() + self.loads() + self.calls()
    }
}

/// The control-flow graph of one function's IR text.
fn blocks(function: &str) -> BTreeMap<String, Block> {
    let mut blocks = BTreeMap::new();
    let mut name = "<entry>".to_owned();
    let mut current = Block::default();
    for raw in function.lines().skip(1) {
        let line = raw.trim();
        if line.is_empty() || line == "}" {
            continue;
        }
        if !raw.starts_with(' ')
            && let Some((label, _)) = line.split_once(':')
        {
            blocks.insert(std::mem::replace(&mut name, label.to_owned()), current);
            current = Block::default();
            continue;
        }
        if !line.contains(" = phi ") {
            let mut rest = line;
            while let Some(at) = rest.find("label %") {
                rest = &rest[at + "label %".len()..];
                let end = rest
                    .find(|c: char| !(c.is_alphanumeric() || "._-$".contains(c)))
                    .unwrap_or(rest.len());
                current.successors.push(rest[..end].to_owned());
            }
        }
        current.lines.push(line.to_owned());
    }
    blocks.insert(name, current);
    blocks
}

/// The cheapest cycle through `start`, by the blocks' costs; `None` off every loop.
fn cycle(blocks: &BTreeMap<String, Block>, start: &str) -> Option<(usize, Vec<String>)> {
    let mut best: BTreeMap<&str, (usize, Vec<String>)> = BTreeMap::new();
    let mut queue = BinaryHeap::new();
    let first = blocks[start].cost();
    for next in &blocks[start].successors {
        queue.push(std::cmp::Reverse((
            first,
            next.as_str(),
            vec![start.to_owned()],
        )));
    }
    while let Some(std::cmp::Reverse((cost, at, path))) = queue.pop() {
        if at == start {
            return Some((cost, path));
        }
        if best.get(at).is_some_and(|(c, _)| *c <= cost) {
            continue;
        }
        best.insert(at, (cost, path.clone()));
        let block = &blocks[at];
        let mut path = path;
        path.push(at.to_owned());
        for next in &block.successors {
            queue.push(std::cmp::Reverse((
                cost + block.cost(),
                next.as_str(),
                path.clone(),
            )));
        }
    }
    None
}

/// The cheapest loop that stores a list element: what one steady iteration runs.
struct Hot<'a> {
    blocks: Vec<&'a Block>,
    path: Vec<String>,
}

impl Hot<'_> {
    fn conditional(&self) -> usize {
        self.blocks.iter().map(|b| b.conditional()).sum()
    }
    fn loads(&self) -> usize {
        self.blocks.iter().map(|b| b.loads()).sum()
    }
    fn capacity_loads(&self) -> usize {
        self.blocks.iter().map(|b| b.capacity_loads()).sum()
    }
    fn calls(&self) -> usize {
        self.blocks.iter().map(|b| b.calls()).sum()
    }
}

/// `None` when no loop stores an element: LLVM unrolled or vectorized the
/// pushes into straight-line stores.
fn spare_loop<'a>(blocks: &'a BTreeMap<String, Block>) -> Option<Hot<'a>> {
    let (_, path) = blocks
        .iter()
        .filter(|(_, b)| b.stores_element())
        .filter_map(|(name, _)| cycle(blocks, name))
        .min_by_key(|(cost, _)| *cost)?;
    Some(Hot {
        blocks: path.iter().map(|n| &blocks[n]).collect(),
        path,
    })
}

fn hot<'a>(blocks: &'a BTreeMap<String, Block>) -> Hot<'a> {
    spare_loop(blocks).expect("a loop stores an element")
}

fn targets() -> [TargetSpec; 2] {
    [
        TargetSpec::x86_64_unknown_linux_gnu(),
        TargetSpec::aarch64_unknown_linux_gnu(),
    ]
}

/// The steady iteration of `source`'s loop holds must-unique in a register: it
/// loads at most the capacity word (one load, runtime ABI 5) and calls nothing,
/// and its only tests are the capacity comparison and the loop's own exit. A
/// body with no element loop left (LLVM proved the capacity and stored the
/// elements straight-line, as the untagged root pointer allows) has no growth
/// call either.
fn assert_promoted(source: &str) {
    for target in targets() {
        let ir = main_ir(source, target);
        let blocks = blocks(&ir);
        let Some(hot) = spare_loop(&blocks) else {
            assert!(!ir.contains("list__grow"), "{ir}");
            continue;
        };
        let shown = || format!("hot path {:?}\n{ir}", hot.path);
        assert_eq!(hot.loads(), hot.capacity_loads(), "{}", shown());
        assert!(hot.capacity_loads() <= 1, "{}", shown());
        assert_eq!(hot.calls(), 0, "{}", shown());
        assert!(hot.conditional() <= 2, "{}", shown());
    }
}

const RESERVED: &str = "pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let mut rows: List[u64] = []
  list.reserve(mut rows, 128u64)
  for i in 0u64..128u64 { list.push(mut rows, i) }
  failure.check(list.len(rows) == 128u64, TextInvariant())
  Ok(())
}
";

#[test]
fn a_reserved_push_loop_carries_uniqueness_as_a_value() {
    assert_promoted(RESERVED);
}

#[test]
fn a_push_loop_after_a_growing_push_carries_uniqueness_as_a_value() {
    assert_promoted(
        "pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let mut rows: List[u64] = []
  list.push(mut rows, 0u64)
  for i in 1u64..128u64 { list.push(mut rows, i) }
  failure.check(list.len(rows) == 128u64, TextInvariant())
  Ok(())
}
",
    );
}

/// Whether any loop of `ir` stores a sequence's count word.
fn stores_a_count_in_a_loop(ir: &str) -> bool {
    let blocks = blocks(ir);
    blocks.iter().any(|(name, block)| {
        block.count(|l| l.starts_with("store ") && l.ends_with("!tbaa !\"sequence length\"")) > 0
            && cycle(&blocks, name).is_some()
    })
}

const TWO_LISTS: &str = "pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let mut xs: List[u64] = []
  let mut ys: List[u64] = []
  list.reserve(mut xs, 100000u64)
  list.reserve(mut ys, 100000u64)
  for i in 0u64..100000u64 {
    list.push(mut xs, i % 127u64)
    list.push(mut ys, i % 31u64)
  }
  failure.check(xs[99999u64] + ys[99999u64] == 99999u64 % 127u64 + 99999u64 % 31u64, TextInvariant())
  Ok(())
}
";

/// Two lists pushed in one loop (float SAXPY's shape): LLVM cannot tell their
/// blocks apart, so it could not sink either count store; the emitter defers
/// both (`SequenceState::dirty`) and stores them once, after the loop. Causal
/// control: a copy of one list taken every iteration needs its count current,
/// so that iteration stores it.
#[test]
fn two_lists_pushed_in_one_loop_store_their_counts_after_it() {
    let copied = TWO_LISTS.replace(
        "    list.push(mut ys, i % 31u64)\n",
        "    list.push(mut ys, i % 31u64)\n    let saved = copy xs\n    failure.check(list.len(saved) == i + 1u64, TextInvariant())\n",
    );
    for target in targets() {
        let ir = main_ir(TWO_LISTS, target.clone());
        assert!(ir.contains("!tbaa !\"sequence length\""), "{ir}");
        assert!(!stores_a_count_in_a_loop(&ir), "{ir}");
        let ir = main_ir(&copied, target);
        assert!(stores_a_count_in_a_loop(&ir), "{ir}");
    }
}

/// A push per index below a fresh list's reservation has spare capacity by
/// proof (FACT-3, `hirsel_rc::spare_push`): its steady iteration tests only
/// the loop's exit and no growth call remains. Causal control: one index past
/// the reservation keeps the capacity test and the growth call.
#[test]
fn a_push_per_index_below_the_reservation_tests_only_the_loop_exit() {
    let source = "pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let mut payload: List[u8] = []
  list.reserve(mut payload, 128u64)
  for j in 0u64..128u64 { list.push(mut payload, int.narrow(97u64 + j * 7u64 % 26u64)) }
  failure.check(list.len(payload) == 128u64, TextInvariant())
  Ok(())
}
";
    for target in targets() {
        let ir = main_ir(source, target.clone());
        let graph = blocks(&ir);
        // With no element loop left, LLVM stored the elements straight-line.
        if let Some(spare) = spare_loop(&graph) {
            let shown = || format!("hot path {:?}\n{ir}", spare.path);
            assert_eq!(spare.conditional(), 1, "{}", shown());
            assert_eq!(spare.loads() + spare.calls(), 0, "{}", shown());
        }
        assert!(!ir.contains("list__grow"), "{ir}");
        let past = source.replace("0u64..128u64", "0u64..129u64");
        let ir = main_ir(&past, target);
        let graph = blocks(&ir);
        let checked = hot(&graph);
        assert_eq!(
            checked.conditional(),
            2,
            "hot path {:?}\n{ir}",
            checked.path
        );
        assert!(ir.contains("list__grow"), "{ir}");
    }
}

/// Negative control: a copy taken every iteration shares the list, so each
/// spare-capacity push still loads and tests its count.
#[test]
fn a_list_shared_in_the_loop_tests_its_count_on_every_push() {
    let source = "pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let mut rows: List[u64] = []
  list.reserve(mut rows, 128u64)
  for i in 0u64..128u64 {
    let saved = copy rows
    list.push(mut rows, i)
    failure.check(list.len(saved) == i, TextInvariant())
  }
  failure.check(list.len(rows) == 128u64, TextInvariant())
  Ok(())
}
";
    for target in targets() {
        let ir = main_ir(source, target);
        let mut blocks = blocks(&ir);
        // A full shared list grows directly into unique storage and needs no
        // count test afterwards. Check the spare-capacity cycle, where a push
        // must still detach the shared root before writing its element.
        for block in blocks.values_mut() {
            if block.count(|l| l.contains("call ") && l.contains("list__grow")) > 0 {
                block.successors.clear();
            }
        }
        let hot = hot(&blocks);
        let counts: usize = hot
            .blocks
            .iter()
            .map(|b| b.count(|l| l.contains(" = icmp eq i32 ") && l.ends_with(", 1")))
            .sum();
        assert_eq!(counts, 1, "hot path {:?}\n{ir}", hot.path);
    }
}

/// Negative control: a list passed to an ordinary function escapes, so it has
/// no value facts and every push reads its length and capacity from memory.
#[test]
fn an_escaping_builder_reads_its_capacity_on_every_push() {
    let source = "fn extend(mut rows: List[u64]) -> () {
  list.push(mut rows, 1u64)
}
pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let mut rows: List[u64] = []
  list.reserve(mut rows, 128u64)
  extend(mut rows)
  for i in 1u64..128u64 { list.push(mut rows, i) }
  failure.check(list.len(rows) == 128u64, TextInvariant())
  Ok(())
}
";
    for target in targets() {
        let ir = main_ir(source, target);
        let blocks = blocks(&ir);
        let hot = hot(&blocks);
        assert!(hot.loads() >= 2, "hot path {:?}\n{ir}", hot.path);
    }
}

/// Negative control: pushing past the reserved range keeps the capacity test,
/// and the loop still reaches the growth call.
#[test]
fn pushing_past_the_reservation_keeps_growth_in_the_loop() {
    let source = "pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let mut rows: List[u64] = []
  list.reserve(mut rows, 16u64)
  for i in 0u64..128u64 { list.push(mut rows, i) }
  failure.check(list.len(rows) == 128u64, TextInvariant())
  Ok(())
}
";
    for target in targets() {
        let ir = main_ir(source, target);
        let blocks = blocks(&ir);
        let hot = hot(&blocks);
        assert_eq!(hot.conditional(), 2, "hot path {:?}\n{ir}", hot.path);
        let grows = blocks
            .iter()
            .filter(|(_, b)| b.count(|l| l.contains("call ") && l.contains("list__grow")) > 0)
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>();
        assert!(!grows.is_empty(), "{ir}");
        assert!(
            grows.iter().all(|name| cycle(&blocks, name).is_some()),
            "{ir}"
        );
    }
}

/// The optimized IR of `source` for `target`, whole.
fn module_ir(source: &str, target: TargetSpec) -> String {
    let source = source.to_owned();
    with_compiler_stack(move || {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("growth.hir", source).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let ready = build(&validated, &Options::new(target)).unwrap();
        emit_ir(&ready, OptLevel::Speed).unwrap()
    })
}

/// Each function the module defines: its symbol and its lines.
fn functions(ir: &str) -> BTreeMap<String, Vec<&str>> {
    let mut out = BTreeMap::new();
    let mut current: Option<(String, Vec<&str>)> = None;
    for line in ir.lines() {
        if line.starts_with("define ") {
            let at = line.find(" @").expect("a defined symbol") + 2;
            let end = at + line[at..].find('(').expect("a parameter list");
            current = Some((line[at..end].to_owned(), Vec::new()));
        } else if line == "}" {
            if let Some((name, lines)) = current.take() {
                out.insert(name, lines);
            }
        } else if let Some((_, lines)) = &mut current {
            lines.push(line);
        }
    }
    out
}

/// The runtime entries that allocate a heap block.
const ALLOCATING: [&str; 6] = [
    "hirsel_rt_v2_alloc",
    "hirsel_rt_v2_seq_grow",
    "hirsel_rt_v2_detach",
    "hirsel_rt_v2_unique_copy",
    "hirsel_rt_v2_unique_copy_value",
    "hirsel_rt_v2_reuse",
];

/// The callee symbol of a call line.
fn callee(line: &str) -> Option<&str> {
    let at = line.find("call ")?;
    let rest = &line[at..];
    let at = rest.find(" @")? + 2;
    let end = at + rest[at..].find('(')?;
    Some(&rest[at..end])
}

/// An empty list is the static empty sequence of its layout: reserving it
/// allocates the requested storage once, as C's one `malloc`, with no
/// allocation for `[]` and no detach before the reserve. Every call `main`
/// reaches is counted: the reserve body is a private function.
#[test]
fn an_empty_list_then_a_reserve_allocates_once() {
    let source = "pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let mut r: List[u64] = []
  list.reserve(mut r, 64u64)
  failure.check(list.len(r) == 0u64, TextInvariant())
  Ok(())
}
";
    assert_one_growth_allocation(source);
}

fn assert_one_growth_allocation(source: &str) {
    for target in targets() {
        let ir = module_ir(source, target);
        let functions = functions(&ir);
        let main = functions
            .keys()
            .find(|name| name.contains("4mainI") && !name.contains("_H1T"))
            .expect("main is defined");
        let mut reached = vec![main.as_str()];
        let mut allocating = Vec::new();
        let mut at = 0;
        while at < reached.len() {
            for line in &functions[reached[at]] {
                let Some(callee) = callee(line) else {
                    continue;
                };
                if ALLOCATING.contains(&callee) {
                    allocating.push(*line);
                } else if functions.contains_key(callee) && !reached.contains(&callee) {
                    reached.push(callee);
                }
            }
            at += 1;
        }
        assert_eq!(allocating.len(), 1, "{allocating:#?} in {reached:?}\n{ir}");
        assert!(allocating[0].contains("hirsel_rt_v2_seq_grow"), "{ir}");
    }
}

/// A push must grow the static empty block directly, without copying it first.
#[test]
fn the_first_push_into_an_empty_list_allocates_once() {
    assert_one_growth_allocation(
        "pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let mut xs: List[u64] = []
  list.push(mut xs, 1u64)
  failure.check(xs[0u64] == 1u64, TextInvariant())
  Ok(())
}",
    );
}

/// The allocating runtime calls `function` makes inside a loop.
fn allocations_in_loops(function: &[&str]) -> Vec<String> {
    let text = format!("define\n{}", function.join("\n"));
    let graph = blocks(&text);
    graph
        .iter()
        .filter(|(name, _)| cycle(&graph, name).is_some())
        .flat_map(|(_, b)| b.lines.iter())
        .filter(|l| callee(l).is_some_and(|c| ALLOCATING.contains(&c)))
        .cloned()
        .collect()
}

/// A list of flat records built only to be taken into a flat field is built
/// in that field's encoding (`Inst::FlatAppend`, LIR schema revision 22): the
/// loop allocates no record block and grows no pointer list, only the
/// payload's own bytes. Causal control: reading the list while it grows keeps
/// it ordinary, so each record is its own block again.
#[test]
fn a_list_built_for_a_flat_field_allocates_no_record_per_element() {
    let source = "flat type Message { payload: Bytes }
flat type Messages { items: List[Message] }
fn build(n: u64) -> Messages {
  let mut items: List[Message] = []
  for i in 0u64..n {
    let mut payload: List[u8] = []
    list.push(mut payload, int.narrow(i % 256u64))
    list.push(mut items, Message(payload: bytes.from_list(take payload)))
  }
  Messages(items: take items)
}
pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let messages = build(64)
  failure.check(list.len(messages.items) == 64u64, TextInvariant())
  Ok(())
}
";
    let ordinary = source.replace(
        "    list.push(mut items, Message(",
        "    failure.check(list.len(items) == i, TextInvariant())\n    list.push(mut items, Message(",
    );
    for target in targets() {
        for (source, built) in [(source.to_owned(), true), (ordinary.clone(), false)] {
            let ir = module_ir(&source, target.clone());
            let functions = functions(&ir);
            let (_, build) = functions
                .iter()
                .find(|(name, _)| name.contains("5buildI"))
                .expect("build is defined");
            let allocating = allocations_in_loops(build);
            let records = allocating
                .iter()
                .filter(|l| callee(l) == Some("hirsel_rt_v2_alloc"))
                .count();
            if built {
                assert_eq!(records, 0, "{allocating:#?}\n{ir}");
                assert!(!ir.contains("hirsel_rt_v2_detach"), "{ir}");
                assert!(
                    build.iter().any(|l| l.contains("hirsel_rt_v2_block_grow")),
                    "{ir}"
                );
            } else {
                assert!(records >= 1, "{allocating:#?}\n{ir}");
            }
        }
    }
}

/// A byte list frozen into a Bytes field of a built element is written where
/// the element will lie (`Inst::FlatField`, LIR schema revision 24):
/// records-messages/flat's message loop allocates, grows and frees no payload
/// block, and its byte loop under the field's reservation (a spare push,
/// FACT-3) calls nothing. Causal control: reading the payload's length before
/// its freeze keeps it an ordinary list, whose block is allocated and freed
/// per message again.
#[test]
fn a_byte_list_frozen_into_a_built_element_allocates_nothing_per_element() {
    let source = "flat type Message { id: u64, kind: u64, payload: Bytes }
flat type Messages { items: List[Message] }
fn build(n: u64) -> Messages {
  let mut items: List[Message] = []
  list.reserve(mut items, n)
  for i in 0u64..n {
    let mut payload: List[u8] = []
    let length = 24u64 + i % 104u64
    list.reserve(mut payload, length)
    for j in 0u64..length { list.push(mut payload, int.narrow(97u64 + (i + j * 7u64) % 26u64)) }
    list.push(mut items, Message(id: i, kind: i % 7u64, payload: bytes.from_list(take payload)))
  }
  Messages(items: take items)
}
pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let messages = build(64)
  failure.check(list.len(messages.items) == 64u64, TextInvariant())
  Ok(())
}
";
    let ordinary = source.replace(
        "    list.push(mut items, Message(",
        "    failure.check(list.len(payload) == length, TextInvariant())\n    list.push(mut items, Message(",
    );
    let blockwise = |l: &str| {
        callee(l).is_some_and(|c| {
            ALLOCATING.contains(&c) || c == "hirsel_rt_v2_free" || c == "hirsel_rt_v2_release"
        })
    };
    for target in targets() {
        for (source, built) in [(source.to_owned(), true), (ordinary.clone(), false)] {
            let ir = module_ir(&source, target.clone());
            let functions = functions(&ir);
            let (_, build) = functions
                .iter()
                .find(|(name, _)| name.contains("5buildI"))
                .expect("build is defined");
            let text = format!("define\n{}", build.join("\n"));
            let graph = blocks(&text);
            let looped: Vec<&String> = graph
                .iter()
                .filter(|(name, _)| cycle(&graph, name).is_some())
                .flat_map(|(_, b)| b.lines.iter())
                .filter(|l| l.contains("call ") && !l.contains("@llvm."))
                .collect();
            if built {
                assert!(
                    !looped.iter().any(|l| blockwise(l)),
                    "{looped:#?}\n{}",
                    build.join("\n")
                );
                // The byte loop: the cheapest cycle through a byte store.
                let (_, path) = graph
                    .iter()
                    .filter(|(_, b)| b.lines.iter().any(|l| l.starts_with("store i8 ")))
                    .filter_map(|(name, _)| cycle(&graph, name))
                    .min_by_key(|(cost, _)| *cost)
                    .expect("a loop stores a byte");
                let calls: usize = path.iter().map(|n| graph[n].calls()).sum();
                assert_eq!(calls, 0, "byte loop {path:?}\n{}", build.join("\n"));
            } else {
                assert!(
                    looped.iter().filter(|l| blockwise(l)).count() >= 2,
                    "{looped:#?}\n{}",
                    build.join("\n")
                );
            }
        }
    }
}

/// A heap record whose one field is a dying builder's list takes the
/// builder's block when it is unique: its two header words become the list's
/// length prefix and count and its descriptor the record's, so no second
/// block holds the list. A reservation grows the builder for that many of its
/// least element, a saturating product. Causal control: a record with another
/// field first copies the builder.
#[test]
fn a_record_of_one_built_list_takes_the_builders_block() {
    let source = "flat type Message { id: u64, payload: Bytes }
flat type Messages { items: List[Message] }
flat type Stamped { stamp: u64, items: List[Message] }
fn build(n: u64) -> Messages {
  let mut items: List[Message] = []
  list.reserve(mut items, n)
  for i in 0u64..n {
    list.push(mut items, Message(id: i, payload: b\"ab\"))
  }
  Messages(items: take items)
}
fn stamped(n: u64) -> Stamped {
  let mut items: List[Message] = []
  for i in 0u64..n {
    list.push(mut items, Message(id: i, payload: b\"cd\"))
  }
  Stamped(stamp: n, items: take items)
}
pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let m = build(64)
  let s = stamped(9)
  failure.check(list.len(m.items) == 64u64 && list.len(s.items) == 9u64, TextInvariant())
  Ok(())
}
";
    // The descriptor's kind bits, kept while its index becomes the record's.
    let retyped = |body: &[&str]| {
        body.iter()
            .any(|l| l.contains(" and i32 ") && l.contains("-536870912"))
    };
    for target in targets() {
        let ir = module_ir(source, target.clone());
        let functions = functions(&ir);
        let body = |name: &str| {
            functions
                .iter()
                .find(|(n, _)| n.contains(name))
                .map(|(_, b)| b.clone())
                .unwrap_or_else(|| panic!("{name} is defined\n{ir}"))
        };
        let build = body("5buildI");
        assert!(retyped(&build), "{ir}");
        assert!(
            build.iter().any(|l| l.contains("llvm.umul.with.overflow")),
            "{ir}"
        );
        assert!(!retyped(&body("7stampedI")), "{ir}");
    }
}

/// A list written around calls to a helper handed no reference
/// (`mix(h: u64, value: u64) -> u64`) keeps its must-unique fact across them:
/// nothing in the loop can add a holder, so after the pushes no write tests
/// its count or reaches a detach (the slots-churn kernel's `step`). Causal
/// control: a helper that borrows the list could duplicate it, so the write
/// after each call tests the count again and keeps its detach.
#[test]
fn a_call_handed_no_reference_keeps_a_list_unique_across_it() {
    let source = "fn mix(h: u64, value: u64) -> u64 {
  let wide: u128 = int.widen(h) * 131u128 + int.widen(value)
  int.truncate(wide)
}
pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let mut rows: List[u64] = []
  for i in 0u64..8u64 { list.push(mut rows, i) }
  let mut h: u64 = 1
  for i in 0u64..128u64 {
    rows[i % 8u64] = h
    h = mix(h, i)
    rows[(i + 1u64) % 8u64] = h
  }
  failure.check(h != rows[0u64], TextInvariant())
  Ok(())
}
";
    let borrowing = source
        .replace(
            "fn mix(h: u64, value: u64) -> u64 {",
            "fn mix(h: u64, value: u64, rows: List[u64]) -> u64 {",
        )
        .replace("int.widen(value)\n", "int.widen(value + list.len(rows))\n")
        .replace("h = mix(h, i)", "h = mix(h, i, rows)");
    let tests = |ir: &str| {
        ir.lines()
            .filter(|l| {
                l.contains("call ") && l.contains("@hirsel_rt_v2_detach")
                    || l.contains(" = icmp eq i32 ") && l.ends_with(", 1")
            })
            .count()
    };
    for target in targets() {
        let ir = main_ir(source, target.clone());
        assert_eq!(tests(&ir), 0, "{ir}");
        let ir = main_ir(&borrowing, target);
        assert!(tests(&ir) >= 2, "{ir}");
    }
}

/// An inclusive range's step tests `counter < end` (the header already proved
/// `counter <= end`), which also proves the header's next test. In
/// slots-churn's kernel, whose inner scan runs over `0..=active` while the
/// outer loop checks `active` against the length only where it grows it, a
/// scan step through a live slot then tests the slot and the range's exit
/// alone, as C does. The old `counter == end` step left the header's
/// `counter + 1 <= end` beside it (both folded into one exit branch), and
/// LLVM then kept the element index's bounds test too: three branches.
#[test]
fn an_inclusive_range_scan_tests_its_bound_once_per_item() {
    let source = "type Slot { generation: u8, live: bool, retired: bool, value: u64 }
type Id { index: u64, generation: u8 }
fn step(h: u64, value: u64) -> u64 {
  let wide: u128 = int.widen(h) * 131u128 + int.widen(value)
  int.truncate(wide)
}
fn churn(n: u64) -> u64 {
  let mut slots: List[Slot] = []
  let capacity = n / 255u64 + 3u64
  list.reserve(mut slots, capacity)
  for unused in 0u64..capacity { list.push(mut slots, Slot(generation: 0u8, live: false, retired: false, value: 0u64)) }
  slots[0u64] = Slot(generation: 1u8, live: true, retired: false, value: 10u64)
  slots[1u64] = Slot(generation: 1u8, live: true, retired: false, value: 20u64)
  slots[2u64].generation = 1u8
  let mut active: u64 = 2
  let mut total: u64 = 0
  for i in 0u64..n {
    failure.check(!slots[active].retired, Bounds())
    let id = Id(index: active, generation: slots[active].generation)
    slots[active].live = true
    slots[active].value = i + 1u64
    let mut h = i
    for index in 0u64..=active {
      if slots[index].live {
        h = step(step(step(h, index), int.widen(slots[index].generation)), slots[index].value)
      }
    }
    total = int.wrapping_add(total, h)
    slots[active].live = false
    if id.generation == 255u8 {
      slots[active].retired = true
      total = int.wrapping_add(total, 1u64)
      active += 1u64
      slots[active] = Slot(generation: 1u8, live: false, retired: false, value: 0u64)
    } else { slots[active].generation += 1u8 }
    let stale = slots[id.index].generation != id.generation || slots[id.index].retired || !slots[id.index].live
    failure.check(stale, Bounds())
    total = int.wrapping_add(total, 1u64)
  }
  total
}
pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  failure.check(churn(4096u64) != 0u64, TextInvariant())
  Ok(())
}
";
    for target in targets() {
        let ir = module_ir(source, target);
        let functions = functions(&ir);
        let (_, churn) = functions
            .iter()
            .find(|(name, _)| name.contains("5churnI") && !name.starts_with("_H1T"))
            .unwrap_or_else(|| panic!("churn is defined\n{ir}"));
        let churn = format!("define\n{}", churn.join("\n"));
        let graph = blocks(&churn);
        // A scan step through a live slot: the cheapest cycle through the hash.
        let (_, path) = graph
            .iter()
            .filter(|(_, b)| {
                b.lines
                    .iter()
                    .any(|l| l.contains("mul i64") && l.ends_with(", 131"))
            })
            .filter_map(|(name, _)| cycle(&graph, name))
            .min_by_key(|(cost, _)| *cost)
            .expect("the scan loops through the hash");
        let conditional: usize = path.iter().map(|n| graph[n].conditional()).sum();
        let calls: usize = path.iter().map(|n| graph[n].calls()).sum();
        // The slot's live test and the range's exit.
        assert_eq!(conditional, 2, "scan {path:?}\n{churn}");
        assert_eq!(calls, 0, "scan {path:?}\n{churn}");
    }
}
