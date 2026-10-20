//! Null-pointer niches (layout version 2) on the ledger: the word of the
//! dataful variant's block reference is the tag, null for the fieldless
//! variant, and a move that takes that reference out, alone or inside a
//! larger part (layout version 5), leaves its variant readable, as the native
//! move leaves the word's bits in place.
use crate::fixtures;

use fixtures::*;
use hirsel_heapcheck::build::{Layouts, path};
use hirsel_heapcheck::{Config, Program, Run, ViolationKind};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{LayoutId, TargetSpec};
use hirsel_lir::stage::{Rc, Stage};

fn execute<S: Stage>(p: &Program<S>) -> Run {
    run(p, &path("main"), &[], &mut Printer, &Config::default())
}

fn output(run: &Run) -> String {
    let audit = run
        .result
        .as_ref()
        .unwrap_or_else(|v| panic!("expected a clean audit: {v}"));
    String::from_utf8(audit.observation.output.clone()).unwrap()
}

/// An owned inline value of `layout` from `fields`.
fn inline(f: &mut F<Rc>, layout: LayoutId, ctor: ConstructorKind, fields: &[ValueId]) -> ValueId {
    let fields: Vec<_> = fields
        .iter()
        .enumerate()
        .map(|(i, &v)| (i as u32, v))
        .collect();
    let k = f.b.construct(ctor, layout, &fields, HEAP);
    let v = f.b.owned(ValueRepr::Aggregate, layout);
    let failure = f.propagates();
    f.inst(Inst::Construct {
        result: v,
        construct: k,
        failure,
    });
    v
}

/// `Node(Box(5), 7)` in a block of `Leaf | Node(Box[cell], u64)`. The box is
/// taken out, the u64 read through its variant's tag check, then the node is
/// released with only that u64 live (fixed) or with every field (broken).
fn moved_niche_field(fixed: bool) -> Program<Rc> {
    let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut l);
    let node = l.niche(0, &[c.boxed, c.u64]);
    let node_box = l.block(node);
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", c);
    let symbol = f.b.symbol(SymbolKind::Type, path("Node"));
    let five = f.int(5);
    let a = f.boxed(five);
    let seven = f.int(7);
    let node_k = f.b.construct(
        ConstructorKind::Variant {
            definition: symbol,
            variant: 1,
        },
        node_box,
        &[(0, a), (1, seven)],
        HEAP,
    );
    let p = f.b.owned(BLOCK, node_box);
    let failure = f.propagates();
    f.inst(Inst::Construct {
        result: p,
        construct: node_k,
        failure,
    });
    let at = f.b.address(
        AddressBase::Value(p),
        &[ProjectionStep::VariantField {
            variant: 1,
            field: 0,
        }],
        BLOCK,
        c.boxed,
    );
    let failure = f.propagates();
    f.inst(Inst::Address {
        address: at,
        failure,
    });
    let taken = f.b.owned(BLOCK, c.boxed);
    f.inst(Inst::Read {
        result: taken,
        address: at,
        reaching: p,
        mode: ReadMode::Take,
    });
    let n = f.load(
        p,
        &[ProjectionStep::VariantField {
            variant: 1,
            field: 1,
        }],
        U64,
        c.u64,
    );
    f.print(n);
    let v = f.unbox(taken);
    f.print(v);
    let live = if fixed {
        MaskRef::Static(f.b.mask(2, &[1]))
    } else {
        MaskRef::All
    };
    f.release_masked(p, node_box, live);
    f.release(taken, c.boxed);
    f.ret(None);
    let main = f.finish(&table);
    program(target, table, vec![main])
}

#[test]
fn a_moved_niche_reference_leaves_its_variant_readable() {
    assert_eq!(output(&execute(&moved_niche_field(true))), "7\n5\n");
}

#[test]
fn a_moved_niche_reference_is_not_released_again() {
    let run = execute(&moved_niche_field(false));
    let violation = run.result.expect_err("the moved box is released twice");
    assert_eq!(
        violation.kind,
        ViolationKind::UninitializedScan,
        "{violation}"
    );
}

/// `Node((Box(5), 3), 7)` in a block of `Leaf | Node((Box[cell], u64), u64)`,
/// whose niche word is the box nested in the pair. The whole pair is taken
/// out, the u64 read through its variant's tag check, then the node is
/// released with only that u64 live (fixed) or with every field (broken).
fn moved_nested_niche_part(fixed: bool) -> Program<Rc> {
    let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut l);
    let pair = l.record(&[c.boxed, c.u64]);
    let node = l.niche(0, &[pair, c.u64]);
    let node_box = l.block(node);
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", c);
    let five = f.int(5);
    let a = f.boxed(five);
    let three = f.int(3);
    let tuple = inline(&mut f, pair, ConstructorKind::Tuple, &[a, three]);
    let symbol = f.b.symbol(SymbolKind::Type, path("Node"));
    let seven = f.int(7);
    let node_k = f.b.construct(
        ConstructorKind::Variant {
            definition: symbol,
            variant: 1,
        },
        node_box,
        &[(0, tuple), (1, seven)],
        HEAP,
    );
    let p = f.b.owned(BLOCK, node_box);
    let failure = f.propagates();
    f.inst(Inst::Construct {
        result: p,
        construct: node_k,
        failure,
    });
    let at = f.b.address(
        AddressBase::Value(p),
        &[ProjectionStep::VariantField {
            variant: 1,
            field: 0,
        }],
        ValueRepr::Aggregate,
        pair,
    );
    let failure = f.propagates();
    f.inst(Inst::Address {
        address: at,
        failure,
    });
    let taken = f.b.owned(ValueRepr::Aggregate, pair);
    f.inst(Inst::Read {
        result: taken,
        address: at,
        reaching: p,
        mode: ReadMode::Take,
    });
    let n = f.load(
        p,
        &[ProjectionStep::VariantField {
            variant: 1,
            field: 1,
        }],
        U64,
        c.u64,
    );
    f.print(n);
    let live = if fixed {
        MaskRef::Static(f.b.mask(2, &[1]))
    } else {
        MaskRef::All
    };
    f.release_masked(p, node_box, live);
    f.release(taken, pair);
    f.ret(None);
    let main = f.finish(&table);
    program(target, table, vec![main])
}

#[test]
fn a_moved_part_holding_the_niche_word_leaves_its_variant_readable() {
    assert_eq!(output(&execute(&moved_nested_niche_part(true))), "7\n");
}

#[test]
fn a_moved_part_holding_the_niche_word_is_not_released_again() {
    let run = execute(&moved_nested_niche_part(false));
    let violation = run.result.expect_err("the moved pair is released twice");
    assert_eq!(
        violation.kind,
        ViolationKind::UninitializedScan,
        "{violation}"
    );
}

/// `Leaf | Node(u64, Box[node])` chains: construction writes the null word for
/// a Leaf and the box for a Node, comparison orders by the decoded ordinal,
/// and releasing the chain drains every node through the niche's drop arm.
#[test]
fn niche_chains_construct_compare_and_release_by_their_null_word() {
    let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut l);
    let chain = l.unit();
    let next = l.block(chain);
    let shape = l.niche(0, &[c.u64, next]);
    let record = l.get(shape).clone();
    *l.get_mut(chain) = record;
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", c);
    let symbol = f.b.symbol(SymbolKind::Type, path("Chain"));
    let leaf_k = ConstructorKind::Variant {
        definition: symbol,
        variant: 0,
    };
    let node_k = ConstructorKind::Variant {
        definition: symbol,
        variant: 1,
    };
    let mut acc = inline(&mut f, chain, leaf_k, &[]);
    for i in 0..3 {
        let boxed_k = f.b.construct(ConstructorKind::Box, next, &[(0, acc)], HEAP);
        let tail = f.b.owned(BLOCK, next);
        let failure = f.propagates();
        f.inst(Inst::Construct {
            result: tail,
            construct: boxed_k,
            failure,
        });
        let value = f.int(i);
        acc = inline(&mut f, chain, node_k, &[value, tail]);
    }
    let leaf = inline(&mut f, chain, leaf_k, &[]);
    for (op, left, right) in [
        (CompareOp::Lt, leaf, acc),
        (CompareOp::Gt, acc, leaf),
        (CompareOp::Eq, acc, acc),
        (CompareOp::Eq, leaf, leaf),
    ] {
        let result = f.b.plain(BOOL, f.c.bool);
        let failure = f.raises("MEM-3", f.propagate);
        f.inst(Inst::Compare {
            result,
            op,
            left,
            right,
            layout: chain,
            failure,
        });
        f.print(result);
    }
    f.release(leaf, chain);
    f.release(acc, chain);
    f.ret(None);
    let main = f.finish(&table);
    let run = execute(&program(target, table, vec![main]));
    assert_eq!(output(&run), "1\n1\n1\n1\n");
    assert_eq!(run.counters.allocations, 3);
    assert_eq!(run.counters.frees, 3);
}
