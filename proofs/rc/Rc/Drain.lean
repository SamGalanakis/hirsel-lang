import Rc.Safety

/-!
# Draining: a finite zero-holder DAG drains to dead storage

`drainAll` repeatedly runs the due finalizer of the worklist head, or one `drain` step.
Finalizers are abstract events whose outcome the caller supplies (`outs`); a user finalizer's
own termination is *assumed*, which is what modelling it as one atomic event means.
The measure `3·|credits| + |work| + |due finalizers|` strictly decreases on every step, so
draining terminates with an empty worklist. With an empty worklist, every live counted block
is held: reachable from a root, the forget ledger, a region, or an immortal block. Everything
else is dead storage.
-/

namespace Rc

/-- The block's finalizer obligation is still due. -/
def Block.due (b : Block) : Bool := b.fin && !b.finDone

/-- Worklist entries whose finalizer is still due. -/
def pend (s : State) : Nat := (s.work.filter (fun a => (s.heap a).due)).length

def measure (s : State) : Nat := 3 * s.credits.length + s.work.length + pend s

/-- One draining step: the head's finalizer if due, else one `drain`. -/
def drainOne (cfg : Config) (ok : Bool) (s : State) : Res :=
  match s.work with
  | [] => .ok s
  | a :: _ =>
    if (s.heap a).fin && !(s.heap a).finDone then step cfg s (.finalize ok)
    else step cfg s .drain

/-- Drain with fuel `n`; `outs i` is the outcome of the finalizer run when `i` fuel remains. -/
def drainAll (cfg : Config) (outs : Nat → Bool) : Nat → State → State
  | 0, s => s
  | n + 1, s =>
    if s.work = [] then s
    else match drainOne cfg (outs n) s with
      | .ok s' => drainAll cfg outs n s'
      | .error _ => s

/-- States reachable by legal steps. -/
inductive Reach (cfg : Config) : State → State → Prop
  | refl (s : State) : Reach cfg s s
  | step {s s' s'' : State} {op : Op} : step cfg s op = .ok s' → Reach cfg s' s'' → Reach cfg s s''

section
variable {cfg : Config} {s s' : State}

theorem filter_length_congr {l : List Nat} {p q : Nat → Bool} (h : ∀ x ∈ l, p x = q x) :
    (l.filter p).length = (l.filter q).length := by
  rw [List.filter_congr h]

theorem release_work_cases (s : State) (c : Credit) :
    ((Rc.release s c).work = s.work) ∨
      ((Rc.release s c).work = c.tgt :: s.work ∧ (Rc.release s c).heap c.tgt =
        { s.heap c.tgt with st := .dying, cnt := .dead }) := by
  unfold Rc.release
  split
  · split
    · right; exact ⟨rfl, by simp⟩
    · left; rfl
  · left; rfl

theorem Inv.pend_release (hi : Inv cfg s) {c : Credit} (hc : c ∈ s.credits) :
    pend (Rc.release s c) ≤ pend s + 1 ∧
      ((Rc.release s c).work = s.work → pend (Rc.release s c) = pend s) := by
  have hl := (hi.tgtOk c hc).1
  have hnw : c.tgt ∉ s.work := by
    intro h; have := (hi.workIff _).mp h; rw [hl] at this; cases this
  have hsame : ∀ x ∈ s.work, ((Rc.release s c).heap x).due = (s.heap x).due := by
    intro x hx
    have : x ≠ c.tgt := fun he => hnw (he ▸ hx)
    rw [release_heap_ne s c this]
  rcases release_work_cases s c with hw | ⟨hw, _⟩
  · have : pend (Rc.release s c) = pend s := by
      unfold pend; rw [hw]; exact filter_length_congr hsame
    exact ⟨by omega, fun _ => this⟩
  · refine ⟨?_, fun h => ?_⟩
    · unfold pend; rw [hw, List.filter_cons]
      have := filter_length_congr hsame
      split <;> simp [this]
    · rw [hw] at h; exact absurd h (by simp)

/-- **Every draining step is legal, preserves the invariant and decreases the measure.** -/
theorem Inv.drain_step (hi : Inv cfg s) (hw : s.work ≠ []) (ok : Bool) :
    ∃ s', Rc.drainOne cfg ok s = .ok s' ∧ Inv cfg s' ∧ measure s' < measure s ∧
      ∃ op, step cfg s op = .ok s' := by
  obtain ⟨a, rest, hwr⟩ : ∃ a rest, s.work = a :: rest := by
    cases h : s.work with
    | nil => exact absurd h hw
    | cons a rest => exact ⟨a, rest, rfl⟩
  have hd : (s.heap a).st = .dying := (hi.workIff a).mp (by rw [hwr]; exact List.mem_cons_self)
  have hn := hi.nodupW; rw [hwr, List.nodup_cons] at hn
  have hrest : ∀ x ∈ rest, x ≠ a := fun x hx he => hn.1 (he ▸ hx)
  by_cases hdue : (s.heap a).fin && !(s.heap a).finDone
  · -- run the due finalizer
    have hstep : step cfg s (.finalize ok) = .ok (Rc.runFinalizer s a ok) := by
      simp [step, opFinalize, hwr, hdue]
    have hdue' : (s.heap a).fin = true ∧ (s.heap a).finDone = false := by simpa using hdue
    refine ⟨Rc.runFinalizer s a ok, by simp [Rc.drainOne, hwr, hdue, hstep],
      hi.runFinalizer hwr hdue' ok, ?_, _, hstep⟩
    have hp : pend (Rc.runFinalizer s a ok) + 1 = pend s := by
      unfold pend
      simp only [runFinalizer_work, hwr, List.filter_cons]
      have hcongr : ∀ x ∈ rest, ((Rc.runFinalizer s a ok).heap x).due = (s.heap x).due := by
        intro x hx; simp [Rc.runFinalizer, upd, hrest x hx]
      rw [List.filter_congr hcongr]
      simp [Rc.runFinalizer, upd, Block.due, hdue]
    simp only [measure, runFinalizer_credits, runFinalizer_work]; omega
  · have hdrain : Rc.drainOne cfg ok s = step cfg s .drain := by simp [Rc.drainOne, hwr, hdue]
    rw [hdrain]
    cases hpm : pickMax (fieldsOf s a) with
    | some c =>
      have hstep : step cfg s .drain = .ok (Rc.release s c) := by
        simp [step, opDrain, hwr, hdue, hpm]
      refine ⟨Rc.release s c, hstep, opDrain_inv hi hstep, ?_, _, hstep⟩
      obtain ⟨hcm, _⟩ := mem_fieldsOf.mp (pickMax_mem hpm)
      have hlen : (s.credits.erase c).length + 1 = s.credits.length := by
        rw [List.length_erase_of_mem hcm]
        have := List.length_pos_of_mem hcm
        omega
      have hp := hi.pend_release hcm
      rcases release_work_cases s c with hw' | ⟨hw', _⟩
      · simp only [measure, release_credits, hw', hp.2 hw']; omega
      · simp only [measure, release_credits, hw', List.length_cons]; omega
    | none =>
      cases hr : (s.heap a).reset with
      | none =>
        have hstep : step cfg s .drain = .ok (Rc.retireFree s a rest) := by
          simp [step, opDrain, hwr, hdue, hpm, hr]
        refine ⟨Rc.retireFree s a rest, hstep, opDrain_inv hi hstep, ?_, _, hstep⟩
        have hp : pend (Rc.retireFree s a rest) = pend s := by
          unfold pend
          simp only [Rc.retireFree, hwr, List.filter_cons]
          have hcongr : ∀ x ∈ rest,
              ((upd s.heap a { gen := (s.heap a).gen }) x).due = (s.heap x).due := by
            intro x hx; simp [upd, hrest x hx]
          rw [List.filter_congr hcongr]
          have hd' : (s.heap a).due = false := by simpa [Block.due] using hdue
          simp [hd']
        simp only [measure, hp]; simp [Rc.retireFree, hwr]
      | some t =>
        have hstep : step cfg s .drain = .ok (Rc.retireRaw s a t rest) := by
          simp [step, opDrain, hwr, hdue, hpm, hr]
        refine ⟨Rc.retireRaw s a t rest, hstep, opDrain_inv hi hstep, ?_, _, hstep⟩
        have hp : pend (Rc.retireRaw s a t rest) = pend s := by
          unfold pend
          simp only [Rc.retireRaw, hwr, List.filter_cons]
          have hcongr : ∀ x ∈ rest,
              ((upd s.heap a { st := .raw, gen := (s.heap a).gen, arity := (s.heap a).arity }) x).due =
                (s.heap x).due := by
            intro x hx; simp [upd, hrest x hx]
          rw [List.filter_congr hcongr]
          have hd' : (s.heap a).due = false := by simpa [Block.due] using hdue
          simp [hd']
        simp only [measure, hp]; simp [Rc.retireRaw, hwr]

/-- **Draining terminates**: with fuel at least the measure, and whatever the finalizer
outcomes, legal steps reach an empty worklist while preserving the invariant. -/
theorem drain_terminates (hi : Inv cfg s) (outs : Nat → Bool) :
    ∀ n, measure s ≤ n →
      (drainAll cfg outs n s).work = [] ∧ Inv cfg (drainAll cfg outs n s) ∧
        Reach cfg s (drainAll cfg outs n s) := by
  intro n
  induction n generalizing s with
  | zero =>
    intro hm
    have : s.work.length = 0 := by unfold measure at hm; omega
    exact ⟨List.eq_nil_of_length_eq_zero this, hi, Reach.refl s⟩
  | succ n ih =>
    intro hm
    by_cases hw : s.work = []
    · rw [show drainAll cfg outs (n + 1) s = s by simp [drainAll, hw]]
      exact ⟨hw, hi, Reach.refl s⟩
    · obtain ⟨s1, h1, hi1, hlt, op, hop⟩ := hi.drain_step hw (outs n)
      simp only [drainAll, hw, ite_false, h1]
      obtain ⟨r1, r2, r3⟩ := ih hi1 (by omega)
      exact ⟨r1, r2, Reach.step hop r3⟩

/-! ## No garbage after draining -/

/-- A block is *held* when it is reachable from a root or forget-ledger credit, from a region,
or from an immortal block (the explicit exemptions). -/
inductive Held (s : State) : Nat → Prop
  | ext {c : Credit} : c ∈ s.credits → (∀ src k, c.h ≠ .field src k) → Held s c.tgt
  | region {c : Credit} {src k : Nat} : c ∈ s.credits → c.h = .field src k →
      (s.heap src).kind.counted = false → Held s c.tgt
  | pinned {a : Nat} : (s.heap a).cnt = .immortal → Held s a
  | edge {c : Credit} {src k : Nat} : c ∈ s.credits → c.h = .field src k → Held s src →
      Held s c.tgt

theorem Inv.held_of_live (hi : Inv cfg s) (hw : s.work = []) :
    ∀ n a, s.clock - (s.heap a).born = n → (s.heap a).st = .live →
      (s.heap a).kind.counted = true → Held s a := by
  intro n
  induction n using Nat.strongRecOn with
  | ind n ih =>
    intro a hn hl hk
    rcases hi.counts a hl hk with himm | ⟨m, hm, h1, _, hic⟩
    · exact Held.pinned himm
    · have : 0 < s.credits.countP (fun c => c.tgt == a) := by simp only [icount] at hic; omega
      obtain ⟨c, hc, hct⟩ := List.countP_pos_iff.mp this
      have hct : c.tgt = a := by simpa using hct
      subst hct
      cases hh : c.h with
      | root r => exact Held.ext hc (by intro src k he; rw [hh] at he; cases he)
      | forgot r => exact Held.ext hc (by intro src k he; rw [hh] at he; cases he)
      | field src k =>
        have hs := hi.srcOk c hc src k hh
        have hsl : (s.heap src).st = .live := by
          rcases hs.1 with h | h
          · exact h
          · have := (hi.workIff src).mpr h; rw [hw] at this; cases this
        cases hsk : (s.heap src).kind.counted with
        | false => exact Held.region hc hh hsk
        | true =>
          have hb := hi.bornLt src (by rw [hsl]; decide)
          exact Held.edge hc hh (ih _ (by omega) src rfl hsl hsk)

/-- **No garbage**: once the worklist is empty, every live counted block is held. -/
theorem no_garbage (hi : Inv cfg s) (hw : s.work = []) {a : Nat}
    (hl : (s.heap a).st = .live) (hk : (s.heap a).kind.counted = true) : Held s a :=
  hi.held_of_live hw _ a rfl hl hk

/-- **A finite zero-holder DAG drains to dead storage**: from any reachable state, legal
draining steps (whatever the finalizer outcomes) reach a state with an empty worklist in
which every live counted block is held by a root, the forget ledger, a region or an
immortal block. A block held by none of these is free or raw storage. -/
theorem drains_to_dead (hi : Inv cfg s) (outs : Nat → Bool) :
    let s' := drainAll cfg outs (measure s) s
    Reach cfg s s' ∧ Inv cfg s' ∧ s'.work = [] ∧
      ∀ a, (s'.heap a).st = .live → (s'.heap a).kind.counted = true → Held s' a := by
  obtain ⟨h1, h2, h3⟩ := drain_terminates hi outs (measure s) (Nat.le_refl _)
  exact ⟨h3, h2, h1, fun a hl hk => no_garbage h2 h1 hl hk⟩

end

end Rc
