/* Standalone cost probe, not a parallel Hirsel executor. Linux x86-64. */
#define _GNU_SOURCE
#include <assert.h>
#include <pthread.h>
#include <sched.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

extern void mc_switch(void **save, void *to);
extern void mc_start(void);
/* Pinned design: Compiled pages begin at 16, fuel at 528; stamp after it.
   Heap::bind's scalar transfer and stamp check are included in stable/refresh. */
struct view {
    uint32_t dup_max, free_inline;
    uint64_t idle_slabs, pages[64], fuel;
    uint32_t worker;
    uint64_t epoch;
};
struct probe {
    void *main_sp, *fiber_sp;
    struct view view, heap;
    uint64_t iterations, elapsed;
    int mode, id, width;
};
static pthread_barrier_t barrier;
static _Atomic unsigned baton;
static uint64_t ns(void) {
    struct timespec t;
    assert(clock_gettime(CLOCK_MONOTONIC_RAW, &t) == 0);
    return (uint64_t)t.tv_sec * 1000000000 + t.tv_nsec;
}
__attribute__((noinline)) static void bind_view(struct probe *p) {
    if (p->mode == 0) return;
    struct view *v = &p->view, *h = &p->heap;
    if (p->mode == 2) h->epoch++;
    v->idle_slabs = h->idle_slabs;
    v->fuel = h->fuel;
    if (v->worker != h->worker || v->epoch != h->epoch) {
        memcpy(v->pages, h->pages, sizeof(v->pages));
        v->worker = h->worker;
        v->epoch = h->epoch;
    }
    __asm__ volatile("" : : "m"(*v) : "memory");
}
__attribute__((noinline)) static void unbind_view(struct probe *p) {
    if (p->mode == 0) return;
    p->heap.idle_slabs = p->view.idle_slabs;
    p->heap.fuel = p->view.fuel;
    __asm__ volatile("" : : "m"(p->heap) : "memory");
}
static void fiber(struct probe *p, void *unused) {
    (void)unused;
    for (;;) {
        unbind_view(p);
        bind_view(p);
        mc_switch(&p->fiber_sp, p->main_sp);
    }
}
static void pin(int id) {
    cpu_set_t set;
    CPU_ZERO(&set);
    /* Eight physical cores, matching B1's worker placement. */
    CPU_SET(31 - id, &set);
    assert(pthread_setaffinity_np(pthread_self(), sizeof(set), &set) == 0);
}
static void *run(void *arg) {
    struct probe *p = arg;
    pin(p->id);
    if (p->mode == 3) {
        pthread_barrier_wait(&barrier);
        uint64_t start = ns();
        for (uint64_t n = 0; n < p->iterations; n++) {
            while (atomic_load_explicit(&baton, memory_order_acquire) != (unsigned)p->id)
                __asm__ volatile("pause");
            atomic_store_explicit(&baton, (p->id + 1) % p->width, memory_order_release);
        }
        p->elapsed = ns() - start;
        return NULL;
    }
    void *stack = aligned_alloc(16, 1 << 20);
    assert(stack);
    uint64_t *sp = (uint64_t *)((char *)stack + (1 << 20)) - 8;
    uint64_t frame[] = {UINT64_C(0x037f00001f80), 0, (uintptr_t)fiber,
                        0, (uintptr_t)p, 0, 0, (uintptr_t)mc_start};
    memcpy(sp, frame, sizeof(frame));
    p->fiber_sp = sp;
    p->heap.worker = p->id + 1;
    for (int i = 0; i < 64; i++) p->heap.pages[i] = i + 1;
    /* Fault stacks in and warm caches before timing. */
    for (int n = 0; n < 1000; n++) {
        unbind_view(p); bind_view(p); mc_switch(&p->main_sp, p->fiber_sp);
    }
    pthread_barrier_wait(&barrier);
    uint64_t start = ns();
    for (uint64_t n = 0; n < p->iterations; n++) {
        unbind_view(p); bind_view(p); mc_switch(&p->main_sp, p->fiber_sp);
    }
    p->elapsed = ns() - start;
    if (p->mode) assert(p->view.pages[63] == 64);
    free(stack);
    return NULL;
}
int main(int argc, char **argv) {
    assert(argc == 4);
    int width = atoi(argv[1]), mode = atoi(argv[2]);
    uint64_t iterations = strtoull(argv[3], NULL, 10);
    assert((width == 4 || width == 8) && mode >= 0 && mode <= 3 && iterations);
    struct probe probes[8] = {0};
    pthread_t threads[8];
    assert(pthread_barrier_init(&barrier, NULL, width) == 0);
    for (int i = 0; i < width; i++) {
        probes[i].mode = mode; probes[i].id = i; probes[i].width = width;
        probes[i].iterations = iterations;
        assert(pthread_create(&threads[i], NULL, run, &probes[i]) == 0);
    }
    uint64_t max = 0, sum = 0;
    for (int i = 0; i < width; i++) {
        assert(pthread_join(threads[i], NULL) == 0);
        if (probes[i].elapsed > max) max = probes[i].elapsed;
        sum += probes[i].elapsed;
    }
    printf("{\"width\":%d,\"mode\":%d,\"iterations\":%lu,\"ns_per_unit\":%.6f,\"checksum\":%lu}\n",
           width, mode, iterations,
           mode == 3 ? (double)max / (width * iterations) : (double)sum / (width * iterations),
           iterations * width);
    pthread_barrier_destroy(&barrier);
}
