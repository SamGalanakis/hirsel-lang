//! Static back-end coverage used by corpus minimisation. No executable runs.
use super::{OptLevel, Options, ReadyProgram, TargetSpec, build, emit_ir};
use hirsel_engine::ValidatedNativeInput;
use hirsel_lir::ir::{ArgumentValue, Inst, LirUnit, Ref};
use hirsel_lir::stage::Ready;
use std::collections::BTreeSet;

/// Erase arena ordinals from debug vocabulary, retaining enum names (U64,
/// F32, etc.). This is a coverage key, never an artifact identity.
fn vocabulary(text: &str) -> String {
    let mut out = String::new();
    let mut digits = false;
    let mut identifier = false;
    for c in text.chars() {
        if c.is_ascii_digit() && !identifier {
            if !digits {
                out.push('#');
            }
            digits = true;
        } else {
            out.push(c);
            digits = false;
        }
        if !c.is_ascii_digit() {
            identifier = c.is_ascii_alphabetic() || c == '_';
        }
    }
    out
}

fn abi_shape(program: &ReadyProgram, id: hirsel_lir::abi_types::FnAbiId) -> String {
    let abi = &program.table.fn_abis[id.0 as usize];
    // Keys contain callable identities; coverage depends on the physical ABI.
    let parameters: Vec<_> = abi
        .parameters
        .iter()
        .map(|p| {
            format!(
                "{:?}/{:?}/{:?}/{:?}/{:?}",
                p.convention,
                p.passing,
                p.slots,
                p.extension,
                program.table.layout(p.layout).unwrap().kind
            )
        })
        .collect();
    let result = abi.result.as_ref().map(|r| {
        format!(
            "{:?}/{:?}/{:?}",
            r.slots,
            r.borrowed_parameters,
            program.table.layout(r.layout).unwrap().kind
        )
    });
    vocabulary(&format!(
        "{:?}/{:?}/{}/{}/{}/{}/{:?}/{}->{result:?}",
        abi.shape,
        abi.multiplicity,
        abi.context,
        abi.environment,
        abi.destination,
        abi.diverges,
        abi.result_arena,
        parameters.join(",")
    ))
}

fn operand(program: &ReadyProgram, unit: &LirUnit<Ready>, reference: Ref) -> String {
    match reference {
        Ref::Call(id) => {
            let call = &unit.tables.calls[id.index()];
            let callee = format!("{:?}", call.callee)
                .split(['(', ' '])
                .next()
                .unwrap()
                .to_string();
            let arguments: Vec<_> = call
                .arguments
                .get(&unit.pools.call_arguments)
                .unwrap()
                .iter()
                .map(|arg| {
                    let value = match arg.value {
                        ArgumentValue::Value(value) => Ref::Value(value),
                        ArgumentValue::Address { address, .. } => Ref::Address(address),
                    };
                    format!("{:?}/{}", arg.passing, operand(program, unit, value))
                })
                .collect();
            let result = call.result.map(|v| operand(program, unit, Ref::Value(v)));
            return format!(
                "Call/{callee}/[{}]->{result:?}/{}",
                arguments.join(","),
                abi_shape(program, call.abi)
            );
        }
        Ref::Construct(id) => {
            let site = &unit.tables.constructs[id.index()];
            let fields: Vec<_> = site
                .fields
                .get(&unit.pools.fields)
                .unwrap()
                .iter()
                .map(|field| operand(program, unit, Ref::Value(field.value)))
                .collect();
            return format!(
                "Construct/{:?}/{:?}/{}/[{}]",
                site.constructor,
                site.destination,
                site.arena_no_drop,
                fields.join(",")
            );
        }
        Ref::Closure(id) => {
            let site = &unit.tables.closures[id.index()];
            let captures: Vec<_> = site
                .captures
                .get(&unit.pools.captures)
                .unwrap()
                .iter()
                .map(|capture| {
                    format!(
                        "{:?}/{}",
                        capture.usage,
                        operand(program, unit, Ref::Value(capture.value))
                    )
                })
                .collect();
            return format!("Closure/{:?}/[{}]", site.destination, captures.join(","));
        }
        Ref::Values(range) => {
            return format!(
                "Values/[{}]",
                range
                    .get(&unit.pools.values)
                    .unwrap()
                    .iter()
                    .map(|v| operand(program, unit, Ref::Value(*v)))
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }
        Ref::Abi(id) => return format!("Abi/{}", abi_shape(program, id)),
        _ => {}
    }
    let ty = match reference {
        Ref::Value(id) => Some(unit.values[id.index()].ty),
        Ref::Slot(id) => Some(unit.slots[id.index()].ty),
        Ref::Address(id) => Some(unit.addresses[id.index()].ty),
        Ref::Type(id) => Some(id),
        _ => None,
    };
    let kind = format!("{reference:?}")
        .split('(')
        .next()
        .unwrap()
        .to_string();
    ty.map_or(kind.clone(), |ty| {
        format!("{kind}/{:?}", unit.types[ty.index()].repr)
    })
}

fn ready_features(program: &ReadyProgram, features: &mut BTreeSet<String>) {
    for unit in &program.units {
        let unit = unit.unit();
        for inst in &unit.instructions {
            let mut operands = Vec::new();
            inst.visit(&mut |reference| operands.push(operand(program, unit, reference)));
            // The opcode alone would merge signed/unsigned widths, arithmetic
            // operators, read modes and conversion modes into one feature.
            let detail = match inst {
                Inst::Unary { op, .. } => format!("/{op:?}"),
                Inst::Binary { op, .. } => format!("/{op:?}"),
                Inst::Compare { op, .. } => format!("/{op:?}"),
                Inst::Convert { mode, .. } => format!("/{mode:?}"),
                Inst::IntArith { op, mode, .. } => format!("/{op:?}/{mode:?}"),
                Inst::Read { mode, .. } => format!("/{mode:?}"),
                _ => String::new(),
            };
            features.insert(format!(
                "lir/{:?}{detail}/{}",
                inst.opcode(),
                vocabulary(&operands.join(","))
            ));
        }
        for term in &unit.terminators {
            let mut operands = Vec::new();
            term.visit(&mut |reference| operands.push(operand(program, unit, reference)));
            features.insert(format!(
                "lir/{:?}/{}",
                term.opcode(),
                vocabulary(&operands.join(","))
            ));
        }
        features.insert(format!(
            "abi/{}/{}",
            program.target.triple,
            abi_shape(program, unit.header.abi)
        ));
        for address in &unit.addresses {
            for projection in address.projection.get(&unit.pools.projections).unwrap() {
                features.insert(format!(
                    "projection/{}",
                    vocabulary(&format!("{projection:?}"))
                ));
            }
        }
    }
    for layout in &program.table.layouts {
        let children: Vec<_> = layout
            .fields
            .iter()
            .map(|field| {
                let child = program.table.layout(field.layout).unwrap();
                format!(
                    "{}@{}",
                    vocabulary(&format!("{:?}", child.kind)),
                    field.offset
                )
            })
            .collect();
        features.insert(format!(
            "layout/{}/size={:?}/align={}/fields=[{}]/tag={}/scan={}/drop={}/copy={}/storage={:?}",
            vocabulary(&format!("{:?}", layout.kind)),
            layout.size,
            layout.align,
            children.join(","),
            vocabulary(&format!("{:?}", layout.tag)),
            vocabulary(&format!("{:?}", layout.scan)),
            vocabulary(&format!("{:?}", layout.drop)),
            vocabulary(&format!("{:?}", layout.copy)),
            layout.storage
        ));
    }
}

/// Features across every distinct lowering choice in the exhaustive matrix.
/// The LLVM IR is emitted at O0 to retain calls optimization could delete.
/// Errors are returned as evidence, rather than pretending they cover Ready.
pub fn backend_features(input: &ValidatedNativeInput) -> (BTreeSet<String>, Vec<String>) {
    let mut features = BTreeSet::new();
    let mut errors = Vec::new();
    for target in TargetSpec::executed() {
        let arch = target.triple.clone();
        for (regions, elide, reuse, flat_walk) in [
            (true, false, false, true),
            (true, true, false, true),
            (true, true, true, true),
            (false, false, false, true),
            (false, true, false, true),
            (false, true, true, true),
            (true, true, true, false),
        ] {
            let options = Options {
                target: target.clone(),
                regions,
                elide,
                reuse,
                flat_walk,
                scoped_specialization: true,
                discharge: Default::default(),
                launch: Default::default(),
                validation: super::Validation::EveryStage,
            };
            match build(input, &options) {
                Err(error) => {
                    errors.push(format!(
                        "{arch}/{regions}/{elide}/{reuse}/{flat_walk}: {error}"
                    ));
                }
                Ok(program) => {
                    ready_features(&program, &mut features);
                    for stage in ["import", "placement", "perceus", "reuse", "ready"] {
                        features.insert(format!("validation/{arch}/{stage}/accepted"));
                    }
                    match emit_ir(&program, OptLevel::None) {
                        Err(error) => errors.push(format!(
                            "{arch}/{regions}/{elide}/{reuse}/{flat_walk}: {error}"
                        )),
                        Ok(ir) => {
                            features.insert(format!("validation/{arch}/emission/accepted"));
                            for line in ir.lines().filter(|line| line.contains("call ")) {
                                if let Some((_, symbol)) = line.split_once("@hirsel_rt_") {
                                    let symbol = symbol.split('(').next().unwrap();
                                    features.insert(format!("runtime/hirsel_rt_{symbol}"));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    errors.sort();
    errors.dedup();
    (features, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_retains_scalar_width_operator_and_runtime_calls() {
        hirsel_engine::with_compiler_stack(|| {
            let mut engine = hirsel_engine::QueryEngine::new(
                hirsel_engine::default_environment(),
                crate::compiler_adapters(),
            );
            let source = "pub fn main(io: Io, args: Args) -> Result[(), GrantError] {\n let x = add(3i32, 4i32);\n let s = io.stdout(io)?;\n let _ = out.print(s, if x == 7i32 { \"hello\" } else { \"wrong\" });\n Ok(())\n}\nfn add(x: i32, y: i32) -> i32 { x + y }\n";
            let file = engine.sources.add("coverage.hir", source.into()).unwrap();
            let checked =
                hirsel_engine::check_progress(&mut engine, file, &hirsel_engine::default_context())
                    .unwrap();
            assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
            let raw = engine.export_native_input().unwrap();
            let input = engine.import_native_input(raw).unwrap();
            let (features, errors) = backend_features(&input);
            assert!(errors.is_empty(), "{errors:?}");
            assert!(
                features
                    .iter()
                    .any(|f| f.starts_with("lir/Binary/Add/") && f.contains("Scalar(I32)")),
                "{features:?}"
            );
            assert!(
                features
                    .iter()
                    .any(|f| f.starts_with("lir/Call/") && f.contains("Scalar(I32)")),
                "{features:?}"
            );
            assert!(
                features.iter().any(|f| f.starts_with("runtime/hirsel_rt_")),
                "{features:?}"
            );
            assert!(
                features.iter().any(|f| f.starts_with("layout/Scalar(I32)")),
                "{features:?}"
            );
            assert!(features.contains("validation/aarch64-unknown-linux-gnu/ready/accepted"));
            assert_eq!(
                vocabulary("Scalar(U64) LayoutId(123)"),
                "Scalar(U64) LayoutId(#)"
            );
        });
    }
}
