//! Stable snapshots omit TypeId provenance, hash/interner allocation identities.
use hirsel_tir::*;
/// Deterministic TIR listing. Arena IDs are local indices; a type is printed by
/// its index in the definition's canonical type table, never by session id.
pub fn definition(definition: &Definition) -> String {
    let table: std::collections::HashMap<hirsel_types::TypeId, usize> = definition
        .types
        .iter()
        .enumerate()
        .map(|(i, t)| (*t, i))
        .collect();
    let index = |t: &hirsel_types::TypeId| table.get(t).copied().unwrap_or(usize::MAX);
    let mut output = format!(
        "definition {} entry b{}\n",
        definition.path.path().0.name, definition.entry.0
    );
    for receipt in &definition.protocol_associations {
        output.push_str(&format!(
            "  protocol {:?} {:?} body {} open {} next {}\n",
            receipt.association.source,
            receipt.association.fingerprint,
            receipt.body.0,
            receipt.open.0,
            receipt.next.0
        ));
    }
    for (n, body) in definition.bodies.iter().enumerate() {
        output.push_str(&format!(
            "body {n} result t{} parameters {:?}\n",
            index(&body.result),
            body.parameters
        ));
        for (i, local) in body.locals.iter().enumerate() {
            output.push_str(&format!(
                "  slot {i}: t{} {:?} scope {}\n",
                index(&local.ty),
                local.storage,
                local.scope.0
            ));
        }
        for (i, place) in body.places.iter().enumerate() {
            output.push_str(&format!(
                "  place {i}: slot {} {:?} t{}{}\n",
                place.root.0,
                place.projection,
                index(&place.ty),
                place
                    .loan
                    .map(|id| format!(" through loan {}", id.0))
                    .unwrap_or_default()
            ));
        }
        for (i, rv) in body.rvalues.iter().enumerate() {
            let text = match rv {
                Rvalue::Shape(ty) => format!("Shape(t{})", index(&ty)),
                _ => format!("{rv:?}"),
            };
            output.push_str(&format!("  rvalue {i}: {text}\n"));
        }
        for i in 0..body.cleanups.len() {
            output.push_str(&format!(
                "  cleanup {i}: {:?}\n",
                body.cleanup_actions(CleanupId(i as u32))
                    .collect::<Vec<_>>()
            ));
        }
        for (i, table) in body.matches.iter().enumerate() {
            output.push_str(&format!("  match {i}: scrutinee {:?}\n", table.scrutinee));
            for (node, pattern) in table.patterns.iter().enumerate() {
                output.push_str(&format!("    pattern {node}: {pattern:?}\n"));
            }
            for (arm, row) in table.arms.iter().enumerate() {
                output.push_str(&format!("    arm {arm}: {row:?}\n"));
            }
        }
        for (i, block) in body.blocks.iter().enumerate() {
            output.push_str(&format!("  block {i}:\n"));
            for instruction in &block.instructions {
                output.push_str(&format!(
                    "    {:?} fail {} @{}..{}\n",
                    instruction.kind,
                    instruction.on_failure.0,
                    instruction.span.start,
                    instruction.span.end
                ));
            }
            let terminator = match &block.terminator {
                Terminator::Call {
                    callee:
                        Callee::Definition {
                            path,
                            arguments: generics,
                        },
                    arguments,
                    returns: Some(CallReturn { destination, next }),
                    on_failure,
                } if !generics.is_empty() => {
                    let generics = generics
                        .iter()
                        .map(|g| match g {
                            hirsel_types::GenericArgument::Type(t) => format!("t{}", index(&t)),
                            hirsel_types::GenericArgument::Size(s) => format!("{s:?}"),
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!(
                        "Call {{ callee: {path:?}[{generics}], arguments: {arguments:?}, destination: {destination:?}, next: {next:?}, on_failure: {on_failure:?} }}"
                    )
                }
                _ => format!("{:?}", block.terminator),
            };
            output.push_str(&format!(
                "    {terminator} @{}..{}\n",
                block.span.start, block.span.end
            ));
        }
    }
    output
}
