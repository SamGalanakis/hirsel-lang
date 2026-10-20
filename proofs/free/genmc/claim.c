// GenMC model of claim versus withdraw (crates/hirsel-free/src/claim.rs) under RC11: one host
// request runs entered, claimed (the CAS Prepared -> ClaimedForIssue), the host call, and done,
// in sections when recording, against a stop at every point. The stop wins the stop word,
// closes the gate (when recording), scans, and sweeps with the CAS Prepared -> Withdrawn.
// Asserts: a withdrawn request never issues; a failed claim was withdrawn; a completion in the
// terminal cut had its call; a claimed request the sweep did not finish is in doubt and its
// late completion is quarantined (never recorded).
//
// -DNO_RECORDING runs without the gate: the CAS alone arbitrates (and models the home bridge,
// whose home thread claims a posted request and whose close sweeps it).
// -DWITHDRAW_BLIND makes the sweep a plain store of Withdrawn: GenMC must find a withdrawn
// request that issued (check.sh expects it to fail).
#include <assert.h>
#include <pthread.h>
#include <stdatomic.h>

enum { PREPARED, CLAIMED, WITHDRAWN, DONE };

atomic_int active;
atomic_int cut;
atomic_int stopped;
atomic_int stop_word;
atomic_int state;
// The request table (Requests in claim.rs): prepare and sweep under its lock; a request
// prepared after the sweep closed the table is born withdrawn.
pthread_mutex_t table = PTHREAD_MUTEX_INITIALIZER;
int listed;
int closed;
int born_withdrawn;

int issued;            // the host call happened (written by the issuer outside sections)
atomic_int issued_seen; // a copy the stopper may read concurrently
int completion;        // the done section's record (plain: read by the cutter only when closed)
int claim_failed;
int quarantined;   // the done section could not open: the completion was dropped

#ifdef NO_RECORDING
static int enter(void) { return atomic_load_explicit(&stopped, memory_order_acquire) == 0; }
static void leave(void) {}
#else
static int enter(void) {
    for (;;) {
        atomic_store_explicit(&active, 1, memory_order_seq_cst);
        if (atomic_load_explicit(&cut, memory_order_seq_cst) == 0)
            return 1;
        atomic_store_explicit(&active, 0, memory_order_release);
        for (;;) {
            if (atomic_load_explicit(&stopped, memory_order_acquire))
                return 0;
            if (atomic_load_explicit(&cut, memory_order_seq_cst) == 0)
                break;
        }
    }
}
static void leave(void) { atomic_store_explicit(&active, 0, memory_order_release); }
#endif

static void *issuer(void *arg) {
    (void)arg;
    // Entered: Prepared.
    if (!enter())
        return 0;
    pthread_mutex_lock(&table);
    if (closed) {
        atomic_store_explicit(&state, WITHDRAWN, memory_order_relaxed);
        born_withdrawn = 1;
    } else
        listed = 1;
    pthread_mutex_unlock(&table);
    leave();
    // Claimed.
    if (!enter())
        return 0;
    int expected = PREPARED;
    if (!atomic_compare_exchange_strong_explicit(&state, &expected, CLAIMED, memory_order_acq_rel,
                                                 memory_order_acquire)) {
        claim_failed = 1;
        leave();
        return 0;
    }
    leave();
    // The host call, outside sections.
    issued = 1;
    atomic_store_explicit(&issued_seen, 1, memory_order_release);
    // Done, or quarantine after the terminal cut.
    if (!enter()) {
        quarantined = 1;
        return 0;
    }
    atomic_store_explicit(&state, DONE, memory_order_release);
    completion = 1;
    leave();
    return 0;
}

int main(void) {
    pthread_t t;
    pthread_create(&t, 0, issuer, 0);
    int zero = 0;
    int won = atomic_compare_exchange_strong_explicit(&stop_word, &zero, 1, memory_order_acq_rel,
                                                      memory_order_acquire);
    assert(won);
    int recorded = 0;
#ifndef NO_RECORDING
    atomic_store_explicit(&cut, 1, memory_order_seq_cst);
    while (atomic_load_explicit(&active, memory_order_seq_cst) == 1)
        ;
    recorded = completion;
#endif
    int withdrawn = 0, in_doubt = 0;
    pthread_mutex_lock(&table);
    closed = 1;
    if (listed) {
#ifdef WITHDRAW_BLIND
        if (atomic_load_explicit(&state, memory_order_acquire) != DONE) {
            atomic_store_explicit(&state, WITHDRAWN, memory_order_release);
            withdrawn = 1;
        }
#else
        int expected = PREPARED;
        withdrawn = atomic_compare_exchange_strong_explicit(&state, &expected, WITHDRAWN,
                                                            memory_order_acq_rel, memory_order_acquire);
        in_doubt = !withdrawn && expected == CLAIMED;
#endif
    }
    pthread_mutex_unlock(&table);
    atomic_store_explicit(&stopped, 1, memory_order_release);
    pthread_join(t, 0);
    assert(!(withdrawn && issued));
    if (claim_failed)
        assert(withdrawn || born_withdrawn);
    assert(!(born_withdrawn && issued));
    if (recorded)
        assert(issued && !in_doubt);
    if (in_doubt)
        assert(!withdrawn);
    // A quarantined completion is never recorded, and its request stays in doubt.
    if (quarantined)
        assert(completion == 0 && !recorded && in_doubt);
    return 0;
}
