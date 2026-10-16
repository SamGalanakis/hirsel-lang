//! Compiled drop glue against the runtime's step-program walk (`support::glue`): the same
//! typed graphs released both ways must finalize, close and free the same blocks in the
//! same order. Native runs need the pinned toolchain, so `tools/check-native.sh` runs them
//! (`--include-ignored`); `tools/gate` runs emission.

use crate::support;

use std::collections::BTreeMap;

use hirsel_lir::layout_types::TargetSpec;
use hirsel_llvm::{EmitInputs, OptLevel, emit_object};
use support::glue::*;
use support::native::*;
use support::*;

const OPTS: [OptLevel; 2] = [OptLevel::None, OptLevel::Speed];
/// Seeded graphs per root.
const SEEDS: u64 = 8;

fn emit(corpus: &Corpus, opt: OptLevel) -> hirsel_lir::stage::ObjectArtifact {
    let codes = failure_codes();
    let input = input();
    let program = &corpus.program;
    let inputs = EmitInputs {
        input: &input,
        target: &program.table.target,
        layouts: &program.table.table,
        manifest: &program.manifest,
        failures: &codes,
        statics: &program.statics,
        finalizers: &program.finalizers,
        inline_instances: &std::collections::BTreeSet::new(),
        available_externally: &std::collections::BTreeSet::new(),
        registry: hirsel_lir::descriptor_registry::DescriptorRegistry::new(
            &program.table.table,
            &program.finalizers,
        ),
        opt,
        measurement: false,
    };
    emit_object(&program.units(), &inputs)
        .unwrap_or_else(|e| panic!("{}: {e}", program.table.target.triple))
}

#[test]
fn glue_covers_every_root_without_a_finalizer_and_frees_through_the_runtime() {
    for target in TargetSpec::executed() {
        let corpus = corpus(&target);
        let glued: Vec<_> = corpus
            .roots
            .iter()
            .filter(|r| r.entry.is_some())
            .map(|r| r.name)
            .collect();
        assert_eq!(
            glued,
            [
                "leaf", "rec", "recs", "wide", "chain", "tree", "niches", "rose", "obj", "held",
                "outer"
            ]
        );
        for opt in OPTS {
            let object = emit(&corpus, opt);
            let imports: Vec<_> = object.imports.iter().map(|i| i.name.as_str()).collect();
            // The free of a dead block, the runtime's release for children glue leaves to
            // it, and the handle close.
            for entry in [
                "hirsel_rt_v2_free",
                "hirsel_rt_v2_release",
                "hirsel_rt_v2_close_handle",
            ] {
                assert!(imports.contains(&entry), "{entry} in {imports:?}");
            }
        }
    }
}

/// The trace sections the harness prints, by scenario name and failing period.
fn sections(output: &str) -> BTreeMap<&str, Vec<&str>> {
    let mut sections: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut current = None;
    for line in output.lines() {
        if let Some(name) = line.strip_prefix("== ") {
            current = Some(name);
        } else if line.starts_with("runs ") {
            current = None;
        } else if let Some(name) = current {
            sections.entry(name).or_default().push(line);
        }
    }
    sections
}

/// The blocks of `graph` in the order a release of its root frees them: each block after
/// the blocks its references name, the reference at the highest offset first (a variant's
/// fields drop in reverse).
fn post_order(graph: &Graph, block: usize, out: &mut Vec<usize>) {
    let mut relocs = graph.blocks[block].relocs.clone();
    relocs.sort_by_key(|(offset, _)| std::cmp::Reverse(*offset));
    for (_, child) in relocs {
        post_order(graph, child, out);
    }
    out.push(block);
}

#[test]
#[ignore = "native: run by tools/check-native.sh"]
fn glue_and_the_runtime_walk_free_the_same_blocks_in_the_same_order() {
    for target in TargetSpec::executed() {
        let corpus = corpus(&target);
        let scenarios = scenarios(&corpus, SEEDS);
        for opt in OPTS {
            let dir = scratch(&format!("glue-{}-{opt:?}", target.triple));
            let object = emit(&corpus, opt);
            std::fs::write(dir.join("glue_cases.h"), cases_header(&corpus, &scenarios)).unwrap();
            let source = std::path::PathBuf::from(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/native/glue_harness.c"
            ));
            let executable = link(&target, &dir, &[("glue.o", &object.bytes)], &[source]);
            let output = run(&target, &executable);
            let label = format!("{} {opt:?}", target.triple);
            // Diagnostic and production mode, each with no failure, every finalizer and
            // close failing, and every third.
            let runs = scenarios.len() * 6;
            assert!(
                output.contains(&format!("runs {runs} differing 0\n")),
                "{label}:\n{output}"
            );
            assert_eq!(
                output.matches("deep chain status 0 live 0\n").count(),
                2,
                "{label}:\n{output}"
            );
            let sections = sections(&output);
            // The full tree: one slab, so the free list links are the whole free order,
            // and it is the post-order of the drop program.
            let tree = &scenarios
                .iter()
                .find(|s| s.name == "tree full")
                .unwrap()
                .graph;
            let mut order = Vec::new();
            post_order(tree, 0, &mut order);
            assert_eq!(order.len(), (1 << (TREE_DEPTH + 1)) - 1);
            let mut want = vec![String::from("status 0")];
            for block in 0..order.len() {
                let at = order.iter().position(|b| *b == block).unwrap();
                want.push(match at {
                    0 => format!("b{block} free after -"),
                    _ => format!("b{block} free after b{}", order[at - 1]),
                });
            }
            want.extend(["live 0".into(), "left 0".into()]);
            assert_eq!(sections["tree full failing 0"], want, "{label}");
            // The chain is longer than the glue's budget: all of it is freed, deepest first.
            let chain = &sections["chain full failing 0"];
            assert_eq!(chain.len(), CHAIN_LINKS as usize + 4, "{label}");
            assert_eq!(chain[1], "b0 free after b1", "{label}");
            // Every scenario frees its whole graph, and the corpus reaches finalizers,
            // closes, failures and surviving shared blocks.
            for (name, lines) in &sections {
                assert_eq!(lines.last(), Some(&"left 0"), "{label} {name}");
            }
            let all: Vec<&str> = sections.values().flatten().copied().collect();
            for prefix in ["finalize b", "close ", "primary 1 ", "cause 1 ", "status 1"] {
                assert!(
                    all.iter().any(|l| l.starts_with(prefix)),
                    "{label}: no {prefix}"
                );
            }
            assert!(all.iter().any(|l| l.contains(" live count ")), "{label}");
        }
    }
}

/// The tree root is the node `crates/hirsel-rt/harness/binarytrees.c` builds: two block
/// references, a Leaf when the first is null. `harness/bench.sh` sets `HIRSEL_GLUE_OBJECT`
/// to have the x86-64 corpus object written there, with `<path>.binding.h` beside it: the
/// program registry, the node's descriptor index and `glue_destroy`, the tree entry through
/// its ABI v1 signature (`hirsel_llvm::c_call`). binarytrees times its releases through it.
#[test]
fn the_tree_root_is_the_binarytrees_node() {
    use hirsel_lir::layout_types::{SizeSpec, TagForm};
    let corpus = corpus(&TargetSpec::x86_64_unknown_linux_gnu());
    let tree = corpus.root("tree");
    let record = &corpus.program.table.table.layouts[tree.payload.0 as usize];
    let tag = record.tag.as_ref().expect("a variant");
    assert_eq!(record.size, SizeSpec::Fixed(16));
    assert!(matches!(tag.form, TagForm::Niche { .. }) && tag.offset == 0);
    tree.entry.as_ref().expect("glue covers the tree");
    if let Some(path) = std::env::var_os("HIRSEL_GLUE_OBJECT") {
        let path = std::path::PathBuf::from(path);
        let corpus = binarytrees_corpus(&TargetSpec::x86_64_unknown_linux_gnu());
        let tree = corpus.root("tree");
        let registry = hirsel_lir::descriptor_registry::DescriptorRegistry::new(
            &corpus.program.table.table,
            &corpus.program.finalizers,
        );
        std::fs::write(&path, emit(&corpus, OptLevel::Speed).bytes).unwrap();
        let index = registry.index(tree.payload).unwrap();
        let header = format!(
            "struct hirsel_tree_binding {{ const hirsel_rt_v2_layout *const *layouts; uint64_t count; }};\nextern const struct hirsel_tree_binding hirsel_tree_program __asm__(\"hirsel.program.registry\");\n#define NODE_DESCRIPTOR_INDEX {index}u\n{}",
            corpus.program.c_call("drop_tree", "glue_destroy")
        );
        std::fs::write(path.with_extension("binding.h"), header).unwrap();
    }
}
