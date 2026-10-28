//! Executable reachability over already C-validated diagnostic bodies. Like
//! native/link.rs's object closure, direct calls and named function references
//! retain their exact instances. Closure code, codec checks, static callable
//! relocations and runtime finalizers are dependencies too.
use crate::native::ReadyProgram;
use hirsel_lir::{
    ir::Inst,
    stage::{Ready, ValidatedUnit},
};
use hirsel_syntax::{ContentHash, DefPath};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn select(
    program: &ReadyProgram,
    units: &[&ValidatedUnit<Ready>],
    entry: &DefPath,
) -> Result<BTreeSet<ContentHash>, String> {
    let bodies = units
        .iter()
        .map(|unit| {
            let body = unit.unit();
            (
                (body.header.definition.clone(), body.header.instance),
                *unit,
            )
        })
        .collect::<BTreeMap<_, _>>();
    if bodies.len() != units.len() || program.summaries.len() != units.len() {
        return Err("ownership executable scope differs from checked bodies".into());
    }
    let instance = *program
        .functions
        .get(entry)
        .ok_or("ownership executable has no checked entry")?;
    let mut pending = vec![(entry.clone(), instance)];
    for instance in program.finalizers.values() {
        let summary = program
            .summaries
            .iter()
            .find(|summary| summary.instance == *instance)
            .ok_or("ownership finalizer has no checked summary")?;
        pending.push((summary.definition.clone(), *instance));
    }
    let mut selected = BTreeSet::new();
    while let Some((definition, instance)) = pending.pop() {
        let body = bodies
            .get(&(definition.clone(), instance))
            .ok_or_else(|| {
                format!("ownership executable dependency has no C-admitted body: {definition}")
            })?
            .unit();
        if !selected.insert(instance) {
            continue;
        }
        let summary = program
            .summaries
            .iter()
            .find(|summary| summary.definition == definition && summary.instance == instance)
            .ok_or("ownership executable body has no checked summary")?;
        let mut dependency = |definition: &DefPath,
                              instance: Option<ContentHash>|
         -> Result<(), String> {
            let instance = instance.ok_or("ownership executable dependency is unspecialized")?;
            pending.push((definition.clone(), instance));
            Ok(())
        };
        for symbol in summary.calls.iter().chain(&summary.references) {
            dependency(&symbol.definition, symbol.instance)?;
        }
        for (path, instance) in summary.glue.entries.iter().chain(&summary.glue.callbacks) {
            dependency(path, *instance)?;
        }
        for check in summary.glue.codecs.iter().flat_map(|codec| &codec.checks) {
            dependency(&check.definition, check.instance)?;
        }
        for instruction in &body.instructions {
            if let Inst::Closure { closure, .. } = *instruction {
                let code = body.tables.closures[closure.index()]
                    .code
                    .ok_or("ownership executable closure has no code")?;
                let symbol = &body.tables.symbols[code.symbol.index()];
                dependency(&symbol.definition, symbol.instance)?;
            }
        }
    }
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_engine::DefinitionQueries;

    fn fixture() -> (ReadyProgram, DefPath, DefPath) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("main.hir"),
            r#"
resource type Guard { value:u64 }
drop(x) {}
fn add(x:u64)->u64 { x+1u64 }
fn ignored(f:fn()->u64 uses {})->u64 uses {f} {
 let guard=Guard(value:1u64)
 f()
}
pub fn main()->u64 {
 let named=add
 let callback=fn(x:u64)->u64 { add(x) }
 named(callback(1u64))
}
"#,
        )
        .unwrap();
        let manifest = dir.path().join("hirsel.json");
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"closure","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        let mut loaded = crate::load_program(&manifest).unwrap();
        loaded.check_for_execution().unwrap();
        let entry = loaded.entry.clone();
        let mut ignored = entry.clone();
        ignored.0.name = "ignored".into();
        let input = loaded
            .engine
            .native_input_for(&[entry.clone(), ignored.clone()])
            .unwrap();
        let mut options =
            crate::native::Options::new(hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu());
        options.elide = false;
        let program = crate::native::build(&input, &options).unwrap();
        (program, entry, ignored)
    }

    #[test]
    fn executable_roots_keep_real_code_and_finalizers_and_refuse_missing_bodies() {
        hirsel_engine::with_compiler_stack(|| {
            let (program, entry, ignored) = fixture();
            let units = program.units.iter().collect::<Vec<_>>();
            let selected = select(&program, &units, &entry).unwrap();
            let excluded = program.functions[&ignored];
            assert!(!selected.contains(&excluded));
            assert!(selected.contains(&program.functions[&entry]));
            assert!(!program.finalizers.is_empty());
            assert!(program.finalizers.values().all(|id| selected.contains(id)));
            assert!(units.iter().any(|unit| {
                unit.unit()
                    .instructions
                    .iter()
                    .any(|inst| matches!(inst, Inst::FunctionRef { .. }))
            }));
            assert!(units.iter().any(|unit| {
                unit.unit()
                    .instructions
                    .iter()
                    .any(|inst| matches!(inst, Inst::Closure { .. }))
            }));
            for missing in &selected {
                let mut incomplete = program.clone();
                incomplete
                    .units
                    .retain(|unit| unit.unit().header.instance != *missing);
                incomplete
                    .summaries
                    .retain(|summary| summary.instance != *missing);
                let units = incomplete.units.iter().collect::<Vec<_>>();
                assert!(
                    select(&incomplete, &units, &entry).is_err(),
                    "missing {missing:?}"
                );
            }
        });
    }

    #[test]
    fn declared_entries_callbacks_codec_checks_and_static_references_cannot_be_omitted() {
        hirsel_engine::with_compiler_stack(|| {
            let (program, entry, ignored) = fixture();
            let target = program.functions[&ignored];
            for route in 0..5 {
                let mut named = program.clone();
                let summary = named
                    .summaries
                    .iter_mut()
                    .find(|summary| summary.definition == entry)
                    .unwrap();
                let symbol = hirsel_lir::ir::Symbol {
                    kind: hirsel_lir::ir::SymbolKind::Function,
                    definition: ignored.clone(),
                    instance: Some(target),
                };
                match route {
                    0 => summary.calls.push(symbol),
                    // Static function relocations enter this same reference list
                    // through UnitSummary::add_static_references.
                    1 => summary.references.push(symbol),
                    2 => {
                        summary.glue.entries.insert((ignored.clone(), Some(target)));
                    }
                    3 => {
                        summary
                            .glue
                            .callbacks
                            .insert((ignored.clone(), Some(target)));
                    }
                    _ => summary.glue.codecs.push(hirsel_lir::glue::GlueCodec {
                        key: hirsel_lir::glue::CodecKey(ContentHash::of_bytes(b"closure-control")),
                        plan: hirsel_lir::codec::CodecPlan {
                            descriptor: vec![],
                            nodes: vec![],
                            root: hirsel_lir::codec::CodecNodeId(0),
                            bytes: hirsel_lir::codec::CodecNodeId(0),
                            result: None,
                            checks: vec![],
                        },
                        checks: vec![hirsel_lir::glue::GlueCheck {
                            definition: ignored.clone(),
                            instance: Some(target),
                            abi: program
                                .summaries
                                .iter()
                                .find(|s| s.instance == target)
                                .unwrap()
                                .abi,
                        }],
                    }),
                }
                assert!(
                    select(&named, &named.units.iter().collect::<Vec<_>>(), &entry)
                        .unwrap()
                        .contains(&target),
                    "route {route}"
                );
                named
                    .units
                    .retain(|unit| unit.unit().header.instance != target);
                named.summaries.retain(|summary| summary.instance != target);
                assert!(
                    select(&named, &named.units.iter().collect::<Vec<_>>(), &entry).is_err(),
                    "route {route}"
                );
            }
        });
    }
}
