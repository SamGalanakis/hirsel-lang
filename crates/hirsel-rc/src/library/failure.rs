//! failure.check uses the ordinary call anchor and checked failure edges.
use super::builder::{Body, Cx};
use super::{LibraryBody, LibraryError, core_path};
use hirsel_lir::ir::*;
use hirsel_lir::stage::Value;

pub static CHECK: LibraryBody = LibraryBody {
    path: || core_path("failure.check"),
    build: check,
};

fn check(cx: &mut Cx<'_>) -> Result<LirUnit<Value>, LibraryError> {
    let mut b = Body::new(cx)?;
    let condition = b.read_bits(b.parameters[0].address)?;
    let rule = b.read_bits(b.parameters[1].address)?;
    let arms = [0, 1, 2].map(|_| b.block(BlockKind::Body, &[]));
    let invalid = b.block(BlockKind::Body, &[]);
    let otherwise = b.edge(invalid, &[]);
    let edges = arms.map(|arm| b.edge(arm, &[]));
    let switches = PoolRange::push(
        &mut b.ed.unit.pools.switch_arms,
        edges
            .into_iter()
            .enumerate()
            .map(|(value, edge)| SwitchArm {
                value: value as u32,
                edge,
            }),
    );
    b.terminate(Terminator::Switch {
        value: rule,
        arms: switches,
        otherwise,
    });
    for (arm, rule) in arms.into_iter().zip(["SEM-1", "SEM-3", "MEM-14"]) {
        b.at(arm);
        let failure = b.failure(Some(rule));
        b.push(Inst::Check { condition, failure });
        let result =
            b.cx.fn_abi()
                .result
                .map(|r| b.constant(r.layout, ConstantValue::Unit));
        b.ret(result);
    }
    b.at(invalid);
    // The declared enum has exactly three variants; C proves this default
    // impossible from the exhaustive switch. No ordinal can choose a rule.
    b.terminate(Terminator::Unreachable);
    Ok(b.finish())
}
