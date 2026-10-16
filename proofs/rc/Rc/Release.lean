import Rc.Invariant

/-! # Releasing one credit preserves the invariant -/

namespace Rc

section
variable {cfg : Config} {s : State}

theorem ParentOk_congr {s s' : State} (hb : s'.borrows = s.borrows) (hl : s'.locals = s.locals)
    (b : Borrow) : ParentOk s' b ↔ ParentOk s b := by
  unfold ParentOk; rw [hb, hl]

theorem TokOk_congr {s s' : State} (t : Token)
    (h : ∀ a, t.val.addr = some a → s'.heap a = s.heap a) : TokOk s' t ↔ TokOk s t := by
  unfold TokOk
  split <;> simp_all [TokVal.addr]

@[simp] theorem release_borrows (s : State) (c : Credit) : (release s c).borrows = s.borrows := by
  unfold release; split <;> (try split) <;> rfl
@[simp] theorem release_locals (s : State) (c : Credit) : (release s c).locals = s.locals := by
  unfold release; split <;> (try split) <;> rfl
@[simp] theorem release_tokens (s : State) (c : Credit) : (release s c).tokens = s.tokens := by
  unfold release; split <;> (try split) <;> rfl
@[simp] theorem release_regions (s : State) (c : Credit) : (release s c).regions = s.regions := by
  unfold release; split <;> (try split) <;> rfl
@[simp] theorem release_events (s : State) (c : Credit) : (release s c).events = s.events := by
  unfold release; split <;> (try split) <;> rfl
@[simp] theorem release_next (s : State) (c : Credit) : (release s c).next = s.next := by
  unfold release; split <;> (try split) <;> rfl
@[simp] theorem release_clock (s : State) (c : Credit) : (release s c).clock = s.clock := by
  unfold release; split <;> (try split) <;> rfl
@[simp] theorem release_credits (s : State) (c : Credit) :
    (release s c).credits = s.credits.erase c := by
  unfold release; split <;> (try split) <;> rfl

theorem release_heap_ne (s : State) (c : Credit) {x : Nat} (hx : x ≠ c.tgt) :
    (release s c).heap x = s.heap x := by
  unfold release; split <;> (try split) <;> simp [hx]

/-- The released target keeps everything but its state and count. -/
theorem release_heap_tgt (s : State) (c : Credit) :
    let b := s.heap c.tgt
    let b' := (release s c).heap c.tgt
    b'.gen = b.gen ∧ b'.kind = b.kind ∧ b'.arity = b.arity ∧ b'.fin = b.fin ∧
      b'.finDone = b.finDone ∧ b'.reset = b.reset ∧ b'.born = b.born ∧
      b'.published = b.published := by
  unfold release; split <;> (try split) <;> simp

theorem mem_erase_iff' {cs : List Credit} (hn : cs.Nodup) {c d : Credit} :
    d ∈ cs.erase c ↔ d ≠ c ∧ d ∈ cs := hn.mem_erase_iff

/-- Releasing a credit on an immortal block only drops the credit. -/
theorem Inv.release_keep (hi : Inv cfg s) {c : Credit} (hc : c ∈ s.credits)
    (hg : ∀ b ∈ s.borrows, b.guard ≠ c.h) (himm : (s.heap c.tgt).cnt = .immortal) :
    Inv cfg { s with credits := s.credits.erase c } := by
  have hn := hi.credits_nodup
  have hmem : ∀ d, d ∈ s.credits.erase c → d ∈ s.credits := fun d hd => List.mem_of_mem_erase hd
  exact {
    frontier := hi.frontier
    nodupH := nodup_map_erase _ c hi.nodupH
    tgtOk := fun d hd => hi.tgtOk d (hmem d hd)
    srcOk := fun d hd => hi.srcOk d (hmem d hd)
    counts := by
      intro a hl hk
      rcases hi.counts a hl hk with h | ⟨m, hm, h1, h2, h3⟩
      · exact Or.inl h
      · right
        refine ⟨m, hm, h1, h2, ?_⟩
        have e := icount_erase hc a
        by_cases hca : c.tgt = a
        · subst hca; rw [himm] at hm; cases hm
        · simp only [icount] at h3 ⊢; simp [hca] at e; omega
    staticImm := hi.staticImm
    finNotImm := hi.finNotImm
    nodupB := hi.nodupB
    bGuard := by
      intro b hb
      obtain ⟨d, hd, h1, h2, h3⟩ := hi.bGuard b hb
      refine ⟨d, (mem_erase_iff' hn).mpr ⟨?_, hd⟩, h1, h2, h3⟩
      rintro rfl; exact hg b hb h1.symm
    bParent := fun b hb => (ParentOk_congr rfl rfl b).mpr (hi.bParent b hb)
    nodupL := hi.nodupL
    lTgt := hi.lTgt
    nodupW := hi.nodupW
    workIff := hi.workIff
    dyingHeap := hi.dyingHeap
    resetDying := hi.resetDying
    nodupT := hi.nodupT
    tokOk := hi.tokOk
    resetTok := hi.resetTok
    rawTok := hi.rawTok
    tokAddr := hi.tokAddr
    nodupR := hi.nodupR
    regBlocks := hi.regBlocks
    regLive := hi.regLive
    bornLt := hi.bornLt
    finDoneLive := hi.finDoneLive
    finDoneEv := hi.finDoneEv
    evOk := hi.evOk
    nodupE := hi.nodupE }

/-- Two heaps that agree on every field except the header count. -/
structure Agree (h h' : Nat → Block) : Prop where
  st : ∀ x, (h' x).st = (h x).st
  gen : ∀ x, (h' x).gen = (h x).gen
  kind : ∀ x, (h' x).kind = (h x).kind
  arity : ∀ x, (h' x).arity = (h x).arity
  fin : ∀ x, (h' x).fin = (h x).fin
  finDone : ∀ x, (h' x).finDone = (h x).finDone
  reset : ∀ x, (h' x).reset = (h x).reset
  born : ∀ x, (h' x).born = (h x).born

theorem TokOk_agree {s s' : State} (ag : Agree s.heap s'.heap) (t : Token) :
    TokOk s' t ↔ TokOk s t := by
  unfold TokOk; split <;> simp [ag.st, ag.reset, ag.gen, ag.arity]

theorem agree_cnt (h : Nat → Block) (a : Nat) (k : Count) :
    Agree h (upd h a { h a with cnt := k }) := by
  constructor <;> intro x <;> by_cases hx : x = a <;> simp [hx]

theorem Inv.live_lt_next (hi : Inv cfg s) {a : Nat} (h : (s.heap a).st ≠ .free) : a < s.next := by
  by_cases ha : s.next ≤ a
  · rw [hi.frontier a ha] at h; exact absurd rfl h
  · omega

theorem Inv.release_dec (hi : Inv cfg s) {c : Credit} (hc : c ∈ s.credits)
    (hg : ∀ b ∈ s.borrows, b.guard ≠ c.h) {n : Nat} (hcn : (s.heap c.tgt).cnt = .fin n)
    (h2 : ¬ n ≤ 1) :
    Inv cfg { s with credits := s.credits.erase c,
                     heap := upd s.heap c.tgt { s.heap c.tgt with cnt := .fin (n - 1) } } := by
  have hn := hi.credits_nodup
  have hmem : ∀ d, d ∈ s.credits.erase c → d ∈ s.credits := fun d hd => List.mem_of_mem_erase hd
  have ag := agree_cnt s.heap c.tgt (.fin (n - 1))
  have ⟨hl, _, hk⟩ := hi.tgtOk c hc
  have hlt := hi.live_lt_next (s := s) (a := c.tgt) (by rw [hl]; decide)
  exact {
    frontier := by
      intro a ha
      have ha' : s.next ≤ a := ha
      have : a ≠ c.tgt := by omega
      simp [this, hi.frontier a ha']
    nodupH := nodup_map_erase _ c hi.nodupH
    tgtOk := by
      intro d hd; have := hi.tgtOk d (hmem d hd)
      simpa [ag.st, ag.gen, ag.kind] using this
    srcOk := by
      intro d hd src k hh; have := hi.srcOk d (hmem d hd) src k hh
      simpa [ag.st, ag.arity, ag.born] using this
    counts := by
      intro a hla hka
      have hla : (s.heap a).st = .live := (ag.st a).symm.trans hla
      have hka : (s.heap a).kind.counted = true := by rw [← ag.kind a]; exact hka
      have e := icount_erase hc a
      by_cases hca : c.tgt = a
      · subst hca
        rcases hi.counts c.tgt hla hka with h | ⟨m, hm, h1, h3, h4⟩
        · rw [hcn] at h; cases h
        · rw [hcn] at hm; cases hm
          right; refine ⟨n - 1, by simp, by omega, by omega, ?_⟩
          simp only [icount] at h4 ⊢; simp at e; omega
      · rcases hi.counts a hla hka with h | ⟨m, hm, h1, h3, h4⟩
        · left; simp [Ne.symm hca, h]
        · right; refine ⟨m, by simp [Ne.symm hca, hm], h1, h3, ?_⟩
          simp only [icount] at h4 ⊢; simp [hca] at e; omega
    staticImm := by
      intro a hla hka
      by_cases hca : a = c.tgt
      · subst hca; simp at hla hka
        have := hi.staticImm _ hla hka; rw [hcn] at this; cases this
      · simp [hca] at hla hka ⊢; exact hi.staticImm a hla hka
    finNotImm := by
      intro a hla hfa
      by_cases hca : a = c.tgt
      · subst hca; simp
      · simp [hca] at hla hfa ⊢; exact hi.finNotImm a hla hfa
    nodupB := hi.nodupB
    bGuard := by
      intro b hb
      obtain ⟨d, hd, h1, h2, h3⟩ := hi.bGuard b hb
      refine ⟨d, (mem_erase_iff' hn).mpr ⟨?_, hd⟩, h1, h2, h3⟩
      rintro rfl; exact hg b hb h1.symm
    bParent := fun b hb => (ParentOk_congr rfl rfl b).mpr (hi.bParent b hb)
    nodupL := hi.nodupL
    lTgt := by intro l hl; simpa [ag.st, ag.gen, ag.kind] using hi.lTgt l hl
    nodupW := hi.nodupW
    workIff := by intro a; simpa [ag.st] using hi.workIff a
    dyingHeap := by intro a; simpa [ag.st, ag.kind] using hi.dyingHeap a
    resetDying := by intro a; simpa [ag.st, ag.reset] using hi.resetDying a
    nodupT := hi.nodupT
    tokOk := fun t ht => (TokOk_agree (s' := { s with heap := _ }) ag t).mpr (hi.tokOk t ht)
    resetTok := by intro a t; simpa [ag.st, ag.reset] using hi.resetTok a t
    rawTok := by intro a; simpa [ag.st, ag.gen, ag.arity] using hi.rawTok a
    tokAddr := hi.tokAddr
    nodupR := hi.nodupR
    regBlocks := by intro R hR a ha; simpa [ag.st, ag.kind] using hi.regBlocks R hR a ha
    regLive := by intro a g; simpa [ag.st, ag.kind] using hi.regLive a g
    bornLt := by intro a; simpa [ag.st, ag.born] using hi.bornLt a
    finDoneLive := by intro a; simpa [ag.st, ag.finDone] using hi.finDoneLive a
    finDoneEv := by intro a; simpa [ag.st, ag.finDone, ag.gen] using hi.finDoneEv a
    evOk := by intro a g ok; simpa [ag.st, ag.finDone, ag.gen] using hi.evOk a g ok
    nodupE := hi.nodupE }

theorem Inv.release_die (hi : Inv cfg s) {c : Credit} (hc : c ∈ s.credits)
    (hg : ∀ b ∈ s.borrows, b.guard ≠ c.h) {n : Nat} (hcn : (s.heap c.tgt).cnt = .fin n)
    (h1 : n ≤ 1) :
    Inv cfg { s with credits := s.credits.erase c,
                     heap := upd s.heap c.tgt { s.heap c.tgt with st := .dying, cnt := .dead },
                     work := c.tgt :: s.work } := by
  have hn := hi.credits_nodup
  have hmem : ∀ d, d ∈ s.credits.erase c → d ∈ s.credits := fun d hd => List.mem_of_mem_erase hd
  have ⟨hl, hgen, hk⟩ := hi.tgtOk c hc
  have hlt := hi.live_lt_next (s := s) (a := c.tgt) (by rw [hl]; decide)
  have hn1 : n = 1 := by
    rcases hi.counts c.tgt hl hk with h | ⟨m, hm, h1', _, _⟩
    · rw [hcn] at h; cases h
    · rw [hcn] at hm; cases hm; omega
  subst hn1
  -- the released credit is the only one on its target
  have honly : ∀ d ∈ s.credits.erase c, d.tgt ≠ c.tgt := by
    intro d hd he
    have hd' := (mem_erase_iff' hn).mp hd
    exact hd'.1 (hi.only_credit hc hd'.2 hcn he)
  have hheap : (s.heap c.tgt).kind = .heap := by
    cases hkk : (s.heap c.tgt).kind with
    | heap => rfl
    | static => have := hi.staticImm _ hl hkk; rw [hcn] at this; cases this
    | region g => rw [hkk] at hk; cases hk
  have hreset : (s.heap c.tgt).reset = none := hi.resetDying _ (by rw [hl]; decide)
  have hfd : (s.heap c.tgt).finDone = false := hi.finDoneLive _ hl
  have hnw : c.tgt ∉ s.work := by
    intro h; have := (hi.workIff _).mp h; rw [hl] at this; cases this
  have hne_live : ∀ a, (s.heap a).st ≠ .live → a ≠ c.tgt := by
    intro a h he; subst he; exact h hl
  exact {
    frontier := by
      intro a ha
      have ha' : s.next ≤ a := ha
      have : a ≠ c.tgt := by omega
      simp [this, hi.frontier a ha']
    nodupH := nodup_map_erase _ c hi.nodupH
    tgtOk := by
      intro d hd
      simp [honly d hd]; exact hi.tgtOk d (hmem d hd)
    srcOk := by
      intro d hd src k hh
      have := hi.srcOk d (hmem d hd) src k hh
      simp only [upd_apply, honly d hd, ite_false]
      by_cases hs : src = c.tgt
      · subst hs; simp; exact this.2
      · simp [hs]; exact this
    counts := by
      intro a hla hka
      by_cases hca : a = c.tgt
      · subst hca; simp at hla
      · simp [hca] at hla hka ⊢
        have e := icount_erase hc a
        rcases hi.counts a hla hka with h | ⟨m, hm, h1, h3, h4⟩
        · left; exact h
        · right; refine ⟨m, hm, h1, h3, ?_⟩
          simp only [icount] at h4 ⊢; simp [Ne.symm hca] at e; omega
    staticImm := by
      intro a hla hka
      by_cases hca : a = c.tgt
      · subst hca; simp at hla
      · simp [hca] at hla hka ⊢; exact hi.staticImm a hla hka
    finNotImm := by
      intro a hla hfa
      by_cases hca : a = c.tgt
      · subst hca; simp at hla
      · simp [hca] at hla hfa ⊢; exact hi.finNotImm a hla hfa
    nodupB := hi.nodupB
    bGuard := by
      intro b hb
      obtain ⟨d, hd, h1, h2, h3⟩ := hi.bGuard b hb
      refine ⟨d, (mem_erase_iff' hn).mpr ⟨?_, hd⟩, h1, h2, h3⟩
      rintro rfl; exact hg b hb h1.symm
    bParent := fun b hb => (ParentOk_congr rfl rfl b).mpr (hi.bParent b hb)
    nodupL := hi.nodupL
    lTgt := by
      intro l hl'
      have := hi.lTgt l hl'
      have : l.tgt ≠ c.tgt := by intro he; rw [he] at this; rw [hk] at this; simp at this
      simp [this]; exact hi.lTgt l hl'
    nodupW := List.nodup_cons.mpr ⟨hnw, hi.nodupW⟩
    workIff := by
      intro a
      by_cases hca : a = c.tgt
      · subst hca; simp
      · simp [hca, hi.workIff a]
    dyingHeap := by
      intro a
      by_cases hca : a = c.tgt
      · subst hca; simp [hheap]
      · simp [hca]; exact hi.dyingHeap a
    resetDying := by
      intro a
      by_cases hca : a = c.tgt
      · subst hca; simp
      · simp [hca]; exact hi.resetDying a
    nodupT := hi.nodupT
    tokOk := by
      intro t ht
      have := hi.tokOk t ht
      unfold TokOk at this ⊢
      split
      · trivial
      · rename_i a _
        rw [‹t.val = _›] at this
        have ha : a ≠ c.tgt := hne_live a (by rw [this.1]; decide)
        simp [ha]; exact this
      · rename_i a _ _ _
        rw [‹t.val = _›] at this
        have ha : a ≠ c.tgt := hne_live a (by rw [this.1]; decide)
        simp [ha]; exact this
    resetTok := by
      intro a t hd hr
      by_cases hca : a = c.tgt
      · subst hca; simp [hreset] at hr
      · simp [hca] at hd hr; exact hi.resetTok a t hd hr
    rawTok := by
      intro a hr
      by_cases hca : a = c.tgt
      · subst hca; simp at hr
      · simp [hca] at hr ⊢; exact hi.rawTok a hr
    tokAddr := hi.tokAddr
    nodupR := hi.nodupR
    regBlocks := by
      intro R hR a ha
      have := hi.regBlocks R hR a ha
      have hca : a ≠ c.tgt := by intro he; rw [he] at this; rw [hheap] at this; simp at this
      simp [hca]; exact this
    regLive := by
      intro a g hka
      by_cases hca : a = c.tgt
      · subst hca; simp [hheap] at hka
      · simp [hca] at hka ⊢; exact hi.regLive a g hka
    bornLt := by
      intro a
      by_cases hca : a = c.tgt
      · subst hca; simp; exact hi.bornLt _ (by rw [hl]; decide)
      · simp [hca]; exact hi.bornLt a
    finDoneLive := by
      intro a
      by_cases hca : a = c.tgt
      · subst hca; simp
      · simp [hca]; exact hi.finDoneLive a
    finDoneEv := by
      intro a hd hf
      by_cases hca : a = c.tgt
      · subst hca; simp [hfd] at hf
      · simp only [upd_apply, hca, ite_false] at hd hf ⊢; exact hi.finDoneEv a hd hf
    evOk := by
      intro a g ok he
      have := hi.evOk a g ok he
      by_cases hca : a = c.tgt
      · subst hca
        simp
        refine ⟨this.1, fun hgg => ?_⟩
        exact absurd hl (this.2 hgg).1
      · simp [hca]; exact this
    nodupE := hi.nodupE }

/-- **Release preserves the invariant** whenever the released credit guards no live borrow. -/
theorem Inv.release (hi : Inv cfg s) {c : Credit} (hc : c ∈ s.credits)
    (hg : ∀ b ∈ s.borrows, b.guard ≠ c.h) : Inv cfg (Rc.release s c) := by
  unfold Rc.release
  split
  · rename_i n hcn
    split
    · exact hi.release_die hc hg hcn ‹_›
    · exact hi.release_dec hc hg hcn ‹_›
  · rename_i hne
    have ⟨hl, _, hk⟩ := hi.tgtOk c hc
    rcases hi.counts c.tgt hl hk with h | ⟨m, hm, _⟩
    · exact hi.release_keep hc hg h
    · exact absurd hm (hne m)

end

end Rc
