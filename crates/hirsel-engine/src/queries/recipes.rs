//! Error-only declaration recipes. A private source snapshot checks the exact
//! proposed edits through the ordinary pipeline; no promise enters the caller's
//! engine and no speculative diagnostic is cached as a front product.
use super::*;
use hirsel_syntax::{
    ExprKind, PassingMode, StmtKind, TypeArgument, TypeExprId, TypeExprKind, TypeModifier,
    Visibility,
};
use hirsel_types::{GenericArgument, TypeId, TypeKind, Uniqueness};

struct Candidate {
    edits: Vec<(Span, String)>,
    site: Span,
    note: String,
}

pub(crate) fn offer(
    engine: &mut QueryEngine,
    file: FileId,
    context: &FileContext,
    lowering: LoweringPolicy,
    diagnostics: &mut [Diagnostic],
) {
    // A dependency failure can propagate through every file in a check. A
    // successful edit of an unrelated file proves nothing about that failure,
    // and distinct header notes would keep its copies from being deduplicated.
    if !diagnostics.iter().any(|d| {
        engine.sources.resolve(d.primary).file == file
            && matches!(d.rule.as_str(), "MODE-2" | "LANG-7" | "LANG-2" | "CONC-5")
    }) {
        return;
    }
    let Ok(parsed) = engine.parse_file(file, context) else {
        return;
    };
    let typed: BTreeMap<_, _> = parsed
        .value
        .items
        .iter()
        .filter_map(|syntax| {
            // Generic mode inference refines the structural check's rows.
            // Prefer that ordinary front result when proposing an inferred row.
            let body = engine
                .mode_definition(&syntax.item.definition)
                .ok()
                .map(|body| (*body.value.typed).clone())
                .or_else(|| {
                    engine
                        .check_definition(&syntax.item.definition)
                        .ok()
                        .map(|body| body.value)
                })?;
            Some((syntax.item.definition.clone(), body))
        })
        .collect();
    let direct_calls: BTreeSet<_> = typed
        .values()
        .flat_map(|body| &body.checked.calls)
        .filter_map(|call| match &call.target {
            hirsel_types::CallTarget::Definition(path) => Some(path.clone()),
            _ => None,
        })
        .collect();
    let mut candidates = vec![];
    for syntax in &parsed.value.items {
        let tree = &syntax.tree;
        if let ItemKind::Function {
            signature,
            body: Some(_),
            abi: None,
        } = &syntax.item.kind
        {
            if let Some(result) = signature.result {
                if syntax.item.visibility == Visibility::Public
                    && direct_calls.contains(&syntax.item.definition)
                {
                    for edits in result_edits(tree, result) {
                        let row =
                            edited_row(engine, tree.types[result.0 as usize].origin.span, &edits);
                        candidates.push(Candidate {
                            edits, site: syntax.item.name_span,
                            note: format!("R1 producer header: declare `{} -> {row}` at this producer's header; its body and this call check with that result contract (no copy)", syntax.item.name),
                        });
                    }
                }
                if !signature.generics.is_empty() && direct_calls.contains(&syntax.item.definition)
                {
                    let parameters: Vec<_> = signature
                        .parameters
                        .iter()
                        .filter(|p| p.passing == PassingMode::Take)
                        .collect();
                    let mut edits = vec![];
                    let mut inputs = vec![];
                    for p in parameters {
                        let span = tree.types[p.ty.0 as usize].origin.span;
                        let Some(text) = engine.sources.slice(span) else {
                            continue;
                        };
                        if !text.starts_with("unique ") {
                            edits.push((span, format!("unique {text}")));
                            inputs.push(format!(
                                "take {}: unique {text}",
                                p.name.as_ref().map(|n| n.as_str()).unwrap_or("_")
                            ));
                        }
                    }
                    let result_span = tree.types[result.0 as usize].origin.span;
                    if let Some(text) = engine.sources.slice(result_span)
                        && !text.starts_with("unique ")
                        && !edits.is_empty()
                        && matches!(tree.types[result.0 as usize].kind, TypeExprKind::Named { ref arguments, .. } if !arguments.is_empty())
                    {
                        let note = if syntax.item.visibility == Visibility::Private {
                            format!(
                                "R2 generic wrapper: declare `{}` on `{}`; its body proves the result evidence at same-module direct calls (no copy)",
                                inputs.join(", "),
                                syntax.item.name
                            )
                        } else {
                            edits.push((result_span, format!("unique {text}")));
                            format!(
                                "R2 generic wrapper: declare `{}` and `-> unique {text}` on `{}`; the edited wrapper and its caller check (no copy)",
                                inputs.join(", "),
                                syntax.item.name
                            )
                        };
                        candidates.push(Candidate {
                            edits,
                            site: syntax.item.name_span,
                            note,
                        });
                    }
                }
            }
        }
        // Even a rejected type check retains elaborated expressions. They are
        // used only to propose text; the fresh check below is the authority.
        let typed = typed.get(&syntax.item.definition);
        let spelling = |ty| {
            typed
                .map(|typed| {
                    hirsel_type_analysis::spelling::type_spelling(
                        &engine.types,
                        ty,
                        &typed.checked.resolution.generic_declarations,
                        &context.package,
                        true,
                    )
                })
                .unwrap_or_default()
        };
        for (index, expression) in tree.expressions.iter().enumerate() {
            let ExprKind::Closure { signature, body } = &expression.kind else {
                continue;
            };
            // Task operations can keep a literal in a local before await/select.
            // Limit these recipes to a file actually using a task boundary.
            let task_boundary = typed.is_some_and(|typed| typed.checked.calls.iter().any(|call| {
                matches!(&call.target, hirsel_types::CallTarget::Definition(path)
                    if ["task.spawn", "task.async", "task.select", "task.await"].iter().any(|name| path.is_core(name)))
            })) || tree.expressions.iter().any(|e| match &e.kind {
                ExprKind::Path(path) => {
                    let names = path.segments.iter().map(|s| s.as_str()).collect::<Vec<_>>();
                    matches!(names.as_slice(), ["task", "async" | "spawn" | "select" | "await"])
                }
                ExprKind::Field { base, field } => {
                    matches!(field.as_str(), "async" | "spawn" | "select" | "await")
                        && matches!(&tree.expressions[base.0 as usize].kind,
                            ExprKind::Path(path) if path.segments.len() == 1 && path.segments[0].as_str() == "task")
                }
                _ => false,
            });
            if !task_boundary {
                continue;
            }
            if let Some(result) = signature.result {
                for edits in result_edits(tree, result) {
                    let row = edited_row(engine, tree.types[result.0 as usize].origin.span, &edits);
                    candidates.push(Candidate { edits, site: expression.origin.span,
                        note: format!("R3 closure literal result rows: write `-> {row}` on this literal at task.async/spawn/select/await; the literal and task boundary check (no copy)") });
                }
            } else if let Some(elaboration) = typed.and_then(|typed| {
                typed
                    .checked
                    .expressions
                    .iter()
                    .find(|e| e.expression.0 as usize == index)
            }) && let Some(row) = engine.types.get(elaboration.ty)
                && let TypeKind::Function(function) = &row.kind
            {
                for result in strengthened_rows(engine, function.result) {
                    let text = spelling(result);
                    let mut site = tree.expressions[body.0 as usize].origin.span;
                    site.end = site.start;
                    candidates.push(Candidate { edits: vec![(site, format!("-> {text} "))], site: expression.origin.span,
                        note: format!("R3 closure literal result rows: write `-> {text}` on this literal at task.async/spawn/select/await; the literal and task boundary check (no copy)") });
                }
            }
            // A generic task call may stop at LANG-2 before it can elaborate
            // the literal's result. Its enclosing declared result is another
            // candidate, never evidence: only the complete edited check admits it.
            if signature.result.is_none()
                && let ItemKind::Function {
                    signature: enclosing,
                    ..
                } = &syntax.item.kind
                && let Some(result) = enclosing.result
                && let Some(row) = engine
                    .sources
                    .slice(tree.types[result.0 as usize].origin.span)
            {
                let row = if row.starts_with("unique ") {
                    row.to_owned()
                } else {
                    format!("unique {row}")
                };
                let mut site = tree.expressions[body.0 as usize].origin.span;
                site.end = site.start;
                candidates.push(Candidate { edits: vec![(site, format!("-> {row} "))], site: expression.origin.span,
                    note: format!("R3 closure literal result rows: write `-> {row}` on this literal at task.async/spawn/select/await; the literal and task boundary check (no copy)") });
            }
        }
        let Some(typed) = typed else { continue };
        for statement in &tree.statements {
            let StmtKind::Let {
                pattern,
                ty: None,
                value,
                ..
            } = statement.kind
            else {
                continue;
            };
            let Some(elaboration) = typed
                .checked
                .expressions
                .iter()
                .find(|e| e.expression == value)
            else {
                continue;
            };
            let Some(row) = engine.types.get(elaboration.ty) else {
                continue;
            };
            let TypeKind::Named { arguments, .. } = &row.kind else {
                continue;
            };
            if !arguments.iter().any(|a| matches!(a, GenericArgument::Type(t) if engine.types.get(*t).is_some_and(|r| r.modes.uniqueness == Uniqueness::Unique))) { continue }
            let inferred = spelling(elaboration.ty);
            let mut weaker_row = row.clone();
            if let TypeKind::Named { arguments, .. } = &mut weaker_row.kind {
                for argument in arguments.iter_mut() {
                    if let GenericArgument::Type(t) = argument
                        && let Some(payload) = engine.types.get(*t)
                        && payload.modes.uniqueness == Uniqueness::Unique
                    {
                        let mut payload = payload.clone();
                        payload.modes.uniqueness = Uniqueness::Aliased;
                        *t = engine.types.intern(payload);
                    }
                }
            }
            let weaker = spelling(engine.types.intern(weaker_row));
            let mut site = tree.patterns[pattern.0 as usize].origin.span;
            site.start = site.end;
            candidates.push(Candidate { edits: vec![(site, format!(": {weaker}"))], site: statement.origin.span,
                note: format!("R4 inferred nested row: inference chose `{inferred}`; explicitly annotate this binding as `{weaker}` to use the weaker nested row; the annotation and its uses check (no copy)") });
        }
    }
    for candidate in candidates {
        if checks(engine, file, context, lowering, &candidate.edits) {
            for diagnostic in diagnostics.iter_mut().filter(|d| {
                engine.sources.resolve(d.primary).file == file
                    && matches!(d.rule.as_str(), "MODE-2" | "LANG-7" | "LANG-2" | "CONC-5")
            }) {
                diagnostic.secondary.push(hirsel_diag::Label {
                    span: candidate.site,
                    message: "checked declaration repair".into(),
                });
                diagnostic.notes.push(candidate.note.clone());
            }
            // Keep the smallest checked edit for this failure.
            break;
        }
    }
}

/// Root plus direct Result/Option/tuple payload rows, as expressible in a header.
fn result_edits(tree: &hirsel_syntax::SyntaxTree, result: TypeExprId) -> Vec<Vec<(Span, String)>> {
    let mut rows = vec![result];
    let base = match &tree.types[result.0 as usize].kind {
        TypeExprKind::Qualified { base, .. } => *base,
        _ => result,
    };
    match &tree.types[base.0 as usize].kind {
        TypeExprKind::Named { path, arguments }
            if path
                .segments
                .last()
                .is_some_and(|s| matches!(s.as_str(), "Result" | "Option")) =>
        {
            rows.extend(arguments.iter().filter_map(|a| match a {
                TypeArgument::Type(t) => Some(*t),
                _ => None,
            }));
        }
        TypeExprKind::Tuple(parts) => rows.extend(parts),
        _ => (),
    }
    rows.retain(|id| !matches!(&tree.types[id.0 as usize].kind, TypeExprKind::Qualified { modifiers, .. } if modifiers.contains(&TypeModifier::Unique)));
    // Result has at most three candidate rows; avoid exponential suggestions on
    // arbitrary tuple widths. A wide tuple gets the root-only recipe.
    if rows.len() > 3 {
        rows.truncate(1)
    }
    (1..(1usize << rows.len()))
        .map(|mask| {
            rows.iter()
                .enumerate()
                .filter_map(|(i, id)| {
                    (mask & (1 << i) != 0).then(|| {
                        let mut span = tree.types[id.0 as usize].origin.span;
                        span.end = span.start;
                        (span, "unique ".into())
                    })
                })
                .collect()
        })
        .collect()
}

fn strengthened_rows(engine: &QueryEngine, ty: TypeId) -> Vec<TypeId> {
    let Some(original) = engine.types.get(ty) else {
        return vec![];
    };
    let payloads = match &original.kind {
        TypeKind::Named {
            definition,
            arguments,
        } if matches!(definition.path().0.name.as_str(), "Result" | "Option") => arguments
            .iter()
            .filter_map(|a| match a {
                GenericArgument::Type(t) => Some(*t),
                _ => None,
            })
            .collect::<Vec<_>>(),
        _ => vec![],
    };
    (1..(1usize << (1 + payloads.len())))
        .map(|mask| {
            let mut row = original.clone();
            if mask & 1 != 0 {
                row.modes.uniqueness = Uniqueness::Unique
            }
            if let TypeKind::Named { arguments, .. } = &mut row.kind {
                for (i, argument) in arguments.iter_mut().take(payloads.len()).enumerate() {
                    if mask & (1 << (i + 1)) != 0
                        && let GenericArgument::Type(t) = argument
                        && let Some(payload) = engine.types.get(*t)
                    {
                        let mut payload = payload.clone();
                        payload.modes.uniqueness = Uniqueness::Unique;
                        *t = engine.types.intern(payload);
                    }
                }
            }
            engine.types.intern(row)
        })
        .collect()
}

fn edited_row(engine: &QueryEngine, span: Span, edits: &[(Span, String)]) -> String {
    let Some(text) = engine.sources.slice(span) else {
        return String::new();
    };
    let at = engine.sources.resolve(span);
    let mut text = text.to_owned();
    let mut edits = edits.to_vec();
    edits.sort_by_key(|(s, _)| std::cmp::Reverse(engine.sources.resolve(*s).start));
    for (site, replacement) in edits {
        let site = engine.sources.resolve(site);
        text.replace_range(
            (site.start - at.start) as usize..(site.end - at.start) as usize,
            &replacement,
        );
    }
    text
}

fn checks(
    engine: &QueryEngine,
    file: FileId,
    context: &FileContext,
    lowering: LoweringPolicy,
    edits: &[(Span, String)],
) -> bool {
    let Some(source) = engine.sources.file(file) else {
        return false;
    };
    let mut text = source.text().to_owned();
    let mut edits = edits.to_vec();
    edits.sort_by_key(|(s, _)| std::cmp::Reverse(engine.sources.resolve(*s).start));
    for (span, replacement) in edits {
        let span = engine.sources.resolve(span);
        if span.file != file {
            return false;
        }
        text.replace_range(span.start as usize..span.end as usize, &replacement);
    }
    let a = &engine.adapters;
    let mut trial = QueryEngine::new(
        engine.environment.clone(),
        PhaseAdapters {
            lexer: a.lexer.clone(),
            parser: a.parser.clone(),
            formatter: a.formatter.clone(),
            resolver: a.resolver.clone(),
            signature: a.signature.clone(),
            checker: a.checker.clone(),
            modes: a.modes.clone(),
            lowering: a.lowering.clone(),
            phases: a.phases.clone(),
            interpreter: None,
        },
    );
    trial.sources = engine.sources.clone();
    trial.types = engine.types.clone();
    trial.memo = engine.memo.clone();
    trial.items = engine.items.clone();
    trial.catalog = engine.catalog.clone();
    trial.declarations = engine.declarations.clone();
    trial.builtins = engine.builtins.clone();
    trial.package_manifests = engine.package_manifests.clone();
    trial.comptime_limits = engine.comptime_limits;
    trial.configured_jobs = engine.configured_jobs;
    let Some(edited) = trial.sources.add_snapshot(file, text) else {
        return false;
    };
    crate::tools::check_files_impl(&mut trial, &[(edited, context, lowering)], false).is_ok_and(
        |outputs| {
            outputs.iter().all(|(_, out)| {
                out.completion == CheckCompletion::Complete && !has_errors(&out.diagnostics)
            })
        },
    )
}
