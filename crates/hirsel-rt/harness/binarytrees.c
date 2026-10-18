/*
 * binarytrees (Computer Language Benchmarks Game shape, single-threaded) in two builds:
 *   default        nodes come from malloc/free (glibc, or mimalloc/jemalloc via LD_PRELOAD),
 *                  16 bytes each unless -DNODE_BYTES pads them;
 *   -DHIRSEL_RT    nodes are counted hirsel_rt_v2 blocks from the size-class allocator, and a
 *                  tree is freed by releasing its root (the MEM-3 worklist drains it). A node is
 *                  an eight-byte header and the 16-byte niche payload B derives: 24 bytes. With
 *                  -DHIRSEL_GLUE_BINDING_HEADER=<header> (written by hirsel-llvm's
 *                  `the_tree_root_is_the_binarytrees_node`) the root is released by the emitted
 *                  entry it binds instead: the compiled drop glue of the node layout.
 * Both print the same output. Usage: binarytrees [max_depth=21]
 */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef HIRSEL_RT
#include "hirsel_rt_v2.h"

/* Node payload, B's null-pointer niche layout of `Leaf | Node(Box[Tree], Box[Tree])`:
 * { left: ref, right: ref }, a Leaf when left is null (right is then unspecified). */
typedef struct {
    hirsel_rt_v2_header *left;
    hirsel_rt_v2_header *right;
} Payload;

/* The lowered drop program of that variant: unless left is non-null skip both steps, else
 * release right and then left. */
#ifdef HIRSEL_GLUE_BINDING_HEADER
#include HIRSEL_GLUE_BINDING_HEADER
#else
#define NODE_DESCRIPTOR_INDEX 2u
static const hirsel_rt_v2_step node_steps[] = {
    {.kind = HIRSEL_RT_V2_STEP_NONNULL, .field = HIRSEL_RT_V2_WHOLE_VALUE, .offset = 0, .span = 2,
     .count_offset = HIRSEL_RT_V2_NO_OFFSET},
    {.kind = HIRSEL_RT_V2_STEP_RELEASE, .field = 1, .offset = offsetof(Payload, right), .count_offset = HIRSEL_RT_V2_NO_OFFSET},
    {.kind = HIRSEL_RT_V2_STEP_RELEASE, .field = 0, .offset = offsetof(Payload, left), .count_offset = HIRSEL_RT_V2_NO_OFFSET},
};
static const hirsel_rt_v2_layout node_layout = {
    HIRSEL_RT_V2_LAYOUT_MAGIC, HIRSEL_RT_V2_LAYOUT_VERSION, sizeof(Payload), 8, NULL, node_steps, 3, 2, 0, 0};
#endif
static hirsel_rt_v2_ctx *ctx;

typedef hirsel_rt_v2_header Node;

static Node *create(int depth) {
    Node *node = hirsel_rt_v2_alloc(ctx, NODE_DESCRIPTOR_INDEX, sizeof(Payload), 8, HIRSEL_RT_V2_DOMAIN_HEAP, 0);
    Payload *payload = (Payload *)((char *)node + HIRSEL_RT_V2_HEADER_SIZE);
    if (depth > 0) {
        payload->left = create(depth - 1);
        payload->right = create(depth - 1);
    } else {
        payload->left = NULL;
    }
    return node;
}

static long check(Node *node) {
    Payload *payload = (Payload *)((char *)node + HIRSEL_RT_V2_HEADER_SIZE);
    return 1 + (payload->left ? check(payload->left) + check(payload->right) : 0);
}

#ifdef HIRSEL_GLUE_BINDING_HEADER
/* The entry hirsel-llvm emits for `fn(take Box[Tree], i64) -> i64`: it releases the tree through
 * the compiled drop glue of this node layout instead of the walk of node_steps. The binding
 * header's glue_destroy calls it through its ABI v1 signature, the i64 result through `out`. */
static void destroy(Node *node) {
    int64_t unused = 0;
    glue_destroy(ctx, &unused, node, 0);
}
#else
static void destroy(Node *node) { hirsel_rt_v2_release(ctx, node, NODE_DESCRIPTOR_INDEX, NULL, 0, 0); }
#endif

#else

#ifndef NODE_BYTES
#define NODE_BYTES 16
#endif

/* NODE_BYTES > 16 pads each node: a control that measures bytes per node apart from allocator
 * code (the prior 48/32-byte blocks are retained as byte-size controls). */
typedef struct Node {
    struct Node *left, *right;
#if NODE_BYTES > 16
    char pad[NODE_BYTES - 16];
#endif
} Node;

static Node *create(int depth) {
    Node *node = malloc(sizeof *node);
    if (depth > 0) {
        node->left = create(depth - 1);
        node->right = create(depth - 1);
    } else {
        node->left = node->right = NULL;
    }
    return node;
}

static long check(Node *node) { return 1 + (node->left ? check(node->left) + check(node->right) : 0); }

static void destroy(Node *node) {
    if (node->left) {
        destroy(node->left);
        destroy(node->right);
    }
    free(node);
}

#endif

int main(int argc, char **argv) {
    int max_depth = argc > 1 ? atoi(argv[1]) : 21;
    if (max_depth < 6) max_depth = 6;
#ifdef HIRSEL_RT
#ifdef HIRSEL_GLUE_BINDING_HEADER
    hirsel_rt_v2_config config = {.abi_version=HIRSEL_RT_V2_ABI_VERSION, .layouts=hirsel_tree_program.layouts, .layout_count=(uint32_t)hirsel_tree_program.count};
#else
    const hirsel_rt_v2_layout *table[] = {&node_layout};
    hirsel_rt_v2_config config = {.abi_version=HIRSEL_RT_V2_ABI_VERSION, .layouts=table, .layout_count=1};
#endif
    if (hirsel_rt_v2_ctx_create(&config, &ctx) != HIRSEL_RT_V2_LAUNCH_OK) return 2;
#endif
    Node *stretch = create(max_depth + 1);
    printf("stretch tree of depth %d\t check: %ld\n", max_depth + 1, check(stretch));
    destroy(stretch);
    Node *long_lived = create(max_depth);
    for (int depth = 4; depth <= max_depth; depth += 2) {
        long iterations = 1L << (max_depth - depth + 4);
        long sum = 0;
        for (long index = 0; index < iterations; index++) {
            Node *tree = create(depth);
            sum += check(tree);
            destroy(tree);
        }
        printf("%ld\t trees of depth %d\t check: %ld\n", iterations, depth, sum);
    }
    printf("long lived tree of depth %d\t check: %ld\n", max_depth, check(long_lived));
    destroy(long_lived);
#ifdef HIRSEL_RT
    hirsel_rt_v2_ctx_destroy(ctx);
#endif
    return 0;
}
