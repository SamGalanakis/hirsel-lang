//! What both canaries share: import the export under the environment it
//! names, resolve places and locations, and write the findings sidecar.

#[cfg(test)]
hirsel_guard::install!();

use hirsel_proof_export::findings::{Finding, Findings};
use hirsel_proof_export::{Anchor, DefinitionSection, Export, Location};
use hirsel_tir::{Body, LocalId, Operand, PlaceId};
use hirsel_types::TypeKind;
use std::process::ExitCode;

pub mod taint;
pub mod units;

/// Read and import `path`, pinned to the compiler its header names (PS-7).
pub fn load(path: &str) -> Result<Export, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let header = hirsel_proof_export::read_header(&text).map_err(|e| format!("{path}: {e}"))?;
    hirsel_proof_export::import(
        &text,
        &header.environment,
        &hirsel_types::TypeInterner::new(None),
    )
    .map_err(|e| format!("{path}: {e}"))
}

/// The local a place is rooted at.
pub fn root(body: &Body, place: PlaceId) -> LocalId {
    body.places[place.0 as usize].root
}

pub fn operand_root(body: &Body, operand: &Operand) -> LocalId {
    root(body, operand.place)
}

/// A span of `section`'s TIR as a source location.
pub fn location(section: &DefinitionSection, span: hirsel_span::Span) -> Location {
    section
        .locations
        .get(span.start as usize)
        .cloned()
        .unwrap_or_else(Location::unknown)
}

/// Whether `operand` is a `core@1/Stdout` value.
pub fn is_stdout(export: &Export, body: &Body, operand: &Operand) -> bool {
    let ty = body.places[operand.place.0 as usize].ty;
    export.types.get(ty).is_some_and(|t| {
        matches!(&t.kind, TypeKind::Named { definition, .. }
            if definition.path().0.package.name == "core" && definition.path().0.name == "Stdout")
    })
}

/// Each `@analysis(name: payload)` on a parameter of an exported
/// definition, as (section index, entry-body local, payload).
pub fn parameters<'e>(export: &'e Export, name: &str) -> Vec<(usize, LocalId, &'e [String])> {
    export
        .annotations
        .iter()
        .filter(|a| a.name == name)
        .filter_map(|a| {
            let Anchor::Parameter(i) = a.anchor else {
                return None;
            };
            let index = export.definitions.iter().position(|d| d.path == a.path)?;
            let tir = &export.definitions[index].tir;
            let local = *tir.bodies[tir.entry.0 as usize]
                .parameters
                .get(i as usize)?;
            Some((index, local, a.payload.as_slice()))
        })
        .collect()
}

/// Read the export at `input`, analyze it and write the findings sidecar to
/// `output`: what each canary command does.
pub fn run(
    analyzer: &str,
    analyze: fn(&Export) -> Vec<Finding>,
    input: &str,
    output: &str,
) -> Result<(), String> {
    let export = load(input)?;
    let findings = Findings {
        export: export.digest(),
        analyzer: format!("{analyzer}@{}", env!("CARGO_PKG_VERSION")),
        findings: analyze(&export),
    };
    std::fs::write(output, findings.to_json()).map_err(|e| format!("{output}: {e}"))
}

/// `<tool> <export> -o <findings>`: [`run`]. Exit 2 on bad usage or input;
/// findings never change the exit status.
pub fn main(analyzer: &str, analyze: fn(&Export) -> Vec<Finding>) -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, flag, output] = args.as_slice() else {
        eprintln!("usage: {analyzer} <export> -o <findings>");
        return ExitCode::from(2);
    };
    if flag != "-o" {
        eprintln!("usage: {analyzer} <export> -o <findings>");
        return ExitCode::from(2);
    }
    match run(analyzer, analyze, input, output) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{analyzer}: {error}");
            ExitCode::from(2)
        }
    }
}
