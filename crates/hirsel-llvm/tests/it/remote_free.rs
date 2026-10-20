//! Cross-worker frees (design §6 Heaps, runtime ABI 26) on both backends: two threads of one
//! run, each on its own worker's heap, free each other's blocks through compiled drop glue.
//! LLVM's inline free must send a block of another heap to the runtime (its page owner is not
//! the context's heap id), and Cranelift's code always calls the runtime; either way the block
//! is published to its owner, which collects it while it allocates, trims and retires.
use crate::support;
use hirsel_lir::emission::{EmitInputs, OptLevel};
use hirsel_lir::{descriptor_registry::DescriptorRegistry, ir::*, layout_types::TargetSpec};
use hirsel_syntax::PassingMode;
use hirsel_types::Primitive;
use support::{
    native::*,
    programs::{Program, payload},
    *,
};

/// `churn(x) = { b = box x; dup b; release b; release b; x }`, whose allocation and free run
/// inline on the calling worker's heap, and `drop_cell(b) = { release b; 0 }`, which frees a
/// block it is given, wherever it was allocated.
fn program(target: &TargetSpec) -> Program {
    let mut schema = Schema::new();
    let word = schema.types.p(Primitive::U64);
    let block = schema.types.boxed(word);
    schema.name("cell", block);
    schema.name("churn", word);
    schema.signature("churn", &[(PassingMode::Borrow, word)], word);
    schema.signature("drop_cell", &[(PassingMode::Take, block)], word);
    let table = schema.build(target);
    let manifest = manifest();
    let payload = payload(&table, "cell");

    let mut u = Unit::new(&table, "churn", "churn");
    let x = u.param(table.layout("churn"));
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
    let churn = seal(&table, &manifest, u.finish());

    let mut u = Unit::new(&table, "drop_cell", "drop_cell");
    let cell = u.param(table.layout("cell"));
    u.builder.unit.values[cell.index()].holding = Holding::Owned;
    u.release(ENTRY, cell, payload);
    let zero = u.int(ENTRY, hirsel_lir::layout_types::ScalarKind::U64, 0);
    u.builder.ret(ENTRY, Some(zero));
    let drop_cell = seal(&table, &manifest, u.finish());

    Program {
        table,
        manifest,
        units: vec![churn, drop_cell],
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
        fuel: false,
    }
}

/// The harness: in production and diagnostic mode,
/// - remote final free against trim: the owner allocates small and large blocks and hands
///   them to a second worker, which frees them through `drop_cell` (small) or the runtime
///   (large) while both churn their own heaps and the owner collects and trims; afterwards
///   both heaps hold no live block and no slab;
/// - an owner retiring, repeatedly (grow and shrink): a fresh worker allocates blocks, hands
///   them to the first and retires while the first frees them into its orphaned heap; the
///   first's scheduling points collect the orphan, and the next worker re-adopts it (same
///   heap id), so eight rounds use two heaps.
fn harness(churn: &str, drop_cell: &str, index: u32) -> String {
    format!(
        r#"
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
{churn}
{drop_cell}
#define BLOCKS 6000
#define LARGE_EVERY 64
static _Atomic(hirsel_rt_v2_header *) slots[BLOCKS];
static atomic_int done;

static uint32_t heap_id(hirsel_rt_v2_ctx *ctx) {{
  return *(uint32_t *)((char *)ctx + HIRSEL_RT_V2_CTX_HEAP_ID_OFFSET);
}}
static void churn_on(hirsel_rt_v2_ctx *ctx, uint64_t i) {{
  uint64_t r = 0;
  assert(call_churn(ctx, &r, i) == 0 && r == i);
}}
static void drop_block(hirsel_rt_v2_ctx *ctx, hirsel_rt_v2_header *block, uint64_t i) {{
  assert(*(uint64_t *)hirsel_rt_v2_payload(block) == i);
  if (i % LARGE_EVERY == 0) {{
    assert(hirsel_rt_v2_release(ctx, block, {index}, NULL, 0, 0) == 0);
  }} else {{
    uint64_t r = 1;
    assert(call_drop_cell(ctx, &r, block) == 0 && r == 0);
  }}
}}
static hirsel_rt_v2_header *make(hirsel_rt_v2_ctx *ctx, uint64_t i) {{
  uint64_t extent = i % LARGE_EVERY == 0 ? 8192 : 8;
  hirsel_rt_v2_header *block = hirsel_rt_v2_alloc(ctx, {index}, extent, 8, HIRSEL_RT_V2_DOMAIN_HEAP, 0);
  assert(block->count == 1);
  *(uint64_t *)hirsel_rt_v2_payload(block) = i;
  return block;
}}
static void empty(hirsel_rt_v2_ctx *ctx) {{
  hirsel_rt_v2_worker_collect(ctx);
  hirsel_rt_v2_trim(ctx);
  hirsel_rt_v2_stats stats;
  hirsel_rt_v2_ctx_stats(ctx, &stats);
  assert(stats.live_blocks == 0);
  assert(stats.slab_bytes == 0 && stats.large_bytes == 0);
}}

/* Remote final free against trim. */
static hirsel_rt_v2_ctx *owner_ctx, *other_ctx;
static void *owner(void *arg) {{
  (void)arg;
  for (uint64_t i = 0; i < BLOCKS; i++) {{
    atomic_store_explicit(&slots[i], make(owner_ctx, i), memory_order_release);
    churn_on(owner_ctx, i);
    if (i % 97 == 0) {{
      hirsel_rt_v2_worker_collect(owner_ctx);
      hirsel_rt_v2_trim(owner_ctx);
    }}
  }}
  while (!atomic_load(&done)) {{
    hirsel_rt_v2_worker_collect(owner_ctx);
    hirsel_rt_v2_trim(owner_ctx);
  }}
  return NULL;
}}
static void *other(void *arg) {{
  (void)arg;
  for (uint64_t i = 0; i < BLOCKS; i++) {{
    hirsel_rt_v2_header *block;
    while (!(block = atomic_load_explicit(&slots[i], memory_order_acquire))) {{
      churn_on(other_ctx, i);
    }}
    drop_block(other_ctx, block, i);
    atomic_store_explicit(&slots[i], NULL, memory_order_relaxed);
  }}
  atomic_store(&done, 1);
  return NULL;
}}

/* An owner retiring: the worker allocates, hands over and retires at once. */
static hirsel_rt_v2_ctx *leaving_ctx;
static void *leaving(void *arg) {{
  (void)arg;
  for (uint64_t i = 0; i < BLOCKS / 4; i++) {{
    atomic_store_explicit(&slots[i], make(leaving_ctx, i), memory_order_release);
    churn_on(leaving_ctx, i);
  }}
  hirsel_rt_v2_worker_retire(leaving_ctx);
  return NULL;
}}

int main(void) {{
  for (unsigned diagnostic = 0; diagnostic < 2; diagnostic++) {{
    hirsel_rt_v2_config config = {{.abi_version = HIRSEL_RT_V2_ABI_VERSION,
      .flags = diagnostic ? HIRSEL_RT_V2_CONFIG_DIAGNOSTIC : 0, .heap_idle_cap = 1}};
    hirsel_rt_v2_ctx *ctx = NULL;
    assert(hirsel_rt_v2_ctx_create(&config, &ctx) == 0);
    assert(heap_id(ctx) == 1);
    owner_ctx = ctx;
    other_ctx = hirsel_rt_v2_worker_create(ctx);
    assert(heap_id(other_ctx) == 2);
    for (unsigned round = 0; round < 3; round++) {{
      atomic_store(&done, 0);
      pthread_t a, b;
      assert(pthread_create(&a, NULL, owner, NULL) == 0);
      assert(pthread_create(&b, NULL, other, NULL) == 0);
      assert(pthread_join(b, NULL) == 0);
      assert(pthread_join(a, NULL) == 0);
      empty(owner_ctx);
      empty(other_ctx);
    }}
    hirsel_rt_v2_worker_retire(other_ctx);
    for (unsigned round = 0; round < 8; round++) {{
      leaving_ctx = hirsel_rt_v2_worker_create(ctx);
      /* The previous rounds' heap was orphaned and is re-adopted. */
      assert(heap_id(leaving_ctx) == 2);
      pthread_t w;
      assert(pthread_create(&w, NULL, leaving, NULL) == 0);
      for (uint64_t i = 0; i < BLOCKS / 4; i++) {{
        hirsel_rt_v2_header *block;
        while (!(block = atomic_load_explicit(&slots[i], memory_order_acquire))) {{
          churn_on(ctx, i);
        }}
        drop_block(ctx, block, i);
        atomic_store_explicit(&slots[i], NULL, memory_order_relaxed);
        if (i % 101 == 0) hirsel_rt_v2_worker_collect(ctx);
      }}
      assert(pthread_join(w, NULL) == 0);
      /* A scheduling point collects and trims the orphan; then nothing of it is live. */
      hirsel_rt_v2_worker_collect(ctx);
      hirsel_rt_v2_ctx *probe = hirsel_rt_v2_worker_create(ctx);
      assert(heap_id(probe) == 2);
      empty(probe);
      hirsel_rt_v2_worker_retire(probe);
    }}
    empty(ctx);
    hirsel_rt_v2_ctx_destroy(ctx);
  }}
  puts("remote free ok");
}}
"#
    )
}

#[test]
#[ignore = "native: two workers' cross-heap frees on both backends and targets"]
fn workers_free_each_others_blocks_on_both_backends() {
    for target in TargetSpec::executed() {
        let p = program(&target);
        let registry = DescriptorRegistry::new(&p.table.table, &p.finalizers);
        let index = registry.index(payload(&p.table, "cell")).unwrap();
        let c = harness(
            &p.c_call("churn", "call_churn"),
            &p.c_call("drop_cell", "call_drop_cell"),
            index,
        );
        for backend in ["llvm", "cranelift"] {
            for opt in [OptLevel::None, OptLevel::Speed] {
                let dir = scratch(&format!("remote-free-{}-{backend}-{opt:?}", target.triple));
                let source = dir.join("remote.c");
                std::fs::write(&source, &c).unwrap();
                let (input, failures) = (input(), failure_codes());
                let inputs = inputs(&p, &input, &failures, opt);
                let object = if backend == "llvm" {
                    hirsel_llvm::emit_object(&p.units(), &inputs).unwrap().bytes
                } else {
                    hirsel_cranelift::emit_object(&p.units(), &inputs)
                        .unwrap()
                        .bytes
                };
                let executable = link(&target, &dir, &[("remote.o", &object)], &[source]);
                assert_eq!(
                    run(&target, &executable),
                    "remote free ok\n",
                    "{backend} {opt:?} {}",
                    target.triple
                );
            }
        }
    }
}
