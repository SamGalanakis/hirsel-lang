//! Arena bump control: the fast block has only loads/stores/arithmetic;
//! native probes cover empty/full segments, exact fit, alignment, the extent-free
//! fixed-layout block and diagnostics.
use crate::support;
use hirsel_layout::layout::OwnerRepresentation;
use hirsel_lir::emission::{EmitInputs, OptLevel};
use hirsel_lir::{
    descriptor_registry::DescriptorRegistry,
    ir::*,
    layout_types::{ScalarKind, TargetSpec},
};
use hirsel_llvm::{emit_object, emit_optimized_ir};
use hirsel_syntax::PassingMode;
use hirsel_types::Primitive;
use support::{
    native::*,
    programs::{Program, payload},
    *,
};

fn program(target: &TargetSpec) -> Program {
    let mut schema = Schema::new();
    let mut arena_path = path("Arena");
    arena_path.0.package.name = "core".into();
    schema.owners.nominals.insert(
        arena_path.clone(),
        OwnerRepresentation::Handle {
            revision: hash("arena"),
            resource: false,
        },
    );
    let arena = schema.types.named(&arena_path, Vec::new());
    schema.name("arena", arena);
    for (name, primitive) in [("word", Primitive::U64), ("wide", Primitive::U128)] {
        let element = schema.types.p(primitive);
        let block = schema.types.boxed(element);
        schema.name(name, block);
        schema.signature(name, &[(PassingMode::Borrow, arena)], block);
    }
    let table = schema.build(target);
    let manifest = manifest();
    let mut units = Vec::new();
    for (name, kind) in [("word", ScalarKind::U64), ("wide", ScalarKind::U128)] {
        let arena = table.layout("arena");
        let mut u = Unit::new(&table, name, name);
        u.param(arena);
        let slot = u.slot(arena, SlotKind::Parameter(0));
        for ty in &mut u.builder.unit.types {
            if ty.layout == arena {
                ty.repr = ValueRepr::Arena;
            }
        }
        u.builder.unit.tables.arenas.push(ExternalInstance { slot });
        let block_layout = table.layout(name);
        let payload = payload(&table, name);
        let value = u.int(ENTRY, kind, 42);
        let block = u.construct_block(
            ENTRY,
            ConstructorKind::Box,
            payload,
            block_layout,
            &[(0, value)],
        );
        let site = u.builder.unit.tables.constructs.last_mut().unwrap();
        site.destination = DestinationClass::Arena(slot);
        site.placement = Some(Placement::Fixed(StorageOwner::Arena(ArenaInstanceId(0))));
        u.builder.ret(ENTRY, Some(block));
        units.push(seal(&table, &manifest, u.finish()));
    }
    Program {
        table,
        manifest,
        units,
        statics: Vec::new(),
        finalizers: Default::default(),
    }
}

fn inputs<'a>(
    p: &'a Program,
    input: &'a hirsel_lir::stage::InputIdentity,
    failures: &'a hirsel_lir::emission::FailureCodes,
    opt: OptLevel,
) -> EmitInputs<'a> {
    static INLINE: std::collections::BTreeSet<hirsel_syntax::ContentHash> =
        std::collections::BTreeSet::new();
    EmitInputs {
        input,
        target: &p.table.target,
        layouts: &p.table.table,
        manifest: &p.manifest,
        failures,
        statics: &p.statics,
        finalizers: &p.finalizers,
        inline_instances: &INLINE,
        available_externally: &INLINE,
        callbacks: &hirsel_lir::emission::NO_CALLBACKS,
        registry: DescriptorRegistry::new(&p.table.table, &p.finalizers),
        opt,
        measurement: false,
    }
}

#[test]
fn arena_fast_path_has_no_call_on_either_target() {
    for target in TargetSpec::executed() {
        let p = program(&target);
        let (input, failures) = (input(), failure_codes());
        let inputs = inputs(&p, &input, &failures, OptLevel::None);
        let ir = emit_optimized_ir(&p.units(), &inputs).unwrap();
        // Each constructor's block which installs the immortal count must also
        // advance the cursor, with no call on that path. A fixed layout's block has
        // no extent: the cursor, the two header words and the result are its stores.
        let fast: Vec<_> = ir
            .split("\n\n")
            .filter(|b| b.contains("store i32 -1,"))
            .collect();
        assert_eq!(fast.len(), 2, "{ir}");
        for block in fast {
            assert!(!block.contains("call "), "{block}");
            assert!(block.contains("store ptr "), "{block}");
            assert!(!block.contains("store i64 "), "{block}");
            assert_eq!(block.matches("store ").count(), 4, "{block}");
        }
        assert!(ir.contains("call ptr @hirsel_rt_v2_arena_alloc"), "{ir}");
        emit_object(&p.units(), &inputs).unwrap();
    }
}

#[test]
#[ignore = "native: focused arena bump controls on both targets"]
fn bumps_preserve_metadata_alignment_and_diagnostic_owners() {
    for target in TargetSpec::executed() {
        let p = program(&target);
        let registry = DescriptorRegistry::new(&p.table.table, &p.finalizers);
        let calls = p.c_call("word", "call_word") + &p.c_call("wide", "call_wide");
        let iw = registry.index(payload(&p.table, "word")).unwrap();
        let ix = registry.index(payload(&p.table, "wide")).unwrap();
        let c = format!(
            r#"
#include <stdint.h>
#include <stdio.h>
{calls}
int main(void) {{
  for (unsigned diagnostic = 0; diagnostic < 2; diagnostic++) {{
    hirsel_rt_v2_config config = {{.abi_version = HIRSEL_RT_V2_ABI_VERSION,
      .flags = diagnostic ? HIRSEL_RT_V2_CONFIG_DIAGNOSTIC : 0}};
    hirsel_rt_v2_ctx *ctx = NULL;
    assert(hirsel_rt_v2_ctx_create(&config, &ctx) == 0);
    uint8_t (*entries[])(hirsel_rt_v2_ctx *, void *, void *, void *) = {{call_word, call_wide}};
    uint32_t indices[] = {{{iw}, {ix}}};
    /* Header and payload only: a 16-byte payload aligned to 16 pads every other block. */
    uint64_t sizes[] = {{8, 16}}, aligns[] = {{8, 16}}, strides[] = {{16, 32}};
    for (unsigned q = 0; q < 2; q++) {{
      /* The token is passed by value; an Arena slot is never the heap. */
      hirsel_rt_v2_region arena;
      hirsel_rt_v2_region_enter(ctx, &arena);
      assert(arena.cursor == NULL && arena.end == NULL);
      char *previous = NULL;
      void *segment = NULL;
      for (unsigned i = 0; i < 6000; i++) {{
        hirsel_rt_v2_header *r = NULL;
        assert(entries[q](ctx, NULL, &r, &arena) == 0 && r->count == UINT32_MAX);
        assert(((uintptr_t)hirsel_rt_v2_payload(r) % aligns[q]) == 0);
        assert(((uintptr_t)arena.cursor % 8) == 0);
        if (previous && arena.segment == segment)
          assert((char *)r == previous + strides[q]);
        previous = (char *)r;
        segment = arena.segment;
        assert(*(uint64_t *)hirsel_rt_v2_payload(r) == 42);
        assert((r->descriptor >> 29) == 3);
        assert((r->descriptor & ((1u << 29) - 1)) == indices[q]);
        assert(!diagnostic || arena.end == NULL);
      }}
      if (!diagnostic) {{
        /* Leave exactly one header and payload: equality takes the inline path. */
        uint64_t request = 8 + sizes[q];
        arena.cursor = (char *)arena.end - request;
        void *end = arena.end;
        hirsel_rt_v2_header *r = NULL;
        assert(entries[q](ctx, NULL, &r, &arena) == 0 && arena.cursor == end && arena.end == end);
        assert(entries[q](ctx, NULL, &r, &arena) == 0 && arena.end != end);
      }}
      assert(hirsel_rt_v2_region_exit(ctx, &arena, 0) == 0);
    }}
    hirsel_rt_v2_stats stats;
    hirsel_rt_v2_ctx_stats(ctx, &stats);
    assert(stats.live_blocks == 0 && stats.active_regions == 0);
    hirsel_rt_v2_ctx_destroy(ctx);
  }}
  puts("arena bump ok");
}}
"#
        );
        for opt in [OptLevel::None, OptLevel::Speed] {
            let dir = scratch(&format!("arena-bump-{}-{opt:?}", target.triple));
            let source = dir.join("arena.c");
            std::fs::write(&source, &c).unwrap();
            let (input, failures) = (input(), failure_codes());
            let object = emit_object(&p.units(), &inputs(&p, &input, &failures, opt)).unwrap();
            let executable = link(&target, &dir, &[("arena.o", &object.bytes)], &[source]);
            assert_eq!(run(&target, &executable), "arena bump ok\n");
        }
    }
}

fn credit_program(target: &TargetSpec, elided: bool, heap_child: bool) -> Program {
    let mut schema = Schema::new();
    let mut arena_path = path("Arena");
    arena_path.0.package.name = "core".into();
    schema.owners.nominals.insert(
        arena_path.clone(),
        OwnerRepresentation::Handle {
            revision: hash("arena"),
            resource: false,
        },
    );
    let arena = schema.types.named(&arena_path, Vec::new());
    schema.name("arena", arena);
    let scalar = schema.types.p(Primitive::U64);
    let leaf = schema.types.boxed(scalar);
    schema.name("leaf", leaf);
    let parent = schema.types.boxed(leaf);
    schema.name("parent", parent);
    schema.signature("parent", &[(PassingMode::Borrow, arena)], parent);
    let table = schema.build(target);
    let manifest = manifest();
    let mut u = Unit::new(&table, "parent", "parent");
    let arena = table.layout("arena");
    u.param(arena);
    let slot = u.slot(arena, SlotKind::Parameter(0));
    for ty in &mut u.builder.unit.types {
        if ty.layout == arena {
            ty.repr = ValueRepr::Arena;
        }
    }
    u.builder.unit.tables.arenas.push(ExternalInstance { slot });
    let six = u.int(ENTRY, ScalarKind::U64, 6);
    let child = u.construct_block(
        ENTRY,
        ConstructorKind::Box,
        payload(&table, "leaf"),
        table.layout("leaf"),
        &[(0, six)],
    );
    if !heap_child {
        let site = u.builder.unit.tables.constructs.last_mut().unwrap();
        site.destination = DestinationClass::Arena(slot);
        site.placement = Some(Placement::Fixed(StorageOwner::Arena(ArenaInstanceId(0))));
    }
    let parent = u.construct_block(
        ENTRY,
        ConstructorKind::Box,
        payload(&table, "parent"),
        table.layout("parent"),
        &[(0, child)],
    );
    let site = u.builder.unit.tables.constructs.last_mut().unwrap();
    site.destination = DestinationClass::Arena(slot);
    site.placement = Some(Placement::Fixed(StorageOwner::Arena(ArenaInstanceId(0))));
    site.arena_no_drop = elided;
    u.builder.ret(ENTRY, Some(parent));
    Program {
        units: vec![seal(&table, &manifest, u.finish())],
        table,
        manifest,
        statics: vec![],
        finalizers: Default::default(),
    }
}

#[test]
fn proved_arena_children_emit_neither_count_loads_nor_scheduling() {
    for target in TargetSpec::executed() {
        for (elided, heap_child) in [(false, false), (false, true), (true, false)] {
            let p = credit_program(&target, elided, heap_child);
            let (input, failures) = (input(), failure_codes());
            let inputs = inputs(&p, &input, &failures, OptLevel::None);
            let ir = emit_optimized_ir(&p.units(), &inputs).unwrap();
            assert_eq!(
                ir.contains("call i8 @hirsel_rt_v2_region_schedule"),
                !elided,
                "{ir}"
            );
            // Heap-token branches also load the allocator's slab live count.
            // A holder read is identified by its immortal-count comparison.
            let holder_load = ir
                .lines()
                .filter_map(|line| {
                    line.trim()
                        .split_once(" = load i32, ptr")
                        .map(|(value, _)| value)
                })
                .any(|value| ir.contains(&format!("icmp ne i32 {value}, -1")));
            assert_eq!(holder_load, !elided, "{ir}");
            emit_object(&p.units(), &inputs).unwrap();
        }
    }
}
