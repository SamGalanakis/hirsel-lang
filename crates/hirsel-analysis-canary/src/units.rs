//! Units canary: parameters annotated `@analysis(unit: <unit>)` carry their
//! unit through copies; adding or subtracting two different units is
//! reported where it happens. Products and quotients have no unit here.

use crate::{location, operand_root, parameters, root};
use hirsel_proof_export::Export;
use hirsel_proof_export::findings::{Finding, Severity};
use hirsel_syntax::BinaryOp;
use hirsel_tir::{InstructionKind, Rvalue};
use std::collections::BTreeMap;

/// Every finding over `export`.
pub fn analyze(export: &Export) -> Vec<Finding> {
    let mut findings = vec![];
    for (s, section) in export.definitions.iter().enumerate() {
        let entry = section.tir.entry;
        let body = &section.tir.bodies[entry.0 as usize];
        // Instructions run in arena order within a block and lowering appends
        // in source order, so one forward pass sees each copy after its source.
        let mut units: BTreeMap<u32, String> = parameters(export, "unit")
            .into_iter()
            .filter(|(section, _, _)| *section == s)
            .map(|(_, local, payload)| (local.0, payload.concat()))
            .collect();
        for block in &body.blocks {
            for instruction in &block.instructions {
                let (destination, unit) = match &instruction.kind {
                    InstructionKind::Assign { destination, value } => {
                        let unit = match &body.rvalues[value.0 as usize] {
                            Rvalue::Use(o) => units.get(&operand_root(body, o).0).cloned(),
                            Rvalue::Binary {
                                op: op @ (BinaryOp::Add | BinaryOp::Sub),
                                left,
                                right,
                            } => {
                                let left = units.get(&operand_root(body, left).0);
                                let right = units.get(&operand_root(body, right).0);
                                match (left, right) {
                                    (Some(l), Some(r)) if l != r => {
                                        findings.push(Finding {
                                            analysis: "unit".into(),
                                            code: "UNIT-1".into(),
                                            severity: Severity::Error,
                                            definition: section.path.to_string(),
                                            location: location(section, instruction.span),
                                            message: format!(
                                                "{} of {l} and {r}",
                                                if *op == BinaryOp::Add {
                                                    "sum"
                                                } else {
                                                    "difference"
                                                }
                                            ),
                                            related: vec![],
                                        });
                                        None
                                    }
                                    (Some(u), _) | (_, Some(u)) => Some(u.clone()),
                                    _ => None,
                                }
                            }
                            _ => None,
                        };
                        (root(body, *destination), unit)
                    }
                    InstructionKind::BindOrigin { source, place, .. } => (
                        root(body, *place),
                        units.get(&root(body, *source).0).cloned(),
                    ),
                    InstructionKind::Replace {
                        destination, value, ..
                    } => (
                        root(body, *destination),
                        units.get(&operand_root(body, value).0).cloned(),
                    ),
                    _ => continue,
                };
                if let Some(unit) = unit {
                    units.insert(destination.0, unit);
                }
            }
        }
    }
    findings
}
