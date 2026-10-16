import Rc.Drain

/-!
# Last-use release

Last-use count release (Perceus) is a proof obligation, and MODE-2 waits on it
before uniqueness may end at a counted share's last use. In the model a holder's credit may
be released as soon as its last loan has ended (no live borrow is guarded by it): the release
step has no scope-end precondition at all. These theorems state that such a release is legal,
safe, disturbs no other holder, borrow or local, and is the ideal release step. They also
state the consequence MODE-2 would rely on: after the last-use release of one of two shares,
the survivor's header passes the uniqueness test.

M1b changes no source rule: relaxing MODE-2 is a separate spec decision that may cite these
theorems.
-/

namespace Rc

section
variable {cfg : Config} {s : State}

/-- **Last-use release is safe.** A root whose loans have all ended (no live borrow is guarded
by it) can release its credit immediately, before its scope ends: the step is legal,
preserves the invariant (hence holder and borrower safety), keeps every other credit, borrow
and local, and is exactly the ideal release step. -/
theorem last_use_release (hok : cfg.Ok) (hi : Inv cfg s) {r : Nat} {c : Credit}
    (hc : rootOf s r = some c) (hloans : ∀ b ∈ s.borrows, b.guard ≠ .root r) :
    ∃ s', step cfg s (.drop r) = .ok s' ∧ Inv cfg s' ∧
      (∀ d ∈ s.credits, d ≠ c → d ∈ s'.credits) ∧ s'.borrows = s.borrows ∧
      s'.locals = s.locals ∧ abs s' = irelease (abs s) c ∧ IStep (abs s) (abs s') := by
  obtain ⟨hcm, hch⟩ := hi.rootOf_mem hc
  have hg : guarded s (.root r) = false := by
    simp only [guarded, List.any_eq_false]; intro b hb he; exact hloans b hb (by simpa using he)
  have hstep : step cfg s (.drop r) = .ok (Rc.release s c) := by
    simp [step, opDrop, hc, hg]
  refine ⟨Rc.release s c, hstep, step_inv hok hi hstep, ?_, by simp, by simp,
    hi.abs_release hcm, ?_⟩
  · intro d hd hne; rw [release_credits]; exact mem_erase_of_mem_ne hd hne
  · rw [hi.abs_release hcm]; exact IStep.release hcm (by rw [hch]; exact hloans)

/-- **What MODE-2 would rely on.** If a value has exactly two holders and is not saturated,
releasing one of them at its last use leaves the other as the unique holder: the header is
exactly one, which is the in-place update and reuse test. -/
theorem last_use_enables_unique (hi : Inv cfg s) {c d : Credit} (hc : c ∈ s.credits)
    (hd : d ∈ s.credits) (hcd : c ≠ d) (ht : d.tgt = c.tgt) (h2 : icount s c.tgt = 2)
    (hnimm : (s.heap c.tgt).cnt ≠ .immortal) :
    ((Rc.release s c).heap d.tgt).cnt = .fin 1 ∧ d ∈ (Rc.release s c).credits ∧
      icount (Rc.release s c) d.tgt = 1 := by
  have ⟨hl, _, hk⟩ := hi.tgtOk c hc
  rcases hi.counts c.tgt hl hk with himm | ⟨n, hn, _, _, hic⟩
  · exact absurd himm hnimm
  · rw [h2] at hic; subst hic
    have hrel : Rc.release s c =
        { s with credits := s.credits.erase c,
                 heap := upd s.heap c.tgt { s.heap c.tgt with cnt := .fin 1 } } := by
      simp [Rc.release, hn]
    have e := icount_erase hc c.tgt
    simp only [ite_true] at e
    refine ⟨?_, ?_, ?_⟩
    · rw [hrel, ht]; simp
    · rw [release_credits]; exact mem_erase_of_mem_ne hd (Ne.symm hcd)
    · rw [ht]; simp only [icount, release_credits]; simp only [icount] at h2; omega

end

end Rc
