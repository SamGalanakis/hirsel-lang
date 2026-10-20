// GenMC model of causal closure across tasks under RC11: an old receive against a new send,
// and a spawn's registration and the child's end, both against a flush cut and the stop.
//
// Worker 0 runs the sender's two send sections. Worker 1 runs the receiver: its entered
// section, then done retries under the same ordinal until a message is there. Worker 2 runs a
// parent whose spawn section registers a child, and the child's end section on worker 3 (it
// starts after the spawn, as a stolen task would). The cutter's two cuts must never show a
// receive without its send, or a child's end without its registration and its parent's spawn
// record. Records are plain memory, so GenMC also checks that the cutter never races them.
#include <assert.h>
#include <pthread.h>
#include <stdatomic.h>

#define SLOTS 4

atomic_int active[SLOTS];
atomic_int cut;
atomic_int stopped;
atomic_int queue; // messages sent so far
atomic_int spawned;

int sends;     // records of worker 0
int received;  // record of worker 1: 1 + the message index it took, 0 if none
int registered;
int spawn_record;
int child_end;

static int enter(int w) {
    for (;;) {
        atomic_store_explicit(&active[w], 1, memory_order_seq_cst);
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

static void *sender(void *arg) {
    (void)arg;
    for (int i = 0; i < 2; i++) {
        if (!enter(0))
            return 0;
        atomic_fetch_add_explicit(&queue, 1, memory_order_relaxed);
        sends += 1;
        leave(0);
    }
    return 0;
}

static void *receiver(void *arg) {
    (void)arg;
    if (!enter(1)) // entered: no message needed
        return 0;
    leave(1);
    for (int retry = 0; retry < 2; retry++) {
        if (!enter(1))
            return 0;
        if (atomic_load_explicit(&queue, memory_order_relaxed) > 0) {
            received = 1;
            leave(1);
            return 0;
        }
        leave(1);
    }
    return 0;
}

static void *child(void *arg) {
    (void)arg;
    if (!enter(3))
        return 0;
    child_end = 1;
    leave(3);
    return 0;
}

static void *parent(void *arg) {
    (void)arg;
    if (!enter(2))
        return 0;
    registered = 1;
    spawn_record = 1;
    leave(2);
    pthread_t c;
    pthread_create(&c, 0, child, 0);
    pthread_join(c, 0);
    return 0;
}

static void scan(void) {
    atomic_store_explicit(&cut, 1, memory_order_seq_cst);
    for (int w = 0; w < SLOTS; w++)
        while (atomic_load_explicit(&active[w], memory_order_seq_cst) == 1)
            ;
    if (received)
        assert(sends >= 1); // the receive's message was sent inside the cut
    if (child_end)
        assert(registered && spawn_record);
}

int main(void) {
    pthread_t t[3];
    pthread_create(&t[0], 0, sender, 0);
    pthread_create(&t[1], 0, receiver, 0);
    pthread_create(&t[2], 0, parent, 0);
    scan();
    atomic_store_explicit(&cut, 0, memory_order_seq_cst);
    scan();
    atomic_store_explicit(&stopped, 1, memory_order_release);
    for (int i = 0; i < 3; i++)
        pthread_join(t[i], 0);
    return 0;
}
