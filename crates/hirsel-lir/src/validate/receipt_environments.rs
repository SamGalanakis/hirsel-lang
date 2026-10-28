//! Source correspondence for ordinary environments used by receipt-aware code.
use super::{Cx, Report, Site, ViolationKind as K};
use crate::{ir::*, sealed_payload_source::*, stage::Stage};

pub(super) fn check<S: Stage>(cx: &Cx<'_, S>, report: &mut Report) {
    let unit = cx.unit;
    let Some(sources) = cx
        .operations
        .seal_environments(&unit.header.definition, unit.header.instance)
    else {
        return;
    };
    let captures = sources
        .entry
        .as_ref()
        .is_some_and(|source| source.check_ordinary_entry(unit, cx.layouts.0).is_ok());
    let origins = super::sealed_payload::producers(
        unit,
        &cx.layouts.0.fn_abis[unit.header.abi.0 as usize],
        captures,
    );
    let paths = super::sealed_payload::projection::Projections::new(unit);
    for site in &unit.tables.closures {
        let anchor = &unit.tables.source_anchors[site.anchor.index()];
        let matching = sources
            .constructions
            .iter()
            .filter(|source| &source.payload.recipe.source == anchor)
            .collect::<Vec<_>>();
        if matching.is_empty() {
            let selected_capture_entry = site
                .code
                .and_then(|code| {
                    let symbol = &unit.tables.symbols[code.symbol.index()];
                    cx.operations
                        .seal_environments(&symbol.definition, symbol.instance?)?
                        .entry
                })
                .is_some_and(|entry| !entry.captures.is_empty());
            if selected_capture_entry {
                report.add(
                    Site::UNIT,
                    K::Sealed("ordinary captured closure lacks its original construction".into()),
                );
            }
            continue;
        }
        let checked = (|| -> Result<(), &'static str> {
            let [source] = matching.as_slice() else {
                return Err("ordinary closure has ambiguous original construction");
            };
            source.check_structure()?;
            let code = site
                .code
                .ok_or("ordinary closure lacks selected original code")?;
            let symbol = &unit.tables.symbols[code.symbol.index()];
            if symbol.definition != source.original.definition
                || symbol.instance != Some(source.instance)
                || site.scoped != (source.context == SourceClosureContext::Scoped)
                || crate::LayoutKeys::rooted(cx.layouts.0, site.environment).0 != source.base_layout
            {
                return Err("ordinary closure differs from original selected code or environment");
            }
            let ordinary = site
                .captures
                .get(&unit.pools.captures)
                .ok_or("ordinary closure operands are missing")?;
            if ordinary.len() != source.captures.len() {
                return Err("ordinary closure lacks complete original captures");
            }
            for ((actual, usage), original) in ordinary
                .iter()
                .zip(&source.captures)
                .zip(&source.payload.recipe.inputs)
            {
                let copied = unit.instructions.iter().any(|inst| matches!(inst,
                    Inst::Copy { result, .. } | Inst::CopyValue { result, .. } if *result == actual.value));
                if actual.usage != *usage
                    && !(*usage == CaptureUse::Copy && actual.usage == CaptureUse::Take && copied)
                {
                    return Err("ordinary closure differs from original capture use");
                }
                if !paths.matches(actual.value, &original.producer, &origins) {
                    return Err("ordinary closure capture producer differs from original path");
                }
            }
            Ok(())
        })();
        if let Err(error) = checked {
            report.add(Site::UNIT, K::Sealed(error.into()));
        }
    }
    for call in &unit.tables.calls {
        let Callee::Bound { code, closure } = call.callee else {
            continue;
        };
        let symbol = &unit.tables.symbols[code.index()];
        let Some(instance) = symbol.instance else {
            continue;
        };
        let Some(entry) = cx
            .operations
            .seal_environments(&symbol.definition, instance)
            .and_then(|sources| sources.entry)
        else {
            continue;
        };
        if entry.captures.is_empty() {
            continue;
        }
        let actual = &origins[closure.index()];
        let selected = !actual.is_empty()
            && actual.iter().all(|origin| {
                let PayloadProducer::Closure(anchor) = origin else {
                    return false;
                };
                sources.constructions.iter().any(|source| {
                    &source.payload.recipe.source == anchor
                        && source.original.definition == symbol.definition
                        && source.instance == instance
                        && source.base_layout == entry.base_layout
                })
            });
        if !selected {
            report.add(
                Site::UNIT,
                K::Sealed(
                    "bound capture entry lacks its actual original environment construction".into(),
                ),
            );
        }
    }
}
