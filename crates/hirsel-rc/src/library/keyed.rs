//! Materialized keyed primitives gate before pending actuals enter their body.
use super::{
    LibraryBody, Request,
    builder::{Body, Cx},
};
use crate::{
    PassError,
    instance::{InstanceError, Program},
};
use hirsel_core_contract::Passing;
use hirsel_lir::{ir::*, stage::Value};
use hirsel_syntax::DefPath;
use hirsel_types::GenericArgument;

macro_rules! adapter {
    ($name:ident, $path:literal) => {
        pub static $name: LibraryBody = LibraryBody {
            path: || hirsel_lir::pretransfer::adapter_path($path),
            build,
        };
    };
}
adapter!(MAP_INSERT, "map.insert");
adapter!(SET_INSERT, "set.insert");
adapter!(MAP_GET, "map.get");
adapter!(SET_CONTAINS, "set.contains");

pub fn request(
    program: &Program,
    path: &DefPath,
    arguments: Vec<GenericArgument>,
) -> Result<Option<Request>, InstanceError> {
    let Some(name) = ["map.insert", "set.insert", "map.get", "set.contains"]
        .into_iter()
        .find(|name| path.is_core(name))
    else {
        return Ok(None);
    };
    let helper = hirsel_lir::pretransfer::adapter_path(name);
    let body = super::body_index(&helper).expect("registered keyed adapter");
    let mut request = Request::operation(program, body, path, arguments)?;
    request.path = helper;
    Ok(Some(request))
}

fn build(cx: &mut Cx<'_>) -> Result<LirUnit<Value>, PassError> {
    let path = hirsel_lir::pretransfer::primitive(&cx.path).expect("keyed adapter path");
    let request = Request::operation(
        cx.program,
        super::body_index(&path).expect("keyed primitive body"),
        &path,
        cx.arguments.clone(),
    )?;
    let direct = cx.require(request)?;
    let mut b = Body::new(cx)?;
    let key = b.parameters[1];
    let reject = b.block(BlockKind::FailureCleanup, &[]);
    let start = b.current();
    b.at(reject);
    b.terminate(Terminator::RejectTransfer);
    b.at(start);
    // Whole parameter addresses cannot fail. Keep their latent failure paths
    // in pending custody as well, so no prefix edge disposes an actual.
    let entry_failures: Vec<_> =
        b.ed.code
            .iter()
            .flat_map(|code| code.iter())
            .filter_map(|i| match *i {
                Inst::Address { failure, .. } => Some(failure),
                _ => None,
            })
            .collect();
    for failure in entry_failures {
        let edge = b.ed.unit.tables.failures[failure.index()].cleanup;
        b.ed.unit.tables.edges[edge.index()].target = reject;
    }
    let (value, loan) = b.borrow(key.address, false)?;
    let failed = b.block(BlockKind::FailureCleanup, &[]);
    b.at(failed);
    b.end_loan(loan);
    b.jump(reject, &[]);
    b.at(start);
    let layout = b.layout_of(value);
    b.structural_access(b.cx.type_actual(0)?, layout)?;
    let word = b.u64_layout()?;
    let result = b.fresh(word);
    let failure = b.failure_to(failed, &[], Some("MEM-3"));
    b.push(Inst::Hash {
        result,
        value,
        layout,
        failure,
    });
    b.end_loan(loan);
    // This is the sole successful handoff. No semantic activation is added.
    let mut arguments = Vec::new();
    let mut loans = Vec::new();
    for p in b.parameters.clone() {
        let source = match b.ed.unit.slots[p.slot.index()].kind {
            SlotKind::Parameter(n) => n,
            _ => unreachable!(),
        };
        let value = match p.passing {
            Passing::Take => ArgumentValue::Value(b.take(p.address)?),
            Passing::Borrow => {
                let (value, loan) = b.borrow(p.address, false)?;
                loans.push(loan);
                ArgumentValue::Value(value)
            }
            Passing::Mut => {
                let address = b.place(AddressBase::Slot(p.slot), &[], p.layout, None)?;
                let version = b.version(p.slot);
                b.set_reaching(p.slot, version);
                ArgumentValue::Address { address, version }
            }
        };
        arguments.push(CallArgument {
            parameter: source,
            passing: p.passing,
            value,
        });
    }
    let symbol = b.symbol(direct.symbol);
    let result = b.call(
        Callee::Direct(symbol),
        vec![],
        direct.abi,
        arguments,
        &loans,
    );
    b.ret(result);
    Ok(b.finish())
}
