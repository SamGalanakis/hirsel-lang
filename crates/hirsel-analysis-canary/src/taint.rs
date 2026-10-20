//! Taint canary: a parameter annotated `@analysis(taint: secret)` must not
//! reach an argument of a call that also passes a `core@1/Stdout`. Explicit
//! flows through assignments, bindings, matches, calls and returns.

use crate::{is_stdout, location, operand_root, parameters, root};
use hirsel_proof_export::Export;
use hirsel_proof_export::findings::{Finding, Severity};
use hirsel_tir::{Callee, InstructionKind, LocalId, Rvalue, Terminator};
use std::collections::BTreeSet;

/// (section, body, local)
type Slot = (usize, u32, u32);

/// Every finding over `export`.
pub fn analyze(export: &Export) -> Vec<Finding> {
    let mut tainted: BTreeSet<Slot> = parameters(export, "taint")
        .into_iter()
        .filter(|(_, _, payload)| payload == &["secret"])
        .map(|(section, local, _)| (section, export.definitions[section].tir.entry.0, local.0))
        .collect();
    let mut returns: BTreeSet<usize> = BTreeSet::new();
    let index =
        |path: &hirsel_syntax::DefPath| export.definitions.iter().position(|d| &d.path == path);
    loop {
        let before = (tainted.len(), returns.len());
        for (s, section) in export.definitions.iter().enumerate() {
            for (b, body) in section.tir.bodies.iter().enumerate() {
                let b = b as u32;
                let is =
                    |tainted: &BTreeSet<Slot>, local: LocalId| tainted.contains(&(s, b, local.0));
                let mut flows: Vec<(Vec<LocalId>, LocalId)> = vec![];
                for block in &body.blocks {
                    for instruction in &block.instructions {
                        match &instruction.kind {
                            InstructionKind::Assign { destination, value } => {
                                let sources = match &body.rvalues[value.0 as usize] {
                                    Rvalue::Use(o) | Rvalue::Unary { value: o, .. } => vec![o],
                                    Rvalue::Binary { left, right, .. } => vec![left, right],
                                    Rvalue::Construct { fields, .. } => {
                                        fields.iter().map(|f| &f.value).collect()
                                    }
                                    Rvalue::Closure { captures, .. } => captures.iter().collect(),
                                    _ => vec![],
                                };
                                flows.push((
                                    sources.into_iter().map(|o| operand_root(body, o)).collect(),
                                    root(body, *destination),
                                ));
                            }
                            InstructionKind::Replace {
                                destination, value, ..
                            }
                            | InstructionKind::Snapshot { destination, value } => {
                                flows.push((
                                    vec![operand_root(body, value)],
                                    root(body, *destination),
                                ));
                            }
                            InstructionKind::BindOrigin { source, place, .. } => {
                                flows.push((vec![root(body, *source)], root(body, *place)));
                            }
                            InstructionKind::BindFields { source, fields } => {
                                for field in fields {
                                    flows.push((vec![root(body, *source)], root(body, *field)));
                                }
                            }
                            _ => {}
                        }
                    }
                    match &block.terminator {
                        Terminator::Call {
                            callee,
                            arguments,
                            returns: result,
                            ..
                        } => {
                            let sources: Vec<_> = arguments
                                .iter()
                                .map(|a| operand_root(body, &a.value))
                                .collect();
                            let target = match callee {
                                Callee::Definition { path, .. } => index(path),
                                Callee::Indirect { .. } => None,
                            };
                            if let Some(t) = target {
                                let tir = &export.definitions[t].tir;
                                let entry = &tir.bodies[tir.entry.0 as usize];
                                for a in arguments {
                                    if is(&tainted, operand_root(body, &a.value))
                                        && let Some(p) =
                                            entry.parameters.get(a.parameter.0 as usize)
                                    {
                                        tainted.insert((t, tir.entry.0, p.0));
                                    }
                                }
                            }
                            if let Some(result) = result {
                                let destination = root(body, result.destination);
                                match target {
                                    Some(t) if returns.contains(&t) => {
                                        tainted.insert((s, b, destination.0));
                                    }
                                    Some(_) => {}
                                    // No body to follow: the result carries its arguments.
                                    None => flows.push((sources, destination)),
                                }
                            }
                        }
                        Terminator::Return { value, .. }
                            if b == section.tir.entry.0
                                && is(&tainted, operand_root(body, value)) =>
                        {
                            returns.insert(s);
                        }
                        _ => {}
                    }
                }
                for table in &body.matches {
                    let scrutinee = root(body, table.scrutinee);
                    for pattern in &table.patterns {
                        flows.push((vec![scrutinee], root(body, pattern.place)));
                    }
                }
                for (sources, destination) in flows {
                    if sources.iter().any(|l| is(&tainted, *l)) {
                        tainted.insert((s, b, destination.0));
                    }
                }
            }
        }
        if (tainted.len(), returns.len()) == before {
            break;
        }
    }
    let mut findings = vec![];
    for (s, section) in export.definitions.iter().enumerate() {
        for (b, body) in section.tir.bodies.iter().enumerate() {
            for block in &body.blocks {
                let Terminator::Call { arguments, .. } = &block.terminator else {
                    continue;
                };
                let writes = arguments.iter().any(|a| is_stdout(section, body, &a.value));
                let leaks = arguments
                    .iter()
                    .any(|a| tainted.contains(&(s, b as u32, operand_root(body, &a.value).0)));
                if writes && leaks {
                    let span = block
                        .instructions
                        .last()
                        .map_or(block.span, |instruction| instruction.span);
                    findings.push(Finding {
                        analysis: "taint".into(),
                        code: "TAINT-1".into(),
                        severity: Severity::Error,
                        definition: section.path.to_string(),
                        location: location(section, span),
                        message: "a secret parameter reaches a stdout write".into(),
                        related: vec![],
                    });
                }
            }
        }
    }
    findings
}
