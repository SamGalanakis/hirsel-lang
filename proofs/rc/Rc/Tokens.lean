import Rc.Alloc

/-! # Tokens, finalizers and the dead worklist preserve the invariant -/

namespace Rc

section
variable {cfg : Config} {s : State}

theorem Inv.tokens_nodup (hi : Inv cfg s) : s.tokens.Nodup := nodup_of_map hi.nodupT

theorem Inv.token_eq_of_id (hi : Inv cfg s) {t u : Token} (ht : t ∈ s.tokens) (hu : u ∈ s.tokens)
    (h : t.id = u.id) : t = u := eq_of_map_nodup hi.nodupT ht hu h

theorem mem_setTok {ts : List Token} {t : Nat} {v : TokVal} {u : Token} :
    u ∈ setTok ts t v ↔ ∃ u0 ∈ ts, u = if u0.id = t then { u0 with val := v } else u0 := by
  simp only [setTok, List.mem_map]
  constructor
  · rintro ⟨u0, h, rfl⟩; exact ⟨u0, h, rfl⟩
  · rintro ⟨u0, h, rfl⟩; exact ⟨u0, h, rfl⟩

theorem setTok_ids (ts : List Token) (t : Nat) (v : TokVal) :
    (setTok ts t v).map Token.id = ts.map Token.id := by
  simp only [setTok, List.map_map]; congr 1; funext u; by_cases h : u.id = t <;> simp [h]

/-- Nothing but its own token refers to raw storage or a block being retired. -/
theorem Inv.unref_of_not_live (hi : Inv cfg s) {a : Nat} (hl : (s.heap a).st ≠ .live) :
    (∀ c ∈ s.credits, c.tgt ≠ a) ∧ (∀ l ∈ s.locals, l.tgt ≠ a) ∧ (∀ R ∈ s.regions, a ∉ R.blocks) ∧
      (∀ g, (s.heap a).kind ≠ .region g) := by
  refine ⟨?_, ?_, ?_, ?_⟩
  · intro c hc he; exact hl (he ▸ (hi.tgtOk c hc).1)
  · intro l hl' he; exact hl (he ▸ (hi.lTgt l hl').1)
  · intro R hR hm; exact hl (hi.regBlocks R hR a hm).1
  · intro g hk; exact hl (hi.regLive a g hk).1

/-- **Dispose** of raw storage (also the first half of reusing it). -/
theorem Inv.freeRaw (hi : Inv cfg s) {tok : Token} (htok : tok ∈ s.tokens) {a g cap : Nat}
    (hv : tok.val = .raw a g cap) : Inv cfg (Rc.freeRaw s tok a) := by
  have htk := hi.tokOk tok htok
  unfold TokOk at htk; rw [hv] at htk
  obtain ⟨hraw, _, _⟩ := htk
  have hnl : (s.heap a).st ≠ .live := by rw [hraw]; decide
  obtain ⟨hnt, hnl', hnreg, hnk⟩ := hi.unref_of_not_live hnl
  have hns : ∀ c ∈ s.credits, ∀ k, c.h ≠ .field a k := by
    intro c hc k he
    have := (hi.srcOk c hc a k he).1; rw [hraw] at this; rcases this with h | h <;> cases h
  have hlt := hi.live_lt_next (s := s) (a := a) (by rw [hraw]; decide)
  have htn := hi.tokens_nodup
  have hother : ∀ u ∈ s.tokens.erase tok, u.val.addr ≠ some a := by
    intro u hu he
    have hu' := (htn.mem_erase_iff).mp hu
    exact hu'.1 (hi.tokAddr u hu'.2 tok htok a he (by rw [hv]; rfl))
  have hheap : ∀ x, x ≠ a → (Rc.freeRaw s tok a).heap x = s.heap x := by
    intro x hx; simp [Rc.freeRaw, hx]
  have hnew : (Rc.freeRaw s tok a).heap a = { gen := (s.heap a).gen } := by simp [Rc.freeRaw]
  have hmem : ∀ u ∈ (Rc.freeRaw s tok a).tokens, u ∈ s.tokens ∧ u ≠ tok := by
    intro u hu; have := (htn.mem_erase_iff).mp hu; exact ⟨this.2, this.1⟩
  exact {
    frontier := by
      intro x hx; have hx' : s.next ≤ x := hx
      rw [hheap x (by omega)]; exact hi.frontier x hx'
    nodupH := hi.nodupH
    tgtOk := by intro d hd; rw [hheap _ (hnt d hd)]; exact hi.tgtOk d hd
    srcOk := by
      intro d hd src k hh
      have hsrc : src ≠ a := fun he => hns d hd k (he ▸ hh)
      rw [hheap _ (hnt d hd), hheap _ hsrc]; exact hi.srcOk d hd src k hh
    counts := by
      intro x hlx hk
      by_cases hx : x = a
      · subst hx; rw [hnew] at hlx; cases hlx
      · rw [hheap x hx] at hlx hk ⊢; exact hi.counts x hlx hk
    staticImm := by
      intro x hlx hk
      by_cases hx : x = a
      · subst hx; rw [hnew] at hlx; cases hlx
      · rw [hheap x hx] at hlx hk ⊢; exact hi.staticImm x hlx hk
    finNotImm := by
      intro x hlx hf
      by_cases hx : x = a
      · subst hx; rw [hnew] at hlx; cases hlx
      · rw [hheap x hx] at hlx hf ⊢; exact hi.finNotImm x hlx hf
    nodupB := hi.nodupB
    bGuard := hi.bGuard
    bParent := fun b hb => (ParentOk_congr rfl rfl b).mpr (hi.bParent b hb)
    nodupL := hi.nodupL
    lTgt := by intro l hl; rw [hheap _ (hnl' l hl)]; exact hi.lTgt l hl
    nodupW := hi.nodupW
    workIff := by
      intro x
      by_cases hx : x = a
      · subst hx; rw [hnew]
        have : x ∉ s.work := fun hw => by have := (hi.workIff x).mp hw; rw [hraw] at this; cases this
        simp [Rc.freeRaw, this]
      · rw [hheap x hx]; exact hi.workIff x
    dyingHeap := by
      intro x
      by_cases hx : x = a
      · subst hx; rw [hnew]; simp
      · rw [hheap x hx]; exact hi.dyingHeap x
    resetDying := by
      intro x
      by_cases hx : x = a
      · subst hx; rw [hnew]; simp
      · rw [hheap x hx]; exact hi.resetDying x
    nodupT := hi.nodupT.sublist (List.erase_sublist.map _)
    tokOk := by
      intro u hu
      refine (TokOk_congr u ?_).mpr (hi.tokOk u (hmem u hu).1)
      intro x hx; apply hheap; intro he; subst he; exact hother u hu hx
    resetTok := by
      intro x t hd hrs
      by_cases hx : x = a
      · subst hx; rw [hnew] at hd; simp at hd
      · rw [hheap x hx] at hd hrs
        obtain ⟨tk, htk, h1, h2⟩ := hi.resetTok x t hd hrs
        refine ⟨tk, (htn.mem_erase_iff).mpr ⟨?_, htk⟩, h1, h2⟩
        rintro rfl; rw [hv] at h2; cases h2
    rawTok := by
      intro x hrw
      by_cases hx : x = a
      · subst hx; rw [hnew] at hrw; simp at hrw
      · rw [hheap x hx] at hrw ⊢
        obtain ⟨tk, htk, h1⟩ := hi.rawTok x hrw
        refine ⟨tk, (htn.mem_erase_iff).mpr ⟨?_, htk⟩, h1⟩
        rintro rfl; rw [hv] at h1; cases h1; exact hx rfl
    tokAddr := fun t ht u hu => hi.tokAddr t (hmem t ht).1 u (hmem u hu).1
    nodupR := hi.nodupR
    regBlocks := by
      intro R hR x hx
      have : x ≠ a := fun he => hnreg R hR (he ▸ hx)
      rw [hheap x this]; exact hi.regBlocks R hR x hx
    regLive := by
      intro x g' hk
      by_cases hx : x = a
      · subst hx; rw [hnew] at hk; cases hk
      · rw [hheap x hx] at hk ⊢; exact hi.regLive x g' hk
    bornLt := by
      intro x hx
      by_cases hxa : x = a
      · subst hxa; rw [hnew] at hx; simp at hx
      · rw [hheap x hxa] at hx ⊢; exact hi.bornLt x hx
    finDoneLive := by
      intro x
      by_cases hx : x = a
      · subst hx; rw [hnew]; simp
      · rw [hheap x hx]; exact hi.finDoneLive x
    finDoneEv := by
      intro x hd
      by_cases hx : x = a
      · subst hx; rw [hnew] at hd; simp at hd
      · rw [hheap x hx] at hd ⊢; exact hi.finDoneEv x hd
    evOk := by
      intro x g' ok he
      have := hi.evOk x g' ok he
      by_cases hx : x = a
      · subst hx; rw [hnew]; simp; exact this.1
      · rw [hheap x hx]; exact this
    nodupE := hi.nodupE }

/-- Consuming a `none` token. -/
theorem Inv.eraseNoneTok (hi : Inv cfg s) {tok : Token} (hv : tok.val = .none) :
    Inv cfg { s with tokens := s.tokens.erase tok } := by
  have htn := hi.tokens_nodup
  have hmem : ∀ u ∈ s.tokens.erase tok, u ∈ s.tokens := fun u hu => List.mem_of_mem_erase hu
  have hkeep : ∀ u ∈ s.tokens, u.val ≠ .none → u ∈ s.tokens.erase tok := by
    intro u hu hn
    exact (htn.mem_erase_iff).mpr ⟨by rintro rfl; exact hn hv, hu⟩
  exact {
    hi with
    nodupT := hi.nodupT.sublist (List.erase_sublist.map _)
    tokOk := fun u hu => hi.tokOk u (hmem u hu)
    resetTok := by
      intro x t hd hrs
      obtain ⟨tk, htk, h1, h2⟩ := hi.resetTok x t hd hrs
      exact ⟨tk, hkeep tk htk (by rw [h2]; intro h; cases h), h1, h2⟩
    rawTok := by
      intro x hrw
      obtain ⟨tk, htk, h1⟩ := hi.rawTok x hrw
      exact ⟨tk, hkeep tk htk (by rw [h1]; intro h; cases h), h1⟩
    tokAddr := fun t ht u hu => hi.tokAddr t (hmem t ht) u (hmem u hu) }

/-- Producing a `none` token. -/
theorem Inv.addNoneTok (hi : Inv cfg s) {t : Nat} (ht : ∀ u ∈ s.tokens, u.id ≠ t) :
    Inv cfg { s with tokens := ⟨t, .none⟩ :: s.tokens } := by
  exact {
    hi with
    nodupT := by
      simp only [List.map_cons, List.nodup_cons]
      refine ⟨?_, hi.nodupT⟩
      intro hm; obtain ⟨u, hu, hid⟩ := List.mem_map.mp hm; exact ht u hu hid
    tokOk := by
      intro u hu
      rcases List.mem_cons.mp hu with rfl | hu
      · unfold TokOk; trivial
      · exact hi.tokOk u hu
    resetTok := by
      intro x t' hd hrs
      obtain ⟨tk, htk, h1, h2⟩ := hi.resetTok x t' hd hrs
      exact ⟨tk, List.mem_cons_of_mem _ htk, h1, h2⟩
    rawTok := by
      intro x hrw
      obtain ⟨tk, htk, h1⟩ := hi.rawTok x hrw
      exact ⟨tk, List.mem_cons_of_mem _ htk, h1⟩
    tokAddr := by
      intro u hu v hv a hua hva
      rcases List.mem_cons.mp hu with rfl | hu
      · simp [TokVal.addr] at hua
      rcases List.mem_cons.mp hv with rfl | hv
      · simp [TokVal.addr] at hva
      exact hi.tokAddr u hu v hv a hua hva }

@[simp] theorem markReset_credits (s : State) (a t : Nat) : (Rc.markReset s a t).credits = s.credits := rfl
@[simp] theorem markReset_borrows (s : State) (a t : Nat) : (Rc.markReset s a t).borrows = s.borrows := rfl
@[simp] theorem markReset_locals (s : State) (a t : Nat) : (Rc.markReset s a t).locals = s.locals := rfl
@[simp] theorem markReset_work (s : State) (a t : Nat) : (Rc.markReset s a t).work = s.work := rfl
@[simp] theorem markReset_regions (s : State) (a t : Nat) : (Rc.markReset s a t).regions = s.regions := rfl
@[simp] theorem markReset_events (s : State) (a t : Nat) : (Rc.markReset s a t).events = s.events := rfl
@[simp] theorem markReset_next (s : State) (a t : Nat) : (Rc.markReset s a t).next = s.next := rfl
@[simp] theorem markReset_clock (s : State) (a t : Nat) : (Rc.markReset s a t).clock = s.clock := rfl

/-- A reset whose donor died: the dying block carries the pending token. -/
theorem Inv.markReset (hi : Inv cfg s) {a t : Nat} (hd : (s.heap a).st = .dying)
    (hr : (s.heap a).reset = none) (ht : ∀ u ∈ s.tokens, u.id ≠ t) :
    Inv cfg (Rc.markReset s a t) := by
  have hother : ∀ u ∈ s.tokens, u.val.addr ≠ some a := by
    intro u hu he
    have := hi.tokOk u hu
    unfold TokOk at this
    split at this
    · rename_i hv; rw [hv] at he; cases he
    · rename_i a' hv; rw [hv] at he; cases he; rw [hr] at this; cases this.2
    · rename_i a' _ _ hv; rw [hv] at he; cases he; rw [hd] at this; cases this.1
  have hheap : ∀ x, x ≠ a → (Rc.markReset s a t).heap x = s.heap x := by
    intro x hx; simp [Rc.markReset, hx]
  have hnew : (Rc.markReset s a t).heap a = { s.heap a with reset := some t } := by
    simp [Rc.markReset]
  have hlt := hi.live_lt_next (s := s) (a := a) (by rw [hd]; decide)
  have htoks : (Rc.markReset s a t).tokens = ⟨t, .pending a⟩ :: s.tokens := rfl
  -- every field but `reset` agrees
  have hst : ∀ x, ((Rc.markReset s a t).heap x).st = (s.heap x).st := by
    intro x; by_cases hx : x = a
    · subst hx; rw [hnew]
    · rw [hheap x hx]
  have hgen : ∀ x, ((Rc.markReset s a t).heap x).gen = (s.heap x).gen := by
    intro x; by_cases hx : x = a
    · subst hx; rw [hnew]
    · rw [hheap x hx]
  have hkind : ∀ x, ((Rc.markReset s a t).heap x).kind = (s.heap x).kind := by
    intro x; by_cases hx : x = a
    · subst hx; rw [hnew]
    · rw [hheap x hx]
  have hari : ∀ x, ((Rc.markReset s a t).heap x).arity = (s.heap x).arity := by
    intro x; by_cases hx : x = a
    · subst hx; rw [hnew]
    · rw [hheap x hx]
  have hfin : ∀ x, ((Rc.markReset s a t).heap x).fin = (s.heap x).fin := by
    intro x; by_cases hx : x = a
    · subst hx; rw [hnew]
    · rw [hheap x hx]
  have hfd : ∀ x, ((Rc.markReset s a t).heap x).finDone = (s.heap x).finDone := by
    intro x; by_cases hx : x = a
    · subst hx; rw [hnew]
    · rw [hheap x hx]
  have hborn : ∀ x, ((Rc.markReset s a t).heap x).born = (s.heap x).born := by
    intro x; by_cases hx : x = a
    · subst hx; rw [hnew]
    · rw [hheap x hx]
  have hcnt : ∀ x, ((Rc.markReset s a t).heap x).cnt = (s.heap x).cnt := by
    intro x; by_cases hx : x = a
    · subst hx; rw [hnew]
    · rw [hheap x hx]
  have hicount : ∀ x, icount (Rc.markReset s a t) x = icount s x := fun _ => rfl
  exact {
    frontier := by
      intro x hx
      have hx' : s.next ≤ x := hx
      rw [hheap x (by omega)]; exact hi.frontier x hx'
    nodupH := hi.nodupH
    tgtOk := by intro d hd'; simpa [hst, hgen, hkind] using hi.tgtOk d hd'
    srcOk := by intro d hd' src k hh; simpa [hst, hari, hborn] using hi.srcOk d hd' src k hh
    counts := by intro x; simpa [hst, hkind, hcnt, hicount] using hi.counts x
    staticImm := by intro x; simpa [hst, hkind, hcnt] using hi.staticImm x
    finNotImm := by intro x; simpa [hst, hfin, hcnt] using hi.finNotImm x
    nodupB := hi.nodupB
    bGuard := hi.bGuard
    bParent := fun b hb => (ParentOk_congr rfl rfl b).mpr (hi.bParent b hb)
    nodupL := hi.nodupL
    lTgt := by intro l hl; simpa [hst, hgen, hkind] using hi.lTgt l hl
    nodupW := hi.nodupW
    workIff := by intro x; simpa [hst] using hi.workIff x
    dyingHeap := by intro x; simpa [hst, hkind] using hi.dyingHeap x
    resetDying := by
      intro x hx
      by_cases hxa : x = a
      · subst hxa; rw [hst] at hx; exact absurd hd hx
      · rw [hheap x hxa] at hx ⊢; exact hi.resetDying x hx
    nodupT := by
      rw [htoks]; simp only [List.map_cons, List.nodup_cons]
      refine ⟨?_, hi.nodupT⟩
      intro hm; obtain ⟨u, hu, hid⟩ := List.mem_map.mp hm; exact ht u hu hid
    tokOk := by
      intro u hu
      rw [htoks] at hu
      rcases List.mem_cons.mp hu with rfl | hu'
      · unfold TokOk; simp only; rw [hnew]; exact ⟨hd, rfl⟩
      · refine (TokOk_congr u ?_).mpr (hi.tokOk u hu')
        intro x hx; apply hheap; intro he; exact hother u hu' (he ▸ hx)
    resetTok := by
      intro x t' hdx hrs
      rw [htoks]
      by_cases hxa : x = a
      · subst hxa; rw [hnew] at hrs; cases hrs; exact ⟨_, List.mem_cons_self, rfl, rfl⟩
      · rw [hheap x hxa] at hdx hrs
        obtain ⟨tk, htk, h1, h2⟩ := hi.resetTok x t' hdx hrs
        exact ⟨tk, List.mem_cons_of_mem _ htk, h1, h2⟩
    rawTok := by
      intro x hrw
      rw [hst] at hrw
      obtain ⟨tk, htk, h1⟩ := hi.rawTok x hrw
      rw [htoks, hgen, hari]
      exact ⟨tk, List.mem_cons_of_mem _ htk, h1⟩
    tokAddr := by
      intro u hu v hv x hua hva
      rw [htoks] at hu hv
      rcases List.mem_cons.mp hu with rfl | hu' <;> rcases List.mem_cons.mp hv with rfl | hv'
      · rfl
      · simp [TokVal.addr] at hua; subst hua; exact absurd hva (hother v hv')
      · simp [TokVal.addr] at hva; subst hva; exact absurd hua (hother u hu')
      · exact hi.tokAddr u hu' v hv' x hua hva
    nodupR := hi.nodupR
    regBlocks := by intro R hR x hx; simpa [hst, hkind] using hi.regBlocks R hR x hx
    regLive := by intro x g; simpa [hst, hkind] using hi.regLive x g
    bornLt := by intro x; simpa [hst, hborn] using hi.bornLt x
    finDoneLive := by intro x; simpa [hst, hfd] using hi.finDoneLive x
    finDoneEv := by intro x; simpa [hst, hfd, hgen] using hi.finDoneEv x
    evOk := by intro x g ok; simpa [hst, hfd, hgen] using hi.evOk x g ok
    nodupE := hi.nodupE }

end

end Rc
