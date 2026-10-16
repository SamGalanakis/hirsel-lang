import Rc.LastUse

/-!
# Sticky immortality

A saturated (or static) block stays live and `Immortal`, with its generation, across every
legal step: no wraparound, no later uniqueness success (the uniqueness tests require
`Finite 1`), and never freed or reused.
-/

namespace Rc

/-- A live counted immortal block of `s` is still live, immortal, of the same generation and
kind in `s'`. -/
def Keeps (s s' : State) : Prop :=
  ∀ a, (s.heap a).st = .live → (s.heap a).kind.counted = true → (s.heap a).cnt = .immortal →
    (s'.heap a).st = .live ∧ (s'.heap a).cnt = .immortal ∧ (s'.heap a).gen = (s.heap a).gen ∧
      (s'.heap a).kind = (s.heap a).kind

theorem Keeps.trans {s1 s2 s3 : State} (h1 : Keeps s1 s2) (h2 : Keeps s2 s3) : Keeps s1 s3 := by
  intro a hl hk hi
  obtain ⟨l2, i2, g2, k2⟩ := h1 a hl hk hi
  obtain ⟨l3, i3, g3, k3⟩ := h2 a l2 (by rw [k2]; exact hk) i2
  exact ⟨l3, i3, g3.trans g2, k3.trans k2⟩

theorem keeps_of_heap {s s' : State} (h : s'.heap = s.heap) : Keeps s s' := by
  intro a hl _ hi; rw [h]; exact ⟨hl, hi, rfl, rfl⟩

/-- Changing only blocks that are not live immortal keeps every live immortal block. -/
theorem keeps_upd {s s' : State} {a : Nat} {b : Block}
    (h : s'.heap = upd s.heap a b)
    (ha : (s.heap a).st = .live → (s.heap a).cnt = .immortal →
      b.st = .live ∧ b.cnt = .immortal ∧ b.gen = (s.heap a).gen ∧ b.kind = (s.heap a).kind) :
    Keeps s s' := by
  intro x hl _ hi
  rw [h]
  by_cases hx : x = a
  · subst hx; simp only [upd_same]; exact ha hl hi
  · simp [hx]; exact ⟨hl, hi⟩

theorem keeps_release (s : State) (c : Credit) : Keeps s (Rc.release s c) := by
  unfold Rc.release
  split
  · rename_i n hn
    split
    · exact keeps_upd rfl (fun _ hi => by rw [hn] at hi; cases hi)
    · exact keeps_upd rfl (fun _ hi => by rw [hn] at hi; cases hi)
  · exact keeps_of_heap rfl

theorem keeps_share (cfg : Config) (s : State) (tgt gen r : Nat) :
    Keeps s (share cfg s tgt gen r) := by
  apply keeps_upd rfl
  intro hl hi; simp [hl, hi, Count.incr]

theorem keeps_installCounted (s : State) {a : Nat} (r : Nat) (b : Block)
    (ha : (s.heap a).st = .free) : Keeps s (installCounted s a r b) :=
  keeps_upd rfl (fun hl _ => by rw [ha] at hl; cases hl)

theorem keeps_installRegion (s : State) {a : Nat} (g l arity : Nat)
    (ha : (s.heap a).st = .free) : Keeps s (installRegion s a g l arity) :=
  keeps_upd rfl (fun hl _ => by rw [ha] at hl; cases hl)

theorem keeps_freeRaw (s : State) (tok : Token) {a : Nat} (ha : (s.heap a).st = .raw) :
    Keeps s (freeRaw s tok a) :=
  keeps_upd rfl (fun hl _ => by rw [ha] at hl; cases hl)

theorem keeps_markReset (s : State) (a t : Nat) : Keeps s (markReset s a t) :=
  keeps_upd rfl (fun hl hi => by simp [hl, hi])

theorem keeps_runFinalizer (s : State) (a : Nat) (ok : Bool) : Keeps s (runFinalizer s a ok) :=
  keeps_upd rfl (fun hl hi => by simp [hl, hi])

theorem keeps_retireFree (s : State) {a : Nat} (rest : List Nat) (ha : (s.heap a).st = .dying) :
    Keeps s (retireFree s a rest) :=
  keeps_upd rfl (fun hl _ => by rw [ha] at hl; cases hl)

theorem keeps_retireRaw (s : State) {a : Nat} (t : Nat) (rest : List Nat)
    (ha : (s.heap a).st = .dying) : Keeps s (retireRaw s a t rest) :=
  keeps_upd rfl (fun hl _ => by rw [ha] at hl; cases hl)

theorem keeps_restamp (s : State) (a : Nat) : Keeps s (restamp s a) :=
  keeps_upd rfl (fun hl hi => by simp [hl, hi])

theorem keeps_publish (s : State) (a : Nat) : Keeps s (publish s a) :=
  keeps_upd rfl (fun hl hi => by simp [hl, hi])

theorem keeps_relabel (s : State) (c : Credit) (h : Holder) : Keeps s (relabel s c h) :=
  keeps_of_heap rfl

theorem keeps_displace (s : State) (a k : Nat) : Keeps s (displace s a k) := by
  unfold displace; split
  · exact keeps_of_heap rfl
  · exact keeps_release s _

theorem keeps_foldl_release : ∀ (l : List Credit) (s : State), Keeps s (l.foldl Rc.release s)
  | [], _ => keeps_of_heap rfl
  | c :: l, s => (keeps_release s c).trans (keeps_foldl_release l (Rc.release s c))

theorem keeps_freeBlocks (s : State) (blocks : List Nat) (rest : List Region)
    (hb : ∀ x ∈ blocks, (s.heap x).kind.counted = false) : Keeps s (freeBlocks s blocks rest) := by
  intro x hl hk hi
  have hx : x ∉ blocks := fun hm => by rw [hb x hm] at hk; cases hk
  simp [freeBlocks, hx]; exact ⟨hl, hi⟩

section
variable {cfg : Config} {s s' : State}

/-- **Immortality is sticky**: every legal step keeps every live counted immortal block live,
immortal, and of the same generation. -/
theorem immortal_sticky (hi : Inv cfg s) {op : Op} (h : step cfg s op = .ok s') : Keeps s s' := by
  cases op with
  | alloc r arity fin =>
    simp only [step, opAlloc] at h; split at h
    · cases h
    · cases h; exact keeps_installCounted s r _ (freeAddr_spec hi).1
  | static r =>
    simp only [step, opStatic] at h; split at h
    · cases h
    · cases h; exact keeps_installCounted s r _ (freeAddr_spec hi).1
  | dup src dst =>
    simp only [step, opDup] at h; split at h
    · cases h
    · split at h
      · cases h
      · split at h
        · cases h
        · cases h; exact keeps_share cfg s _ _ _
  | drop r =>
    simp only [step, opDrop] at h; split at h
    · cases h
    · split at h
      · cases h
      · cases h; exact keeps_release s _
  | write b k r =>
    simp only [step, opWrite] at h; split at h
    · cases h
    · split at h
      · cases h
      · split at h
        · cases h
        · split at h
          · cases h
          · split at h
            · cases h
            · split at h
              · cases h
              · split at h
                · cases h
                · cases h
                  exact ((keeps_displace s _ _).trans (keeps_restamp _ _)).trans (keeps_relabel _ _ _)
  | take b k r =>
    simp only [step, opTake] at h; split at h
    · cases h
    · split at h
      · cases h
      · split at h
        · cases h
        · split at h
          · cases h
          · split at h
            · cases h
            · cases h; exact keeps_relabel s _ _
  | load p k r =>
    simp only [step, opLoad] at h; split at h
    · cases h
    · split at h
      · cases h
      · split at h
        · cases h
        · split at h
          · cases h
          · cases h; exact keeps_share cfg s _ _ _
  | borrow id r =>
    simp only [step, opBorrow] at h; split at h
    · cases h
    · split at h
      · cases h
      · cases h; exact keeps_of_heap rfl
  | borrowField id p k =>
    simp only [step, opBorrowField] at h; split at h
    · cases h
    · split at h
      · cases h
      · split at h
        · cases h
        · cases h; exact keeps_of_heap rfl
  | borrowLocal id l k =>
    simp only [step, opBorrowLocal] at h; split at h
    · cases h
    · split at h
      · cases h
      · split at h
        · cases h
        · cases h; exact keeps_publish s _
  | endBorrow id =>
    simp only [step, opEndBorrow] at h; split at h
    · cases h
    · split at h
      · cases h
      · cases h; exact keeps_of_heap rfl
  | reset b t =>
    simp only [step, opReset] at h; split at h
    · cases h
    · split at h
      · cases h
      · split at h
        · cases h
        · split at h
          · cases h; exact (keeps_release s _).trans (keeps_markReset _ _ _)
          · cases h; exact keeps_release s _
  | reuse t r arity fin =>
    simp only [step, opReuse] at h; split at h
    · cases h
    · rename_i tok htok
      split at h
      · cases h
      · split at h
        · cases h; exact keeps_installCounted _ r _ (freeAddr_spec hi).1
        · cases h
        · rename_i a g cap hv
          split at h
          · cases h
          · cases h
            have htk := hi.tokOk tok (tokenOf_mem htok).1
            unfold TokOk at htk; rw [hv] at htk
            exact (keeps_freeRaw s tok htk.1).trans
              (keeps_installCounted _ r _ (by simp [freeRaw]))
  | dispose t =>
    simp only [step, opDispose] at h; split at h
    · cases h
    · rename_i tok htok
      split at h
      · cases h; exact keeps_of_heap rfl
      · cases h
      · rename_i a g cap hv; cases h
        have htk := hi.tokOk tok (tokenOf_mem htok).1
        unfold TokOk at htk; rw [hv] at htk
        exact keeps_freeRaw s tok htk.1
  | finalize ok =>
    simp only [step, opFinalize] at h; split at h
    · cases h
    · split at h
      · cases h
      · cases h; exact keeps_runFinalizer s _ _
  | drain =>
    simp only [step, opDrain] at h; split at h
    · cases h
    · rename_i a rest hw
      have hd := (hi.drained_head hw).1
      split at h
      · cases h
      · split at h
        · cases h; exact keeps_release s _
        · split at h
          · cases h; exact keeps_retireFree s rest hd
          · cases h; exact keeps_retireRaw s _ rest hd
  | forget r =>
    simp only [step, opForget] at h; split at h
    · cases h
    · split at h
      · cases h
      · split at h
        · cases h
        · cases h; exact keeps_relabel s _ _
  | regionOpen g =>
    simp only [step, opRegionOpen] at h; split at h
    · cases h
    · cases h; exact keeps_of_heap rfl
  | regionAlloc g l arity =>
    simp only [step, opRegionAlloc] at h; split at h
    · cases h
    · split at h
      · cases h
      · cases h; exact keeps_installRegion s g l arity (freeAddr_spec hi).1
  | regionStore l k r =>
    simp only [step, opRegionStore] at h; split at h
    · cases h
    · split at h
      · cases h
      · split at h
        · cases h
        · split at h
          · cases h
          · split at h
            · cases h
            · split at h
              · cases h
              · cases h; exact (keeps_restamp s _).trans (keeps_relabel _ _ _)
  | localCopy l l' =>
    simp only [step, opLocalCopy] at h; split at h
    · cases h
    · split at h
      · cases h
      · cases h; exact keeps_publish s _
  | localDrop l =>
    simp only [step, opLocalDrop] at h; split at h
    · cases h
    · split at h
      · cases h
      · cases h; exact keeps_of_heap rfl
  | regionClose g =>
    simp only [step, opRegionClose] at h; split at h
    · cases h
    · rename_i R rest hrs
      split at h
      · cases h
      · split at h
        · cases h
        · cases h
          have hR : R ∈ s.regions := by rw [hrs]; exact List.mem_cons_self
          refine (keeps_foldl_release _ s).trans (keeps_freeBlocks _ _ _ ?_)
          intro x hx
          rw [foldl_release_heap _ s x (by
            intro c hc he
            have := (hi.tgtOk c (mem_outgoing.mp hc).1).2.2
            rw [he, (hi.regBlocks R hR x hx).2] at this; cases this)]
          rw [(hi.regBlocks R hR x hx).2]; rfl

/-- Once immortal, a block never again passes the uniqueness test (`Finite 1`). -/
theorem immortal_never_unique (hi : Inv cfg s) {op : Op} (h : step cfg s op = .ok s') {a : Nat}
    (hl : (s.heap a).st = .live) (hk : (s.heap a).kind.counted = true)
    (himm : (s.heap a).cnt = .immortal) : (s'.heap a).cnt ≠ .fin 1 := by
  rw [(immortal_sticky hi h a hl hk himm).2.1]; intro h; cases h

end

end Rc
