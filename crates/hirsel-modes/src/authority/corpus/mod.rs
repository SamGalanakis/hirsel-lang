//! Real-phase corpus ratchet. Pending outcomes remain visible and named.
mod support;
use super::*;
use hirsel_check::body;
use hirsel_resolve::{ManifestFixture, ManifestIndex};
use hirsel_syntax as ast;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

fn owned(rule: &str) -> bool {
    matches!(
        rule,
        "PROV-1"
            | "PROV-2"
            | "PROV-3"
            | "PROV-4"
            | "PROV-5"
            | "CAP-1"
            | "CAP-2"
            | "CAP-3"
            | "CAP-4"
            | "CLO-2"
            | "CLO-3"
            | "CLO-4"
            | "MODE-4"
            | "MODE-5"
            | "MODE-6"
            | "OWN-6"
            | "CONC-2"
            | "CONC-5"
            | "CONC-6"
            | "CONC-7"
    )
}
fn selected(prefix: &str, name: &str) -> bool {
    match prefix {
        "MODE" => ["MODE-4", "MODE-5", "MODE-6"]
            .iter()
            .any(|r| name.starts_with(r)),
        "OWN" => name.starts_with("OWN-6"),
        _ => true,
    }
}
fn roles() -> Intrinsics {
    let mut roles: Intrinsics = [
        ("shared_freeze", Intrinsic::SharedFreeze),
        ("arena_seal", Intrinsic::ArenaSeal),
        ("task.async", Intrinsic::TaskAsync),
        ("task.isolate", Intrinsic::TaskIsolate),
        ("task.await", Intrinsic::TaskAwait),
        ("task.detach", Intrinsic::TaskDetach),
        ("mutex.create", Intrinsic::MutexCreate),
        ("mutex.with", Intrinsic::MutexWith),
        ("channel.open", Intrinsic::ChannelOpen),
        ("channel.send", Intrinsic::ChannelSend),
        ("atomic.create", Intrinsic::AtomicCreate),
    ]
    .into_iter()
    .map(|(name, role)| (support::path(name), role))
    .collect();
    roles.insert(support::path("shared.freeze"), Intrinsic::SharedFreeze);
    roles.insert(support::path("arena.seal"), Intrinsic::ArenaSeal);
    for name in ["atomic.load", "atomic.store"] {
        roles.insert(support::path(name), Intrinsic::Synchronization);
    }
    for name in [
        "file.write_all",
        "file.commit",
        "slots.get",
        "slots.remove",
        "channel.receive",
        "sender.dup",
    ] {
        roles.insert(support::path(name), Intrinsic::HandleOperation);
    }
    roles.insert(
        support::path("int.unchecked_add"),
        Intrinsic::UnsafeOperation,
    );
    roles
}
#[test]
fn real_phase_corpus_ratchet() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../conformance");
    let (empty, _, _) = support::parse("", &[]);
    let base = support::registry(&empty);
    let roles = roles();
    let mut passing = BTreeSet::new();
    let mut pending = BTreeMap::new();
    let mut counts: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    let mut annotations = BTreeMap::new();
    for prefix in ["PROV", "CAP", "CLO", "MODE", "OWN", "CONC"] {
        let mut paths: Vec<_> = std::fs::read_dir(root.join(prefix))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "hir"))
            .collect();
        paths.sort();
        for path in paths {
            let filename = path.file_name().unwrap().to_str().unwrap();
            if !selected(prefix, filename) {
                continue;
            }
            let name = format!("{prefix}/{filename}");
            let source = std::fs::read_to_string(&path).unwrap();
            let expect = source
                .lines()
                .find_map(|l| l.strip_prefix("// expect: "))
                .unwrap();
            let rejection = expect
                .strip_prefix("reject ")
                .and_then(|r| r.split_once(" at line "));
            if rejection.is_some_and(|(r, _)| !owned(r)) {
                counts.entry(prefix.into()).or_default().2 += 1;
                pending.insert(name, "dependency-owned I/P: operative rule is ownership/locality/allocation rather than J".to_owned());
                continue;
            }
            if let Some(issue) = source.lines().find_map(|l| l.strip_prefix("// spec: ")) {
                annotations.insert(name.clone(), issue.to_owned());
            }
            counts.entry(prefix.into()).or_default().1 += 1;
            let outcome = (|| -> Result<(), String> {
                let (file, map, ds) = support::parse(&source, &[]);
                if let Some(d) = ds.first() {
                    return Err(format!(
                        "dependency-owned A/B: {} line {}",
                        d.rule,
                        map.location(d.primary).unwrap().line
                    ));
                }
                let (index, ds) = ManifestIndex::new(
                    ManifestFixture::from_files(
                        support::context().package,
                        [&file],
                        ast::ContentHash::default(),
                    )
                    .with_dependency_signatures(base.signatures.values()),
                );
                if let Some(d) = ds.first() {
                    return Err(format!("dependency-owned D: index {}", d.rule));
                }
                let mut registry = support::Registry {
                    signatures: base.signatures.clone(),
                    index,
                    reads: vec![],
                };
                if let Err(e) = support::normalize_file(&file, &mut registry) {
                    return Err(format!("dependency-owned E/v20 follow-up: {e}"));
                }
                let mut diagnostics = Vec::new();
                for id in &file.roots {
                    let resolution = hirsel_resolve::resolve_item(
                        &file,
                        *id,
                        &file.symbols,
                        &registry.index.clone(),
                        &mut registry,
                    );
                    if let Some(d) = resolution.query.diagnostics.first() {
                        return Err(format!(
                            "dependency-owned D/fixture M: {} line {}",
                            d.rule,
                            map.location(d.primary).unwrap().line
                        ));
                    }
                    let sig =
                        registry.signatures[&file.tree.items[id.0 as usize].definition].clone();
                    let output = body::check_with_loop_protocols(
                        body::LoopInput {
                            body: body::Input {
                                tree: &file.tree,
                                item: *id,
                                symbols: &file.symbols,
                                context: &support::context(),
                                resolution: &resolution,
                                signature: &sig,
                            },
                            protocols: &hirsel_types::core_roles::loop_protocols(),
                        },
                        &mut registry,
                    )
                    .map_err(|e| format!("dependency-owned H/provider: {e:?}"))?;
                    let checked = output.checked;
                    if let Some(d) = checked.diagnostics.first() {
                        return Err(format!(
                            "dependency-owned H: {} line {}: {}",
                            d.rule,
                            map.location(d.primary).unwrap().line,
                            d.message
                        ));
                    }
                    if std::env::var("HIRSEL_AUTHORITY_DEBUG").ok().as_deref() == Some(&name) {
                        eprintln!("DEBUG {name} item {id:?} signature {:?}", sig.kind);
                        eprintln!("RES {:?}", checked.value.resolution);
                        for b in &checked.value.bindings {
                            eprintln!("BIND {b:?}");
                        }
                        for row in &checked.value.expressions {
                            eprintln!(
                                "EXPR {:?} line {:?} {:?} TYPE {:?}",
                                row.expression,
                                map.location(
                                    file.tree.expressions[row.expression.0 as usize].origin.span
                                ),
                                file.tree.expressions[row.expression.0 as usize].kind,
                                checked.value.types.get(row.ty)
                            );
                        }
                    }
                    let out = check_file_with_clauses(
                        &file,
                        *id,
                        &sig,
                        checked.value,
                        &mut registry,
                        &roles,
                        &output
                            .plans
                            .loops
                            .iter()
                            .filter_map(|p| p.cursor().cloned())
                            .collect::<Vec<_>>(),
                        &output
                            .plans
                            .contracts
                            .iter()
                            .map(|p| (p.expression, p.function))
                            .collect::<Vec<_>>(),
                    );
                    if let Some(e) = out.failure {
                        return Err(format!("dependency-owned provider: {e:?}"));
                    }
                    diagnostics.extend(out.diagnostics);
                }
                diagnostics.sort_by_key(|d| {
                    let (p, n) = d.rule.as_str().split_once('-').unwrap();
                    (
                        d.primary.start,
                        d.primary.end,
                        p.to_owned(),
                        n.parse::<u32>().unwrap(),
                    )
                });
                if let Some((rule, line)) = rejection {
                    match diagnostics.first() {
                        Some(d)
                            if d.rule.as_str() == rule
                                && map.location(d.primary).unwrap().line
                                    == line.parse::<u32>().unwrap() =>
                        {
                            Ok(())
                        }
                        Some(d) => Err(format!(
                            "not done J: expected {rule} line {line}; got {} line {}: {}",
                            d.rule,
                            map.location(d.primary).unwrap().line,
                            d.message
                        )),
                        None => Err(format!("not done J: missing {rule} line {line}")),
                    }
                } else if let Some(d) = diagnostics.first() {
                    Err(format!(
                        "not done J: unexpected {} line {}: {}",
                        d.rule,
                        map.location(d.primary).unwrap().line,
                        d.message
                    ))
                } else {
                    Ok(())
                }
            })();
            let outcome = outcome.map_err(|reason| if name == "MODE/MODE-4-iterator-state-ok.hir" && reason.starts_with("not done J:") {
                format!("dependency-owned H/contract revision 1: factory state sendability is absent from erased Iter; conservative rejection: {reason}")
            } else { reason });
            match outcome {
                Ok(()) => {
                    counts.get_mut(prefix).unwrap().0 += 1;
                    passing.insert(name);
                }
                Err(reason) => {
                    pending.insert(name, reason);
                }
            }
        }
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/authority/corpus");
    let baseline = std::fs::read_to_string(dir.join("passing.txt")).unwrap();
    for path in baseline
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        assert!(
            passing.contains(path),
            "authority corpus regression {path}: {:?}",
            pending.get(path)
        );
    }
    if std::env::var_os("HIRSEL_BLESS").is_some() {
        std::fs::write(
            dir.join("passing.txt"),
            passing.iter().cloned().collect::<Vec<_>>().join("\n") + "\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("pending.txt"),
            pending
                .iter()
                .map(|(p, why)| format!("{p} # {why}"))
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("annotations.txt"),
            annotations
                .iter()
                .map(|(p, issue)| format!("{p} # spec: {issue}"))
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .unwrap();
    }
    eprintln!(
        "authority corpus (passing, considered, other operative rule): {counts:?}; {} passing, {} pending; {} spec annotations",
        passing.len(),
        pending.len(),
        annotations.len()
    );
    assert!(
        !passing.is_empty(),
        "real corpus produced no passing program"
    );
}

fn last_function(file: &ast::SyntaxFile) -> ast::ItemId {
    *file
        .roots
        .iter()
        .rev()
        .find(|item| {
            matches!(
                file.tree.items[item.0 as usize].kind,
                ast::ItemKind::Function { .. }
            )
        })
        .expect("fixture has a function")
}

fn check_source(source: &str) -> (ast::SyntaxFile, AuthorityOutput) {
    let (file, _, _, _, out) = check_source_phases(source);
    (file, out)
}

fn check_source_phases(
    source: &str,
) -> (
    ast::SyntaxFile,
    SignatureRef,
    support::Registry,
    CheckedBody,
    AuthorityOutput,
) {
    let (file, _, diagnostics) = support::parse(source, &[]);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let mut registry = support::registry(&file);
    support::normalize_file(&file, &mut registry).unwrap();
    let item = last_function(&file);
    let resolution = hirsel_resolve::resolve_item(
        &file,
        item,
        &file.symbols,
        &registry.index.clone(),
        &mut registry,
    );
    assert!(
        resolution.query.diagnostics.is_empty(),
        "{:?}",
        resolution.query.diagnostics
    );
    let signature = registry.signatures[&file.tree.items[item.0 as usize].definition].clone();
    let output = body::check_with_loop_protocols(
        body::LoopInput {
            body: body::Input {
                tree: &file.tree,
                item,
                symbols: &file.symbols,
                context: &support::context(),
                resolution: &resolution,
                signature: &signature,
            },
            protocols: &hirsel_types::core_roles::loop_protocols(),
        },
        &mut registry,
    )
    .unwrap();
    let checked = output.checked;
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let out = check_file_with_clauses(
        &file,
        item,
        &signature,
        checked.value.clone(),
        &mut registry,
        &roles(),
        &output
            .plans
            .loops
            .iter()
            .filter_map(|p| p.cursor().cloned())
            .collect::<Vec<_>>(),
        &output
            .plans
            .contracts
            .iter()
            .map(|p| (p.expression, p.function))
            .collect::<Vec<_>>(),
    );
    (file, signature, registry, checked.value, out)
}

#[test]
fn result_types_cover_formal_named_frames_and_inferred_closures() {
    let source = "fn outer(p: u64) -> u64 {\n fn inner(q: u64) -> u64 { q }\n let callback = fn() { p }\n inner(p)\n}\n";
    let (file, out) = check_source(source);
    super::test_support::clean(&out);
    let item = file.roots[0];
    let statement = out.body.resolution.local_functions[0].0;
    let closure = out
        .captures
        .iter()
        .find(|c| {
            matches!(
                file.tree.expressions[c.expression.0 as usize].kind,
                ast::ExprKind::Closure { .. }
            )
        })
        .unwrap()
        .expression;
    assert_eq!(out.result_types.len(), 3);
    for owner in [
        NameSite::Item(item),
        NameSite::Statement(statement),
        NameSite::Expression(closure),
    ] {
        let result = out
            .result_types
            .iter()
            .find(|(site, _)| *site == owner)
            .unwrap()
            .1;
        assert!(matches!(
            out.body.types.get(result).unwrap().kind,
            TypeKind::Primitive(Primitive::U64)
        ));
        if let NameSite::Expression(e) = owner {
            let info = out
                .body
                .closures
                .iter()
                .find(|c| c.expression == e)
                .unwrap();
            let TypeKind::Function(f) = &out.body.types.get(info.ty).unwrap().kind else {
                panic!()
            };
            assert_eq!(result, f.result);
        }
    }
}

#[test]
fn named_frame_result_origins_resolve_the_outer_declaring_scope() {
    for same in [false, true] {
        let source = format!(
            "fn outer(unused: Dir, p: Dir, take value: File of p) -> File of p {{\n fn inner(q: Dir, take inner_value: File of {brand}) -> File of {brand} {{ take inner_value }}\n inner(p, take value)\n}}\n",
            brand = if same { "p" } else { "unused" }
        );
        let (file, out) = check_source(&source);
        if same {
            super::test_support::clean(&out);
        } else {
            super::test_support::rejected(&out, "PROV-1");
        }
        let outer = NameSite::Item(file.roots[0]);
        let inner = NameSite::Statement(out.body.resolution.local_functions[0].0);
        for (owner, ordinal) in [(outer.clone(), 1), (inner, if same { 1 } else { 0 })] {
            let result = out
                .result_types
                .iter()
                .find(|(site, _)| *site == owner)
                .unwrap()
                .1;
            let root = out.body.types.get(result).unwrap().brand.alternatives[0]
                .root
                .clone();
            let OriginRoot::Parameter(parameter) = root else {
                panic!("{root:?}")
            };
            assert_eq!(parameter, ParameterRef::body(outer, ParamId(ordinal)));
        }
    }
}

#[test]
fn ownership_retains_named_function_type_separately_from_its_body_result() {
    let (file, signature, mut registry, _, mut out) = check_source_phases(
        "fn example(n: u32) -> u32 { fn loop(x: u32) -> u32 { if x == 0 { 0 } else { x - 1 } } loop(n) }",
    );
    super::test_support::clean(&out);
    let item = last_function(&file);
    let ast::StmtKind::Function {
        body: body_expression,
        ..
    } = file.tree.statements[out.body.resolution.local_functions[0].0.0 as usize].kind
    else {
        panic!()
    };
    let info = out
        .body
        .closures
        .iter()
        .find(|c| c.expression == body_expression)
        .unwrap();
    assert!(matches!(
        out.body.types.get(info.ty).unwrap().kind,
        TypeKind::Function(_)
    ));
    let parameters: Vec<_> = out
        .body
        .bindings
        .iter()
        .filter(|binding| {
            matches!(binding.owner, NameSite::Statement(_))
                && matches!(binding.binding, ResolvedName::Parameter(_))
        })
        .map(|binding| (binding.owner.clone(), binding.binding.clone(), binding.ty))
        .collect();
    assert!(!parameters.is_empty());
    let owned = crate::ownership::check_ownership_with_clauses(
        &file.tree,
        &file.symbols,
        item,
        &signature,
        &mut out.body,
        &[],
        &[],
        &mut out.result_types,
        &[],
        &[],
        &mut registry,
    )
    .unwrap();
    assert!(owned.diagnostics.is_empty(), "{:?}", owned.diagnostics);
    for (owner, name, declared) in parameters {
        let binding = out
            .body
            .bindings
            .iter()
            .find(|binding| binding.owner == owner && binding.binding == name)
            .unwrap();
        assert!(
            semantic_equal(&out.body.types, declared, &out.body.types, binding.ty).unwrap(),
            "local function initialization changed its parameter modes: {:?}",
            out.body.types.get(binding.ty)
        );
    }
    let function = out
        .body
        .bindings
        .iter()
        .find(|binding| {
            matches!(binding.site, NameSite::Statement(_))
                && matches!(binding.binding, ResolvedName::Local(_))
        })
        .unwrap();
    assert_eq!(
        out.body.types.get(function.ty).unwrap().modes.uniqueness,
        Uniqueness::Unique
    );

    let info = out
        .body
        .closures
        .iter()
        .find(|c| c.expression == body_expression)
        .unwrap();
    assert!(matches!(
        out.body.types.get(info.ty).unwrap().kind,
        TypeKind::Function(_)
    ));
    let result = out
        .body
        .expressions
        .iter()
        .find(|e| e.expression == body_expression)
        .unwrap();
    assert!(matches!(
        out.body.types.get(result.ty).unwrap().kind,
        TypeKind::Primitive(Primitive::U32)
    ));
}
