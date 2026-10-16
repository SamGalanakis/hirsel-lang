import Rc

/-! The published theorem set. `audit.sh` checks that each depends only on Lean's standard
axioms (`propext`, `Classical.choice`, `Quot.sound`). -/

-- safety of every step and of every reachable state
#print axioms Rc.inv_init
#print axioms Rc.step_inv
#print axioms Rc.reachable_inv
#print axioms Rc.holder_safety
#print axioms Rc.borrower_safety
#print axioms Rc.step_safety
#print axioms Rc.step_no_reuse_of_reachable
-- counts: tracking, no wraparound, sticky immortality
#print axioms Rc.counts_track
#print axioms Rc.step_no_wrap
#print axioms Rc.incr_bounded
#print axioms Rc.Config.production_ok
#print axioms Rc.production_incr_bounded
#print axioms Rc.production_sentinel_sticky
#print axioms Rc.immortal_sticky
#print axioms Rc.immortal_never_unique
#print axioms Rc.resource_never_immortal
#print axioms Rc.unique_header_exact
-- refinement of the ideal machine
#print axioms Rc.Inv.abs_release
#print axioms Rc.simulation
-- token linearity
#print axioms Rc.token_linearity
#print axioms Rc.reset_produces
#print axioms Rc.reuse_consumes
#print axioms Rc.dispose_consumes
#print axioms Rc.token_used_once
-- region-exit credit conservation
#print axioms Rc.region_close_conserves
-- finalizer obligations
#print axioms Rc.finalizer_at_most_once
#print axioms Rc.drain_after_finalizer
-- draining a zero-holder DAG
#print axioms Rc.drain_terminates
#print axioms Rc.no_garbage
#print axioms Rc.drains_to_dead
-- last-use release (MODE-2)
#print axioms Rc.last_use_release
#print axioms Rc.last_use_enables_unique
