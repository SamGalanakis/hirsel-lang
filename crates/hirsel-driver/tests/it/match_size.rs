//! Match-specific R4 gate, including arena payloads and shared path retention.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_tir::*;
use hirsel_types::{SharedPath, Type, TypeId};
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
    for ty in d.types.iter().map(|&id| engine.types().row(id)) {
        size.edges += ty.kind.graph_edges().len();
    }
    size.arena_bytes += size.edges * size_of::<TypeId>();
    arena(&mut size, &d.bodies);
    let mut paths = HashSet::new();
    let mut origins = HashSet::new();
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
fn product(n: usize) -> (String, usize, usize) {
    let width = n * (n - 1);
    let mut rows = vec![];
    for pigeon in 0..n {
        rows.push(
            (0..width)
                .map(|i| if i / (n - 1) == pigeon { "false" } else { "_" })
                .collect::<Vec<_>>(),
        );
    }
    for a in 0..n {
        for b in a + 1..n {
            for hole in 0..n - 1 {
                rows.push(
                    (0..width)
                        .map(|i| {
                            if i == a * (n - 1) + hole || i == b * (n - 1) + hole {
                                "true"
                            } else {
                                "_"
                            }
                        })
                        .collect(),
                );
            }
        }
    }
    let arms = rows
        .iter()
        .map(|r| format!("({})=>()", r.join(",")))
        .collect::<Vec<_>>()
        .join(",");
    (
        format!(
            "fn f(p:({})) {{ match p {{ {arms} }} }}",
            vec!["bool"; width].join(",")
        ),
        width * rows.len(),
        rows.len(),
    )
}
#[test]
fn product_match_retention_is_linear_in_pattern_nodes() {
    with_compiler_stack(|| {
        let mut previous: Option<(usize, Size)> = None;
        for n in 2..=5 {
            let (source, cells, arms) = product(n);
            let size = measure(source);
            eprintln!("match product n={n}, cells={cells}, arms={arms}: {size:?}");
            assert!(size.rows <= 12 * (cells + arms) + 128, "{size:?}");
            assert!(size.paths <= 4 * cells + 64, "{size:?}");
            if let Some((units, smaller)) = &previous {
                let larger_units = cells + arms;
                for (name, x, y) in [
                    ("rows", smaller.rows, size.rows),
                    ("payload bytes", smaller.arena_bytes, size.arena_bytes),
                    ("checked paths", smaller.source_paths, size.source_paths),
                ] {
                    assert!(
                        y * units <= (x + 8) * larger_units,
                        "product {name}: {smaller:?} -> {size:?}"
                    );
                }
            }
            previous = Some((cells + arms, size));
        }
    });
}
#[test]
fn guarded_arm_retention_scales_with_source_arms() {
    with_compiler_stack(|| {
        let mut counts = vec![];
        for n in [16, 64] {
            let arms = (0..n)
                .map(|i| format!("{i}u64 if a>{i}u64=>a,"))
                .collect::<String>();
            let size = measure(format!("fn f(a:u64)->u64 {{match a{{{arms}_=>a}}}}"));
            eprintln!("guarded match n={n}: {size:?}");
            counts.push(size);
        }
        let a = &counts[0];
        let b = &counts[1];
        for (name, x, y) in [
            ("rows", a.rows, b.rows),
            ("type rows", a.types, b.types),
            ("paths", a.paths, b.paths),
            ("cleanup nodes", a.actions, b.actions),
            ("bytes", a.arena_bytes, b.arena_bytes),
        ] {
            assert!(y <= 4 * x + 8, "guarded match {name}: {x} -> {y}");
        }
    });
}
