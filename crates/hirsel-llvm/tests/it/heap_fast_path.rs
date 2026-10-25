//! Heap fast path control (runtime ABI 4): a constant small allocation, a dup and the drop
//! glue's free run inline after optimization, with no call on their fast blocks, and a block
//! that never escapes is still deleted; native probes interleave them with the runtime's own
//! entries in production and diagnostic mode.
use crate::support;
use hirsel_lir::emission::{EmitInputs, OptLevel};
use hirsel_lir::{descriptor_registry::DescriptorRegistry, ir::*, layout_types::TargetSpec};
use hirsel_llvm::{emit_object, emit_optimized_ir};
use hirsel_syntax::PassingMode;
use hirsel_types::Primitive;
use support::{
    native::*,
    programs::{Program, payload},
    *,
};

/// `churn(x) = { b = box x; dup b; release b; release b; x }`.
fn program(target: &TargetSpec) -> Program {
    let mut schema = Schema::new();
    let word = schema.types.p(Primitive::U64);
    let block = schema.types.boxed(word);
    schema.name("cell", block);
    schema.name("churn", word);
    schema.signature("churn", &[(PassingMode::Borrow, word)], word);
    let table = schema.build(target);
    let manifest = manifest();
    let mut u = Unit::new(&table, "churn", "churn");
    let x = u.param(table.layout("churn"));
    let payload = payload(&table, "cell");
    let b = u.construct_block(
        ENTRY,
        ConstructorKind::Box,
        payload,
        table.layout("cell"),
        &[(0, x)],
    );
    u.builder.push(ENTRY, Inst::Dup { value: b });
    let failure = u.releasing(&[(b, payload)], None, SourceAnchorId(0));
    u.release_to(ENTRY, b, payload, failure);
    u.release(ENTRY, b, payload);
    u.builder.ret(ENTRY, Some(x));
    let units = vec![seal(&table, &manifest, u.finish())];
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
    opt: OptLevel,
) -> EmitInputs<'a> {
    static INLINE: std::collections::BTreeSet<hirsel_syntax::ContentHash> =
        std::collections::BTreeSet::new();
    EmitInputs {
        ahead: None,
        alternate: false,
        index: None,
        verify_backend: true,
        input,
        target: &p.table.target,
        layouts: &p.table.table,
        keys: &p.table.keys,
        manifest: &p.manifest,
        statics: &p.statics,
        inline_instances: &INLINE,
        available_externally: &INLINE,
        callbacks: false,
        foreign: &hirsel_lir::emission::NO_FOREIGN,
        tracking: false,
        opt,
        measurement: false,
        fuel: false,
    }
}

#[test]
#[ignore = "native: focused heap fast path controls on both targets"]
fn inline_allocation_dup_and_free_agree_with_the_runtime() {
    for target in TargetSpec::executed() {
        let p = program(&target);
        let registry = DescriptorRegistry::new(&p.table.table, &p.finalizers);
        let churn = p.c_call("churn", "call_churn");
        let index = registry.index(payload(&p.table, "cell")).unwrap();
        let c = format!(
            r#"
#include <stdint.h>
#include <stdio.h>
{churn}
static uint32_t word32(hirsel_rt_v2_ctx *ctx, uint64_t offset) {{
  return *(uint32_t *)((char *)ctx + offset);
}}
int main(void) {{
  for (unsigned diagnostic = 0; diagnostic < 2; diagnostic++) {{
    hirsel_rt_v2_config config = {{.abi_version = HIRSEL_RT_V2_ABI_VERSION,
      .flags = diagnostic ? HIRSEL_RT_V2_CONFIG_DIAGNOSTIC : 0}};
    hirsel_rt_v2_ctx *ctx = NULL;
    assert(hirsel_rt_v2_ctx_create(&config, &ctx) == 0);
    assert((word32(ctx, HIRSEL_RT_V2_CTX_DUP_MAX_OFFSET) != 0) == !diagnostic);
    assert(word32(ctx, HIRSEL_RT_V2_CTX_FREE_INLINE_OFFSET) == !diagnostic);
    static hirsel_rt_v2_header *held[5000];
    for (uint64_t round = 0; round < 8; round++) {{
      /* Runtime blocks of the same pool fill and drain the pages between inline churn. */
      for (unsigned i = 0; i < 5000; i++) {{
        held[i] = hirsel_rt_v2_alloc(ctx, {index}, 8, 8, HIRSEL_RT_V2_DOMAIN_HEAP, 0);
        assert(held[i]->count == 1 && held[i]->descriptor == {index});
        *(uint64_t *)hirsel_rt_v2_payload(held[i]) = i;
      }}
      for (uint64_t i = 0; i < 3000; i++) {{
        uint64_t r = 0;
        assert(call_churn(ctx, &r, round * 3000 + i) == 0 && r == round * 3000 + i);
      }}
      for (unsigned i = round % 2; i < 5000; i += 2)
        assert(hirsel_rt_v2_release(ctx, held[i], {index}, NULL, 0, 0) == 0);
      for (uint64_t i = 0; i < 3000; i++) {{
        uint64_t r = 0;
        assert(call_churn(ctx, &r, i) == 0);
      }}
      for (unsigned i = 1 - round % 2; i < 5000; i += 2) {{
        assert(*(uint64_t *)hirsel_rt_v2_payload(held[i]) == i);
        assert(hirsel_rt_v2_release(ctx, held[i], {index}, NULL, 0, 0) == 0);
      }}
      hirsel_rt_v2_stats stats;
      hirsel_rt_v2_ctx_stats(ctx, &stats);
      assert(stats.live_blocks == 0 && stats.idle_slab_bytes == stats.slab_bytes);
      assert(*(uint64_t *)((char *)ctx + HIRSEL_RT_V2_CTX_IDLE_SLABS_OFFSET) * HIRSEL_RT_V2_SLAB_SIZE
             == stats.idle_slab_bytes);
      hirsel_rt_v2_trim(ctx);
    }}
    void *page = *(void **)((char *)ctx + HIRSEL_RT_V2_CTX_PAGES_OFFSET);
    assert((page != NULL) == !diagnostic);
    hirsel_rt_v2_ctx_destroy(ctx);
  }}
  puts("heap fast path ok");
}}
"#
        );
        for opt in [OptLevel::None, OptLevel::Speed] {
            let dir = scratch(&format!("heap-fast-path-{}-{opt:?}", target.triple));
            let source = dir.join("heap.c");
            std::fs::write(&source, &c).unwrap();
            let input = input();
            let object = emit_object(&p.units(), &inputs(&p, &input, opt)).unwrap();
            let executable = link(
                &target,
                &dir,
                &[("heap.o", &object.bytes), ("program-glue.o", &p.glue())],
                &[source],
            );
            assert_eq!(run(&target, &executable), "heap fast path ok\n");
        }
    }
}
