// GenMC model of the free-mode section gate (crates/hirsel-free/src/section.rs) under RC11,
// with the SC accesses exactly as the kernel writes them. Two workers each run two sections
// that take a version of one shared object and record it in their plain (non-atomic) logs; a
// cutter flushes once and then stops. GenMC's race detector checks that the cutter's plain
// reads never race a section's writes, and the asserts check that every cut is causally
// closed (the recorded versions are exactly 0..n) and that no section opens after the stop.
//
// -DWEAK_ENTER makes the section's `active := 1` a release store instead of SC: the Dekker
// handshake breaks and GenMC must find the race (check.sh expects it to fail).
#include <assert.h>
#include <pthread.h>
#include <stdatomic.h>

#define WORKERS 2
#define OPS 2

#ifdef WEAK_ENTER
#define ENTER_ORDER memory_order_release
#else
#define ENTER_ORDER memory_order_seq_cst
#endif

atomic_int active[WORKERS];
atomic_int cut;
atomic_int stopped;
atomic_int version;

// Written only inside sections, read only by the cutter while the gate is closed.
int records[WORKERS][OPS];
int lengths[WORKERS];
int progress[WORKERS];
int opened_after_stop;

static int enter(int w) {
    for (;;) {
        atomic_store_explicit(&active[w], 1, ENTER_ORDER);
        if (atomic_load_explicit(&cut, memory_order_seq_cst) == 0)
            return 1;
        atomic_store_explicit(&active[w], 0, memory_order_release);
        for (;;) {
            if (atomic_load_explicit(&stopped, memory_order_acquire))
                return 0;
            if (atomic_load_explicit(&cut, memory_order_seq_cst) == 0)
                break;
        }
    }
}

static void leave(int w) { atomic_store_explicit(&active[w], 0, memory_order_release); }

static void *worker(void *arg) {
    int w = (int)(long)arg;
    for (int op = 0; op < OPS; op++) {
        if (!enter(w))
            return 0;
        int v = atomic_fetch_add_explicit(&version, 1, memory_order_relaxed);
        records[w][lengths[w]] = v;
        lengths[w] += 1;
        progress[w] = op + 1;
        leave(w);
    }
    return 0;
}

static void close_gate(void) {
    atomic_store_explicit(&cut, 1, memory_order_seq_cst);
    for (int w = 0; w < WORKERS; w++)
        while (atomic_load_explicit(&active[w], memory_order_seq_cst) == 1)
            ;
}

static void check_closed(void) {
    int seen[WORKERS * OPS] = {0};
    int n = 0;
    for (int w = 0; w < WORKERS; w++) {
        assert(progress[w] == lengths[w]); // progress and records are published together
        for (int i = 0; i < lengths[w]; i++) {
            seen[records[w][i]] = 1;
            n++;
        }
    }
    for (int v = 0; v < n; v++)
        assert(seen[v]); // no hole: every version below the highest recorded is recorded
}

int main(void) {
    pthread_t t[WORKERS];
    for (long w = 0; w < WORKERS; w++)
        pthread_create(&t[w], 0, worker, (void *)w);
    // A flush: close, scan, reopen.
    close_gate();
    check_closed();
    atomic_store_explicit(&cut, 0, memory_order_seq_cst);
    // The stop: close for good.
    close_gate();
    check_closed();
    int total = lengths[0] + lengths[1];
    atomic_store_explicit(&stopped, 1, memory_order_release);
    for (int w = 0; w < WORKERS; w++)
        pthread_join(t[w], 0);
    // Nothing was recorded after the stop's scan.
    assert(lengths[0] + lengths[1] == total);
    return 0;
}
