import Rc.Ideal

/-!
# Safety, token linearity, region conservation and finalizer theorems
-/

namespace Rc

section
variable {cfg : Config} {s s' : State}

/-! ## Holder and borrower safety -/

/-- **Holder safety**: every credit is on a live allocation of its own generation. -/
theorem holder_safety (hi : Inv cfg s) :
    ∀ c ∈ s.credits, (s.heap c.tgt).st = .live ∧ (s.heap c.tgt).gen = c.gen :=
  fun c hc => ⟨(hi.tgtOk c hc).1, (hi.tgtOk c hc).2.1⟩

/-- **Borrower safety**: every borrow views a live allocation of its own generation, guarded
by a credit that is still held; every local views a live region block. -/
theorem borrower_safety (hi : Inv cfg s) :
    (∀ b ∈ s.borrows, (s.heap b.tgt).st = .live ∧ (s.heap b.tgt).gen = b.gen ∧
      ∃ c ∈ s.credits, c.h = b.guard ∧ c.tgt = b.tgt) ∧
    (∀ l ∈ s.locals, (s.heap l.tgt).st = .live ∧ (s.heap l.tgt).gen = l.gen) := by
  refine ⟨fun b hb => ?_, fun l hl => ⟨(hi.lTgt l hl).1, (hi.lTgt l hl).2.1⟩⟩
  obtain ⟨c, hc, h1, h2, h3⟩ := hi.bGuard b hb
  have := hi.tgtOk c hc
  rw [h2] at this
  exact ⟨this.1, this.2.1.trans h3, c, hc, h1, h2⟩

/-- **Every step preserves holder and borrower safety.** -/
theorem step_safety (hok : cfg.Ok) (hi : Inv cfg s) {op : Op} (h : step cfg s op = .ok s') :
    (∀ c ∈ s'.credits, (s'.heap c.tgt).st = .live ∧ (s'.heap c.tgt).gen = c.gen) ∧
    (∀ b ∈ s'.borrows, (s'.heap b.tgt).st = .live ∧ (s'.heap b.tgt).gen = b.gen ∧
      ∃ c ∈ s'.credits, c.h = b.guard ∧ c.tgt = b.tgt) ∧
    (∀ l ∈ s'.locals, (s'.heap l.tgt).st = .live ∧ (s'.heap l.tgt).gen = l.gen) := by
  have hi' := step_inv hok hi h
  exact ⟨holder_safety hi', borrower_safety hi'⟩

/-- **No step frees or reuses a reachable allocation**: every holder, borrow or local that
survives a step still sees a live allocation of the same generation. -/
theorem step_no_reuse_of_reachable (hok : cfg.Ok) (hi : Inv cfg s) {op : Op}
    (h : step cfg s op = .ok s') :
    (∀ c ∈ s.credits, c ∈ s'.credits →
      (s'.heap c.tgt).st = .live ∧ (s'.heap c.tgt).gen = (s.heap c.tgt).gen) ∧
    (∀ b ∈ s.borrows, b ∈ s'.borrows →
      (s'.heap b.tgt).st = .live ∧ (s'.heap b.tgt).gen = (s.heap b.tgt).gen) ∧
    (∀ l ∈ s.locals, l ∈ s'.locals →
      (s'.heap l.tgt).st = .live ∧ (s'.heap l.tgt).gen = (s.heap l.tgt).gen) := by
  have hi' := step_inv hok hi h
  refine ⟨fun c hc hc' => ?_, fun b hb hb' => ?_, fun l hl hl' => ?_⟩
  · have h1 := holder_safety hi c hc; have h2 := holder_safety hi' c hc'
    exact ⟨h2.1, h2.2.trans h1.2.symm⟩
  · have h1 := (borrower_safety hi).1 b hb; have h2 := (borrower_safety hi').1 b hb'
    exact ⟨h2.1, h2.2.1.trans h1.2.1.symm⟩
  · have h1 := (borrower_safety hi).2 l hl; have h2 := (borrower_safety hi').2 l hl'
    exact ⟨h2.1, h2.2.trans h1.2.symm⟩

/-! ## Counts never wrap -/

/-- Every finite header lies in `1 .. max-1` and equals the ideal count; a header that is not
finite is `Immortal`. A live counted block is never `Dead`. -/
theorem counts_track (hi : Inv cfg s) {a : Nat} (hl : (s.heap a).st = .live)
    (hk : (s.heap a).kind.counted = true) :
    (s.heap a).cnt = .immortal ∨
      ∃ n, (s.heap a).cnt = .fin n ∧ 1 ≤ n ∧ n < cfg.max ∧ icount s a = n :=
  hi.counts a hl hk

/-- **No step wraps a count**: after any legal step every finite header is in `1 .. max-1`. -/
theorem step_no_wrap (hok : cfg.Ok) (hi : Inv cfg s) {op : Op} (h : step cfg s op = .ok s') :
    ∀ a n, (s'.heap a).st = .live → (s'.heap a).kind.counted = true →
      (s'.heap a).cnt = .fin n → 1 ≤ n ∧ n < cfg.max := by
  intro a n hl hk hn
  rcases (step_inv hok hi h).counts a hl hk with h1 | ⟨m, hm, h1, h2, _⟩
  · rw [hn] at h1; cases h1
  · rw [hn] at hm; cases hm; exact ⟨h1, h2⟩

/-- The header increment never produces a finite count at or above `max`. -/
theorem incr_bounded (n : Nat) : ∀ m, Count.incr cfg (.fin n) = .fin m → m < cfg.max := by
  intro m h
  simp only [Count.incr] at h
  split at h
  · cases h; assumption
  · cases h

/-- Every finite result of a production increment fits below the u32 sentinel. -/
theorem production_incr_bounded (n m : Nat)
    (h : Count.incr Config.production (.fin n) = .fin m) : m < 4294967295 :=
  incr_bounded n m h

/-- The production sentinel is sticky, just as every model configuration's. -/
theorem production_sentinel_sticky : Count.incr Config.production .immortal = .immortal := rfl

/-- Resources (finalizable values) never saturate: they are noncopyable. -/
theorem resource_never_immortal (hi : Inv cfg s) {a : Nat} (hl : (s.heap a).st = .live)
    (hf : (s.heap a).fin = true) : (s.heap a).cnt ≠ .immortal :=
  hi.finNotImm a hl hf

/-- A unique header is exact: count one means exactly one credit and no saturation, so the
in-place update and reuse tests are sound. -/
theorem unique_header_exact (hi : Inv cfg s) {c : Credit} (hc : c ∈ s.credits)
    (h1 : (s.heap c.tgt).cnt = .fin 1) : ∀ d ∈ s.credits, d.tgt = c.tgt → d = c :=
  fun _ hd ht => hi.only_credit hc hd h1 ht

/-! ## Token linearity -/

/-- **Token linearity (state)**: raw storage is owned by exactly one token, a pending reset
by exactly one token, no two tokens share storage, and every token is backed by its storage. -/
theorem token_linearity (hi : Inv cfg s) :
    (∀ a, (s.heap a).st = .raw →
      ∃ tk ∈ s.tokens, tk.val = .raw a (s.heap a).gen (s.heap a).arity ∧
        ∀ u ∈ s.tokens, u.val.addr = some a → u = tk) ∧
    (∀ a t, (s.heap a).st = .dying → (s.heap a).reset = some t →
      ∃ tk ∈ s.tokens, tk.id = t ∧ tk.val = .pending a ∧
        ∀ u ∈ s.tokens, u.val.addr = some a → u = tk) ∧
    (∀ t ∈ s.tokens, TokOk s t) ∧ (s.tokens.map Token.id).Nodup := by
  refine ⟨?_, ?_, hi.tokOk, hi.nodupT⟩
  · intro a hr
    obtain ⟨tk, htk, hv⟩ := hi.rawTok a hr
    exact ⟨tk, htk, hv, fun u hu he => hi.tokAddr u hu tk htk a he (by rw [hv]; rfl)⟩
  · intro a t hd hrs
    obtain ⟨tk, htk, h1, hv⟩ := hi.resetTok a t hd hrs
    exact ⟨tk, htk, h1, hv, fun u hu he => hi.tokAddr u hu tk htk a he (by rw [hv]; rfl)⟩

/-- **Reuse consumes exactly its token**, which then no longer exists. -/
theorem reuse_consumes (hi : Inv cfg s) {t r arity : Nat} {fin : Bool}
    (h : step cfg s (.reuse t r arity fin) = .ok s') :
    ∃ tok ∈ s.tokens, tok.id = t ∧ s'.tokens = s.tokens.erase tok ∧
      ∀ u ∈ s'.tokens, u.id ≠ t := by
  simp only [step, opReuse] at h
  split at h
  · cases h
  · rename_i tok htok
    obtain ⟨htm, hid⟩ := tokenOf_mem htok
    have hgone : ∀ u ∈ s.tokens.erase tok, u.id ≠ t := by
      intro u hu he
      have hu' := (hi.tokens_nodup.mem_erase_iff).mp hu
      exact hu'.1 (hi.token_eq_of_id hu'.2 htm (by rw [he, hid]))
    split at h
    · cases h
    · split at h
      · cases h; exact ⟨tok, htm, hid, rfl, hgone⟩
      · cases h
      · split at h
        · cases h
        · cases h; exact ⟨tok, htm, hid, rfl, hgone⟩

/-- **Dispose consumes exactly its token**, which then no longer exists. -/
theorem dispose_consumes (hi : Inv cfg s) {t : Nat} (h : step cfg s (.dispose t) = .ok s') :
    ∃ tok ∈ s.tokens, tok.id = t ∧ s'.tokens = s.tokens.erase tok ∧
      ∀ u ∈ s'.tokens, u.id ≠ t := by
  simp only [step, opDispose] at h
  split at h
  · cases h
  · rename_i tok htok
    obtain ⟨htm, hid⟩ := tokenOf_mem htok
    have hgone : ∀ u ∈ s.tokens.erase tok, u.id ≠ t := by
      intro u hu he
      have hu' := (hi.tokens_nodup.mem_erase_iff).mp hu
      exact hu'.1 (hi.token_eq_of_id hu'.2 htm (by rw [he, hid]))
    split at h
    · cases h; exact ⟨tok, htm, hid, rfl, hgone⟩
    · cases h
    · cases h; exact ⟨tok, htm, hid, rfl, hgone⟩

/-- A token cannot be consumed twice. -/
theorem token_used_once (hi : Inv cfg s) {t r arity : Nat} {fin : Bool} {op : Op}
    (h : step cfg s op = .ok s') (hop : op = .reuse t r arity fin ∨ op = .dispose t) :
    (∀ r' arity' fin', ∃ e, step cfg s' (.reuse t r' arity' fin') = .error e) ∧
      ∃ e, step cfg s' (.dispose t) = .error e := by
  have hgone : ∀ u ∈ s'.tokens, u.id ≠ t := by
    rcases hop with rfl | rfl
    · obtain ⟨_, _, _, _, h4⟩ := reuse_consumes hi h; exact h4
    · obtain ⟨_, _, _, _, h4⟩ := dispose_consumes hi h; exact h4
  have hnone : tokenOf s' t = none := by
    simp only [tokenOf, List.find?_eq_none]; intro u hu he; exact hgone u hu (by simpa using he)
  refine ⟨fun r' arity' fin' => ?_, ?_⟩
  · simp only [step, opReuse, hnone]; exact ⟨_, rfl⟩
  · simp only [step, opDispose, hnone]; exact ⟨_, rfl⟩

/-- Reset produces exactly one token, under its fresh id: pending when the donor was unique,
`none` otherwise. -/
theorem reset_produces {b t : Nat} (h : step cfg s (.reset b t) = .ok s') :
    (∀ u ∈ s.tokens, u.id ≠ t) ∧ ∃ v, s'.tokens = ⟨t, v⟩ :: s.tokens ∧
      (v = .none ∨ ∃ a, v = .pending a) := by
  simp only [step, opReset] at h
  split at h
  · cases h
  · rename_i cb _
    split at h
    · cases h
    · split at h
      · cases h
      · rename_i ht
        refine ⟨tokenOf_none (by simpa using ht), ?_⟩
        split at h
        · cases h
          exact ⟨.pending cb.tgt, by simp [Rc.markReset], Or.inr ⟨_, rfl⟩⟩
        · cases h; exact ⟨_, by simp, Or.inl rfl⟩

/-! ## Region-exit credit conservation -/

theorem countP_foldl_erase {α : Type} [DecidableEq α] (p : α → Bool) :
    ∀ (l cs : List α), l.Nodup → (∀ c ∈ l, c ∈ cs) →
      (l.foldl List.erase cs).countP p + l.countP p = cs.countP p
  | [], cs, _, _ => by simp
  | c :: l, cs, hn, hm => by
    rw [List.nodup_cons] at hn
    simp only [List.foldl_cons, List.countP_cons]
    have ih := countP_foldl_erase p l (cs.erase c) hn.2 (fun d hd =>
      mem_erase_of_mem_ne (hm d (List.mem_cons_of_mem _ hd)) (fun he => hn.1 (he ▸ hd)))
    have e := countP_erase_add p cs c (hm c List.mem_cons_self)
    omega

/-- **Region-exit credit conservation**: closing a region releases exactly its blocks'
outgoing heap credits, each once; every other credit survives; each target's ideal count
drops by exactly the number of outgoing credits on it; the region's storage becomes free. -/
theorem region_close_conserves (hi : Inv cfg s) {g : Nat}
    (h : step cfg s (.regionClose g) = .ok s') :
    ∃ R rest, s.regions = R :: rest ∧ R.id = g ∧ s'.regions = rest ∧
      (∀ c ∈ outgoing s.credits R.blocks, ∃ src k, c.h = .field src k ∧ src ∈ R.blocks) ∧
      (∀ c ∈ s.credits, (∃ src k, c.h = .field src k ∧ src ∈ R.blocks) →
        c ∈ outgoing s.credits R.blocks) ∧
      (∀ c, c ∈ s'.credits ↔ c ∈ s.credits ∧ c ∉ outgoing s.credits R.blocks) ∧
      (∀ x, icount s x = icount s' x + (outgoing s.credits R.blocks).countP (fun c => c.tgt == x)) ∧
      (∀ x ∈ R.blocks, (s'.heap x).st = .free) := by
  simp only [step, opRegionClose] at h
  split at h
  · cases h
  · rename_i R rest hrs
    split at h
    · cases h
    · rename_i hid
      split at h
      · cases h
      · cases h
        have hcn := hi.credits_nodup
        have hnd : (outgoing s.credits R.blocks).Nodup := hcn.filter _
        have hm : ∀ c ∈ outgoing s.credits R.blocks, c ∈ s.credits :=
          fun c hc => (mem_outgoing.mp hc).1
        have hcred : (freeBlocks ((outgoing s.credits R.blocks).foldl Rc.release s) R.blocks rest).credits =
            (outgoing s.credits R.blocks).foldl List.erase s.credits := by
          simp only [freeBlocks]; exact foldl_release_credits _ _
        refine ⟨R, rest, hrs, by simpa using hid, rfl, fun c hc => (mem_outgoing.mp hc).2,
          fun c hc hs => mem_outgoing.mpr ⟨hc, hs⟩, ?_, ?_, ?_⟩
        · intro c; rw [hcred]; exact mem_foldl_erase _ _ hcn c
        · intro x
          simp only [icount]; rw [hcred]
          exact (countP_foldl_erase _ _ _ hnd hm).symm
        · intro x hx; simp [freeBlocks, hx]

/-! ## Finalizer obligations -/

/-- **Each finalizer obligation is consumed at most once**: no allocation generation has two
finalizer events. -/
theorem finalizer_at_most_once (hi : Inv cfg s) : (s.events.map Event.key).Nodup := hi.nodupE

/-- A dead resource is never retired, nor are its fields released, before its finalizer
event: draining a finalizable block requires its finalizer to have run. -/
theorem drain_after_finalizer (hi : Inv cfg s) {a : Nat} {rest : List Nat}
    (hw : s.work = a :: rest) (hf : (s.heap a).fin = true) (h : step cfg s .drain = .ok s') :
    ∃ ok, Event.finalized a (s.heap a).gen ok ∈ s.events := by
  simp only [step, opDrain, hw] at h
  split at h
  · cases h
  · rename_i hdue
    have hd : (s.heap a).st = .dying := (hi.workIff a).mp (by rw [hw]; exact List.mem_cons_self)
    have hfd : (s.heap a).finDone = true := by simp [hf] at hdue; exact hdue
    exact hi.finDoneEv a hd hfd

end

end Rc
