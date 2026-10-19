//! `noalias` on `mut` place addresses (`docs/spec/implementation.md`, Aliasing facts):
//! the Ready probes' `bump(mut x)` states it on its address parameter, its by-value
//! parameters and runtime declarations state none, and the same program with a C
//! callback named states none anywhere.
use crate::support;

use std::collections::BTreeSet;

use hirsel_lir::descriptor_registry::DescriptorRegistry;
use hirsel_lir::emission::{EmitInputs, NO_CALLBACKS, OptLevel};
use hirsel_lir::layout_types::TargetSpec;
use hirsel_llvm::emit_optimized_ir;
use hirsel_syntax::DefPath;
use support::programs::Program;
use support::ready::*;
use support::*;

fn ir(program: &Program, opt: OptLevel, callbacks: &BTreeSet<DefPath>) -> String {
    let codes = failure_codes();
    let input = input();
    let none = BTreeSet::new();
    let inputs = EmitInputs {
        input: &input,
        target: &program.table.target,
        layouts: &program.table.table,
        manifest: &program.manifest,
        failures: &codes,
        statics: &program.statics,
        finalizers: &program.finalizers,
        inline_instances: &none,
        available_externally: &none,
        callbacks,
        registry: DescriptorRegistry::new(&program.table.table, &program.finalizers),
        opt,
        measurement: false,
        fuel: false,
    };
    emit_optimized_ir(&program.units(), &inputs)
        .unwrap_or_else(|e| panic!("{}: {e}", program.table.target.triple))
}

/// The parameter list of the definition whose symbol contains `name`, not its thunk.
fn parameters<'a>(ir: &'a str, name: &str) -> &'a str {
    let line = ir
        .lines()
        .find(|l| l.starts_with("define") && l.contains(name) && !l.contains("@_H1T"))
        .unwrap_or_else(|| panic!("no definition of {name}: {ir}"));
    let start = line.find('(').expect("a parameter list");
    &line[start..]
}

#[test]
fn a_mut_place_address_is_noalias_unless_the_program_names_a_callback() {
    let callback: BTreeSet<DefPath> = [DefPath::core("callback")].into();
    for target in TargetSpec::executed() {
        let program = ready_program(&target);
        for opt in [OptLevel::None, OptLevel::Speed] {
            let stated = ir(&program, opt, &NO_CALLBACKS);
            // bump(ctx, mut x): the context states nothing, the address is noalias.
            let bump = parameters(&stated, "4bump");
            assert_eq!(bump.matches("noalias").count(), 1, "{opt:?}: {bump}");
            assert!(bump.contains("ptr noalias"), "{opt:?}: {bump}");
            // A by-value caller: no aliasing attribute.
            let caller = parameters(&stated, "11mut__caller");
            assert!(!caller.contains("noalias"), "{opt:?}: {caller}");
            // Runtime declarations state no parameter aliasing (an allocator's
            // result is the only `noalias` they carry).
            for declared in stated
                .lines()
                .filter(|l| l.starts_with("declare") && l.contains("@hirsel_rt_v2_"))
            {
                let (_, parameters) = declared.split_once('(').unwrap();
                assert!(!parameters.contains("noalias"), "{opt:?}: {declared}");
            }
            let unstated = ir(&program, opt, &callback);
            let bump = parameters(&unstated, "4bump");
            assert!(!bump.contains("noalias"), "{opt:?}: {bump}");
        }
    }
}
