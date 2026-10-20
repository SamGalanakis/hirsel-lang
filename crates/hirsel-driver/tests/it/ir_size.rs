//! R4 retained representation gate. Expanded debug paths/cleanup lists are not storage.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_tir::*;
use hirsel_types::{
    AuthoritySummary, DropAllocation, ErasedCaptures, GenericArgument, GenericParameter, Reach,
    Sendability, SharedPath, SharedSlice, Type, TypeKind, Uses,
};
use std::{collections::HashSet, mem::size_of};

#[derive(Debug, Default)]
struct Size {
    types: usize,
    edges: usize,
    rows: usize,
    actions: usize,
    paths: usize,
    source_paths: usize,
    arena_bytes: usize,
}
fn nodes<T>(mut path: &SharedPath<T>, seen: &mut HashSet<usize>) {
    while let Some(id) = path.node_identity() {
        if !seen.insert(id) {
            break;
        }
        path = path.prefix().unwrap();
    }
}
fn arena<T>(size: &mut Size, rows: &[T]) {
    size.rows += rows.len();
    size.arena_bytes += rows.len() * size_of::<T>();
}
// Each immutable allocation is charged once, including nested qualifier lists.
fn children<T>(size: &mut Size, slice: &SharedSlice<T>, seen: &mut HashSet<usize>) -> bool {
    if slice.is_empty() || !seen.insert(slice.storage_identity()) {
        return false;
    }
    size.arena_bytes += slice.len() * size_of::<T>();
    true
}
fn authority(size: &mut Size, a: &AuthoritySummary, seen: &mut HashSet<usize>) {
    if let AuthoritySummary::Known(terms) = a {
        children(size, terms, seen);
    }
}
fn type_children(
    size: &mut Size,
    ty: &Type,
    seen: &mut HashSet<usize>,
    origins: &mut HashSet<usize>,
) {
    if children(size, &ty.brand.alternatives, seen) {
        for origin in &ty.brand.alternatives {
            nodes(&origin.fields, origins);
        }
    }
    let reaches = ty.modes.relation.reaches();
    if !reaches.is_empty() && seen.insert(reaches.as_ptr() as usize) {
        size.arena_bytes += std::mem::size_of_val(reaches);
        for reach in reaches {
            if let Reach::Place(origin) = reach {
                nodes(&origin.fields, origins);
            }
        }
    }
    if let Sendability::DependsOn(ps) = &ty.modes.sendability {
        children(size, ps, seen);
    }
    if let DropAllocation::DependsOn(ps) = &ty.drop_allocation {
        children(size, ps, seen);
    }
    match &ty.erased_captures {
        Some(ErasedCaptures::FromArguments(ps)) => {
            children(size, ps, seen);
        }
        Some(ErasedCaptures::Known(a)) => authority(size, a, seen),
        None => (),
    }
    match &ty.kind {
        TypeKind::Tuple(ts) => {
            if children(size, ts, seen) {
                size.edges += ts.len();
            }
        }
        TypeKind::Record(fs) => {
            if children(size, fs, seen) {
                size.edges += fs.len();
            }
        }
        TypeKind::Variant(vs) => {
            if children(size, vs, seen) {
                for v in vs {
                    if children(size, &v.fields, seen) {
                        size.edges += v.fields.len();
                    }
                }
            }
        }
        TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => {
            if children(size, arguments, seen) {
                size.edges += arguments
                    .iter()
                    .filter(|a| matches!(a, GenericArgument::Type(_)))
                    .count();
            }
        }
        TypeKind::Function(f) => {
            size.edges += 1 + usize::from(f.error.is_some());
            if children(size, &f.parameters, seen) {
                size.edges += f.parameters.len();
            }
            if children(size, &f.generics, seen) {
                for g in &f.generics {
                    if let GenericParameter::Type { bounds, .. } = g {
                        children(size, bounds, seen);
                    }
                }
            }
            children(size, &f.contracts, seen);
            authority(size, &f.authority, seen);
            if let Uses::Only(a) = &f.uses {
                authority(size, a, seen);
            }
        }
        TypeKind::Box(_) => size.edges += 1,
        _ => (),
    }
}
fn measure(source: String) -> Size {
    let adapters = compiler_adapters();
    let lowerer = adapters.lowering.clone().unwrap();
    let mut engine = QueryEngine::new(default_environment(), adapters);
    let file = engine.sources.add("size.hir", source).unwrap();
    let parsed = engine.parse_file(file, &default_context()).unwrap();
    let definition = parsed
        .value
        .items
        .iter()
        .map(|id| &id.item)
        .find(|item| item.definition.0.name == "f")
        .unwrap()
        .definition
        .clone();
    let checked = engine.mode_definition(&definition).unwrap();
    let mut source_nodes = HashSet::new();
    for place in &checked.value.typed.checked.places {
        nodes(&place.projections, &mut source_nodes);
    }
    let signature = engine.item_signature(&definition).unwrap();
    let comptime = engine
        .prepare_comptime(&checked.value, &signature.value)
        .unwrap();
    let tir = lowerer
        .lower(
            engine.types(),
            engine.type_memo(),
            &checked.value,
            &comptime,
            &signature.value,
        )
        .unwrap();
    let d = &tir.value;
    let mut size = Size {
        types: d.types.len(),
        source_paths: source_nodes.len(),
        ..Size::default()
    };
    size.arena_bytes += d.types.len() * size_of::<Type>();
    let mut slices = HashSet::new();
    let mut origins = HashSet::new();
    for ty in d.types.iter().map(|&id| engine.types().row(id)) {
        type_children(&mut size, ty, &mut slices, &mut origins);
    }
    arena(&mut size, &d.bodies);
    let mut paths = HashSet::new();
    for body in &d.bodies {
        arena(&mut size, &body.locals);
        arena(&mut size, &body.places);
        arena(&mut size, &body.scopes);
        arena(&mut size, &body.blocks);
        arena(&mut size, &body.rvalues);
        arena(&mut size, &body.cleanups);
        arena(&mut size, &body.cleanup_nodes);
        arena(&mut size, &body.parameters);
        arena(&mut size, &body.matches);
        for table in &body.matches {
            arena(&mut size, &table.patterns);
            arena(&mut size, &table.arms);
            for pattern in &table.patterns {
                arena(&mut size, pattern.kind.fields());
            }
        }
        size.actions += body.cleanup_nodes.len();
        for place in &body.places {
            nodes(&place.projection, &mut paths);
            if let Some(binding) = &place.binding {
                nodes(&binding.fields, &mut origins);
            }
        }
        for block in &body.blocks {
            arena(&mut size, &block.instructions);
            if let Terminator::Call { arguments, .. } = &block.terminator {
                arena(&mut size, arguments);
            }
        }
        for value in &body.rvalues {
            match value {
                Rvalue::Closure { captures, .. } => arena(&mut size, captures),
                Rvalue::Construct { fields, .. } => arena(&mut size, fields),
                _ => {}
            }
        }
    }
    size.paths = paths.len() + origins.len();
    // Node payload is prefix handle, element and length; Arc allocator headers are excluded.
    size.arena_bytes += paths.len()
        * (size_of::<SharedPath<Projection>>() + size_of::<Projection>() + size_of::<usize>());
    size.arena_bytes += origins.len()
        * (size_of::<SharedPath<hirsel_types::FieldId>>()
            + size_of::<hirsel_types::FieldId>()
            + size_of::<usize>());
    size
}
fn source(shape: &str, n: usize) -> String {
    match shape {
        "wide_tuple" => format!(
            "fn f(a:({}))->() {{ {} () }}",
            vec!["u8"; n].join(","),
            (0..n).map(|i| format!("let x{i}=a\n")).collect::<String>()
        ),
        "lets" => format!(
            "fn f(a:u8)->u8 {{ {} a }}",
            (0..n)
                .map(|i| format!("let x{i}:u8=a\n"))
                .collect::<String>()
        ),
        "else_if" => format!(
            "fn f(a:u64)->u64 {{ {} {{ a }} }}",
            (0..n)
                .map(|i| format!("if a=={i}u64 {{ a }} else "))
                .collect::<String>()
        ),
        "match" => format!(
            "fn f(a:u64)->u64 {{ match a {{ {} _=>a }} }}",
            (0..n).map(|i| format!("{i}u64=>a,\n")).collect::<String>()
        ),
        "closures" => format!(
            "fn f(a:u8)->u8 {{ {} a }}",
            (0..n)
                .map(|i| format!("let x{i}=fn()->u8{{a}}\n"))
                .collect::<String>()
        ),
        "nested" => format!("fn f(a:u8)->u8 {{ {}a{} }}", "{ ".repeat(n), " }".repeat(n)),
        "fields" => {
            let mut text = "type R0 { x:u8 }\n".to_string();
            for i in 1..n {
                text += &format!("type R{i} {{ x:R{} }}\n", i - 1);
            }
            text + &format!("fn f(a:R{})->u8 {{ a{} }}", n - 1, ".x".repeat(n))
        }
        "generic_scopes" => format!(
            "fn f[T:plain](a:T)->() {{ {}(){} }}",
            (0..n)
                .map(|i| format!("let c{i}=fn()->(){{ "))
                .collect::<String>(),
            "}\n()".repeat(n)
        ),
        _ => unreachable!(),
    }
}
#[test]
fn retained_ir_is_linear_in_source_size() {
    with_compiler_stack(|| {
        for shape in [
            "lets",
            "else_if",
            "match",
            "closures",
            "nested",
            "fields",
            "generic_scopes",
            "wide_tuple",
        ] {
            let n = 16;
            let a = measure(source(shape, n));
            let b = measure(source(shape, 4 * n));
            eprintln!("{shape}: n={n} {a:?}; n={} {b:?}", 4 * n);
            for (metric, x, y) in [
                ("type rows", a.types, b.types),
                ("type edges", a.edges, b.edges),
                ("arena rows", a.rows, b.rows),
                ("cleanup actions", a.actions, b.actions),
                ("TIR path nodes", a.paths, b.paths),
                ("checked path nodes", a.source_paths, b.source_paths),
                ("arena payload bytes", a.arena_bytes, b.arena_bytes),
            ] {
                assert!(y <= 4 * x + 8, "{shape} {metric}: {x} -> {y}");
            }
        }
    });
}
