//! Whole-runtime protocol qualification for the final default route
//! (runtime-cutover-50). The composed production-transition driver runs a
//! seed range beyond its own suite on the integrated tree: every seed repeats
//! exactly against its checked transcript, a foreign transcript diverges, and
//! the union covers every required ownership, host, timer, cut and stop vector.
//! Equal peer deadlines also come from an explicit driver witness: two sleeps
//! armed at one virtual instant, delivered together by its clock pump.
use super::runtime50::{Config, Coverage, run};

const FIRST: u64 = 384;
const SEEDS: u64 = 384;

#[test]
fn runtime50_full_protocol_integration() {
    let mut total = Coverage::default();
    let (mut entries, mut stops) = (0, 0);
    for seed in FIRST..FIRST + SEEDS {
        let config = Config::of(seed);
        let first = run(config, None, None);
        assert!(first.first_uncontrolled.is_none(), "seed {seed}");
        let again = run(config, Some(first.transcript.clone()), None);
        assert!(
            again.divergence.is_none(),
            "seed {seed} repeated differently: {:?}",
            again.divergence
        );
        assert_eq!(first.projection, again.projection, "seed {seed}");
        entries += first.transcript.len();
        stops += u64::from(first.projection.lines().any(|l| l.starts_with("stop ")));
        total.add(&first.coverage);
    }
    // Equal peer deadlines through the driver's own clock pump.
    let witness = run(Config::equal_sleeps(0), None, None);
    let again = run(
        Config::equal_sleeps(0),
        Some(witness.transcript.clone()),
        None,
    );
    assert!(again.divergence.is_none(), "{:?}", again.divergence);
    assert!(
        witness.coverage.timer_equal_peers > 0,
        "two sleeps armed at one instant are delivered in one clock step"
    );
    total.add(&witness.coverage);
    let foreign = run(Config::of(FIRST + 1), None, None);
    let checked = run(Config::of(FIRST + 7), Some(foreign.transcript), None);
    assert!(
        checked.divergence.is_some(),
        "a checked transcript is not vacuous"
    );
    let Coverage {
        wake_before_save,
        coalesced,
        completed_wake_inert,
        stale_generation_inert,
        spurious_repark,
        reused_rows,
        yields,
        mutex_handoffs,
        cancel_withdrew,
        claim_beat_cancel,
        shield_kept_request,
        claim_lost,
        shielded_delivery,
        delayed_delivery,
        failed_calls,
        timer_equal_now,
        timer_equal_peers,
        timer_exit,
        clock_steps,
        nested_tail,
        deferred_release,
        flush_cuts,
        stop_prepared,
        stop_claimed,
        stop_returned,
        in_doubt,
        quarantined,
        reclaimed,
        discarded_after_stop,
        contraction_while_held,
        permit_waits,
        shrink_drains,
        stopped_mutex_owner,
        concrete_reuse,
        retained_reuse: _,
        deferred_until_save,
        loans,
        home_reentry,
        home_refused,
        role_waits,
        cancel_at_registration,
        stale_notices,
        credit_waits,
        late_handles,
        credit_held_after_abandon,
        index_equal_now,
        index_equal_peers,
        index_latched,
        index_expired,
        deadline_exit,
    } = total.clone();
    let cells = [
        ("wake before save", wake_before_save),
        ("coalesced wake", coalesced),
        ("completed wake inert", completed_wake_inert),
        ("stale generation inert", stale_generation_inert),
        ("spurious wake re-parks", spurious_repark),
        ("row reuse", reused_rows),
        ("yield", yields),
        ("mutex custody handoff", mutex_handoffs),
        ("cancel withdraws Prepared", cancel_withdrew),
        ("claim beats cancel", claim_beat_cancel),
        ("shield keeps request", shield_kept_request),
        ("withdrawn claim loses", claim_lost),
        ("shielded delivery after cancel", shielded_delivery),
        ("delayed initialized handle", delayed_delivery),
        ("failed host call", failed_calls),
        ("timer equal to now", timer_equal_now),
        ("timers equal to each other", timer_equal_peers),
        ("timer exit", timer_exit),
        ("clock step housekeeping", clock_steps),
        ("phase-root nested tail", nested_tail),
        ("deferred release", deferred_release),
        ("flush cut", flush_cuts),
        ("stop at Prepared", stop_prepared),
        ("stop at claimed call", stop_claimed),
        ("stop at ended, not done", stop_returned),
        ("in doubt", in_doubt),
        ("quarantine", quarantined),
        ("reclaimed at teardown", reclaimed),
        ("discarded after stop", discarded_after_stop),
        ("contraction while held", contraction_while_held),
        ("permit wait", permit_waits),
        ("shrink drain", shrink_drains),
        ("mutex payload held through the stop", stopped_mutex_owner),
        ("concrete slot reuse", concrete_reuse),
        ("delivery deferred until save", deferred_until_save),
        ("home loan under the role", loans),
        ("home re-enters the caller's role", home_reentry),
        ("home idle claim refused", home_refused),
        ("role wait behind a loan", role_waits),
        ("cancel at registration", cancel_at_registration),
        ("stale cancellation notice", stale_notices),
        ("job credit waits FIFO", credit_waits),
        ("late initialized handle quarantined", late_handles),
        ("abandoned job keeps its credit", credit_held_after_abandon),
        ("index deadline equal to now", index_equal_now),
        ("index deadlines equal to each other", index_equal_peers),
        ("index edge before publication latched", index_latched),
        ("index expiry", index_expired),
        ("deadline scope exit on cancel", deadline_exit),
    ];
    let missing: Vec<_> = cells.iter().filter(|c| c.1 == 0).map(|c| c.0).collect();
    assert!(missing.is_empty(), "uncovered vectors: {missing:?}");
    assert!(stops > SEEDS / 4, "{stops} stopped seeds");
    eprintln!(
        "RUNTIME50 integration: seeds {FIRST}..{} repeated exactly, {entries} entries, \
         {stops} stops, {} vectors covered",
        FIRST + SEEDS,
        cells.len()
    );
}
