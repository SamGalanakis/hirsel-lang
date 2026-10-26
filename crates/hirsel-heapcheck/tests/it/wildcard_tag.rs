//! A native catch-all destination never licenses a corrupt ordinal in H.
use crate::fixtures::*;
use hirsel_heapcheck::build::{Layouts, path};
use hirsel_heapcheck::{Config, Program, Run, ViolationKind};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::TargetSpec;
use hirsel_lir::stage::Rc;
use hirsel_lir::statics::StaticData;

fn dispatch(tag: u32) -> Run {
    let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut layouts);
    let variant = layouts.variant(&[&[], &[], &[]]);
    let (target, table) = layouts.finish();
    let mut f = F::<Rc>::new("main", c);
    let symbol = f.b.symbol(SymbolKind::Constant, path("FORGED"));
    let value = f.b.plain(ValueRepr::Aggregate, variant);
    f.inst(Inst::ReadStatic {
        result: value,
        symbol,
    });
    let hit = f.b.block(BlockKind::Body, &[]);
    let miss = f.b.block(BlockKind::Body, &[]);
    let yes = f.b.edge(hit, &[]);
    let no = f.b.edge(miss, &[]);
    let arms = PoolRange::push(
        &mut f.b.unit.pools.switch_arms,
        [
            SwitchArm { value: 0, edge: no },
            SwitchArm {
                value: 1,
                edge: yes,
            },
            SwitchArm { value: 2, edge: no },
        ],
    );
    f.b.term(
        f.cur,
        Terminator::Switch {
            value,
            arms,
            otherwise: no,
        },
    );
    for (block, marker) in [(hit, 1), (miss, 0)] {
        f.cur = block;
        let marker = f.int(marker);
        f.print(marker);
        f.ret(None);
    }
    let unit = f.finish(&table);
    let mut p: Program<Rc> = program(target, table, vec![unit]);
    p.statics.push(StaticData::plain(
        Symbol {
            kind: SymbolKind::Constant,
            definition: path("FORGED"),
            instance: None,
        },
        variant,
        tag.to_le_bytes().to_vec(),
    ));
    run(&p, &path("main"), &[], &mut Printer, &Config::default())
}

#[test]
fn checked_h_rejects_a_forged_tag_even_with_a_catch_all() {
    for tag in [3, 255, u32::MAX] {
        let result = dispatch(tag);
        assert_eq!(
            result.violation(),
            Some(ViolationKind::Malformed),
            "{result:?}"
        );
    }
    for (tag, expected) in [(0, "0\n"), (1, "1\n"), (2, "0\n")] {
        let result = dispatch(tag);
        let audit = result.result.unwrap();
        assert_eq!(audit.observation.output, expected.as_bytes());
    }
}
