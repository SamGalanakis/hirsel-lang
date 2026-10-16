// Tree construction/checking adapted from Benchmarks Game C gcc #3 by Jeremy
// Zerfas, based on Jon Harrop, Alex Mizrahi and Bruno Coutinho. BSD-3-Clause;
// full notice in third-party/NOTICES.md. Pool epochs follow gcc #2/#3.
// https://benchmarksgame-team.pages.debian.net/benchmarksgame/program/binarytrees-gcc-3.html
// APR development files are absent on the timing host. This minimal pool keeps
// linked 64 KiB chunks, bump-allocates individual nodes, and retains chunks on
// clear. It never precomputes tree sizes or replaces nodes with an index array.
enum { TREE_CHUNK_NODES = 4096 };
typedef struct TreeChunk {
    struct TreeChunk *next;
    Tree nodes[TREE_CHUNK_NODES];
} TreeChunk;
typedef struct {
    TreeChunk *first, *current;
    size_t used, reserved;
} TreeArena;
#ifdef STATS
static size_t tree_reserved;
#endif

static Tree *arena_node(TreeArena *arena) {
    if (!arena->current || arena->used == TREE_CHUNK_NODES) {
        TreeChunk *chunk = arena->current ? arena->current->next : arena->first;
        if (!chunk) {
            chunk = malloc(sizeof(*chunk));
            if (!chunk) abort();
            chunk->next = NULL;
            if (arena->current) arena->current->next = chunk;
            else arena->first = chunk;
            arena->reserved += sizeof(*chunk);
#ifdef STATS
            tree_reserved += sizeof(*chunk);
            PEAK(tree_reserved);
#endif
        }
        arena->current = chunk;
        arena->used = 0;
    }
    COUNT(0, 1);
    COUNT(1, sizeof(Tree));
    return &arena->current->nodes[arena->used++];
}

static void clear_tree_arena(TreeArena *arena) {
    COUNT(7, arena->reserved);
    arena->current = arena->first;
    arena->used = 0;
}

static void drop_tree_arena(TreeArena *arena) {
    TreeChunk *chunk = arena->first;
    while (chunk) {
        TreeChunk *next = chunk->next;
        free(chunk);
        chunk = next;
    }
#ifdef STATS
    assert(tree_reserved >= arena->reserved);
    tree_reserved -= arena->reserved;
#endif
    *arena = (TreeArena){0};
}

static Tree *arena_tree(size_t depth, TreeArena *arena) {
    Tree *root = arena_node(arena);
    if (depth) {
        root->a = arena_tree(depth - 1, arena);
        root->b = arena_tree(depth - 1, arena);
    } else root->a = root->b = NULL;
    return root;
}

static uint64_t check_arena_tree(const Tree *root) {
    return root->a ? add(add(check_arena_tree(root->a), check_arena_tree(root->b)), 1) : 1;
}

static uint64_t binary(size_t depth) {
    assert(depth >= 4 && depth <= 20);
    TreeArena retained = {0}, temporary = {0};
    Tree *long_tree = arena_tree(depth, &retained);
    uint64_t total = check_arena_tree(arena_tree(depth + 1, &temporary));
    clear_tree_arena(&temporary);
    for (size_t d = 4; d <= depth; d += 2) {
        for (size_t i = 0; i < ((size_t)1 << (depth - d)); i++) {
            total = add(total, check_arena_tree(arena_tree(d, &temporary)));
            clear_tree_arena(&temporary);
        }
    }
    total = add(total, check_arena_tree(long_tree));
    drop_tree_arena(&temporary);
    drop_tree_arena(&retained);
    return total;
}
