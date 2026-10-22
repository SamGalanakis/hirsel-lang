//! LIR schema revision 9: bound calls. A generic nested function's value has
//! no code of its own (spec #42 lets it be only a direct callee), so it is
//! called only through `Callee::Bound`, which names the instantiation's code
//! and takes the value's environment. Each rule has its rejected mutation and
//! accepted twin built by one function.
use crate::support;

use hirsel_core_contract::Passing;
use hirsel_lir::abi_types::CallableShape;
use hirsel_lir::ir::*;
use hirsel_lir::stage::Rc;
use support::*;

/// A closure borrowing the parameter into `ENV`, with or without code of its
/// own, called once and released.
fn closure_call(code: bool, callee: impl Fn(&mut Lir<Rc>, ValueId) -> Callee) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[(Passing::Borrow, l::I64)], None);
    let x = t.plain(l::I64);
    t.param(x);
    let abi = t.abi_shaped(CallableShape::Closure, &[], None);
    let code = code.then(|| ClosureCode {
        symbol: t.symbol(SymbolKind::Function),
        abi,
    });
    let captures = PoolRange::push(
        &mut t.builder.unit.pools.captures,
        [CaptureValue {
            value: x,
            usage: CaptureUse::Borrow,
        }],
    );
    t.builder.unit.tables.closures.push(ClosureSite {
        scoped: false,
        code,
        environment: l::ENV,
        captures,
        destination: DestinationClass::Heap,
        placement: t.heap(),
        anchor: SourceAnchorId(0),
    });
    let closure = ClosureId(t.builder.unit.tables.closures.len() as u32 - 1);
    let value = t.owned(l::CLOSURE);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Closure {
            result: value,
            closure,
            failure,
        },
    );
    let after = t.builder.block(BlockKind::Body, &[]);
    let failed = t.releasing(&[value]);
    let callee = callee(&mut t, value);
    t.call_to(ENTRY, callee, abi, &[], None, &[], (after, &[]), failed);
    t.release(after, value, &[]);
    t.builder.ret(after, None);
    t.check()
}

/// A code-less closure is called through a bound call naming the code; as an
/// indirect callee (the mutant) it would run code it does not have.
#[test]
fn a_code_less_closure_is_called_only_through_a_bound_call() {
    pair(
        |mutant| {
            closure_call(false, |t, value| {
                if mutant {
                    Callee::Indirect(value)
                } else {
                    Callee::Bound {
                        code: t.symbol(SymbolKind::Function),
                        closure: value,
                    }
                }
            })
        },
        |k| *k == K::CallShape("a code-less closure outside a bound call"),
    );
}

/// A bound call runs function code; naming a type symbol (the mutant) runs
/// nothing.
#[test]
fn a_bound_call_names_function_code() {
    pair(
        |mutant| {
            closure_call(true, |t, value| Callee::Bound {
                code: t.symbol(if mutant {
                    SymbolKind::Type
                } else {
                    SymbolKind::Function
                }),
                closure: value,
            })
        },
        |k| *k == K::CallShape("bound callee"),
    );
}
