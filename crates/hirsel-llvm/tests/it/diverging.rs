//! A diverging call is not `noreturn`: its callee's failure returns status 1
//! along the failure edge, and only a zero status is unreachable.
use crate::support;

use hirsel_lir::emission::{EmitInputs, OptLevel, symbol_name};
use hirsel_lir::{ir::*, layout_types::TargetSpec};
use hirsel_llvm::emit_ir;
use hirsel_types::Primitive;
use support::{programs::Program, *};

fn program(target: &TargetSpec) -> Program {
    let mut schema = Schema::new();
    let never = schema.types.p(Primitive::Never);
    let unit = schema.types.unit();
    schema.signature("stop", &[], never);
    schema.signature("probe", &[], unit);
    let table = schema.build(target);
    let manifest = manifest();
    // `fn stop() -> Never` fails LANG-9; its own ABI diverges, so it has no Return.
    let mut stop = Unit::new(&table, "stop", "stop");
    let failure = stop.fails(Some("LANG-9"));
    stop.builder.term(ENTRY, Terminator::Raise(failure));
    // `fn probe() { stop() }` ends in the diverging call.
    let mut probe = Unit::new(&table, "probe", "probe");
    let symbol = probe.symbol(SymbolKind::Function, "stop");
    let failure = probe.fails(None);
    probe.diverging_call(ENTRY, Callee::Direct(symbol), "stop", failure);
    let units = vec![
        seal(&table, &manifest, stop.finish()),
        seal(&table, &manifest, probe.finish()),
    ];
    Program {
        table,
        manifest,
        units,
        statics: Vec::new(),
        finalizers: Default::default(),
    }
}

#[test]
fn a_diverging_call_branches_on_status_without_noreturn() {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let program = program(&target);
    assert!(program.table.table.fn_abis[program.table.abi("stop").0 as usize].diverges);
    let input = input();
    let inputs = EmitInputs {
        index: None,
        verify_backend: true,
        input: &input,
        target: &program.table.target,
        layouts: &program.table.table,
        keys: &program.table.keys,
        manifest: &program.manifest,
        statics: &program.statics,
        inline_instances: &std::collections::BTreeSet::new(),
        available_externally: &std::collections::BTreeSet::new(),
        callbacks: false,
        tracking: false,
        opt: OptLevel::None,
        measurement: false,
        fuel: false,
    };
    let ir = emit_ir(&program.units(), &inputs).unwrap();
    assert!(!ir.contains("noreturn"), "{ir}");
    let stop = symbol_name(&path("stop"), Some(&instance("stop")));
    let probe = symbol_name(&path("probe"), Some(&instance("probe")));
    let body = ir
        .split(&format!("@{probe}("))
        .nth(1)
        .and_then(|rest| rest.split("\n}\n").next())
        .unwrap_or_else(|| panic!("no probe body in {ir}"));
    let call = body
        .find(&format!("call i8 @{stop}("))
        .unwrap_or_else(|| panic!("no status call in {body}"));
    let after = &body[call..];
    assert!(after.contains("icmp eq i8"), "{body}");
    assert!(after.contains("br i1"), "{body}");
    assert!(after.contains("unreachable"), "{body}");
}
