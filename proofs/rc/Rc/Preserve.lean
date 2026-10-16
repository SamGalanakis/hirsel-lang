import Rc.Release

/-! # Preservation lemmas for the building blocks of operations -/

namespace Rc

section
variable {cfg : Config} {s : State}

@[simp] theorem restamp_credits (s : State) (a : Nat) : (Rc.restamp s a).credits = s.credits := rfl
@[simp] theorem restamp_borrows (s : State) (a : Nat) : (Rc.restamp s a).borrows = s.borrows := rfl
@[simp] theorem restamp_locals (s : State) (a : Nat) : (Rc.restamp s a).locals = s.locals := rfl
@[simp] theorem restamp_tokens (s : State) (a : Nat) : (Rc.restamp s a).tokens = s.tokens := rfl
@[simp] theorem restamp_work (s : State) (a : Nat) : (Rc.restamp s a).work = s.work := rfl
@[simp] theorem restamp_regions (s : State) (a : Nat) : (Rc.restamp s a).regions = s.regions := rfl
@[simp] theorem restamp_events (s : State) (a : Nat) : (Rc.restamp s a).events = s.events := rfl
@[simp] theorem restamp_next (s : State) (a : Nat) : (Rc.restamp s a).next = s.next := rfl
@[simp] theorem relabel_heap (s : State) (c : Credit) (h : Holder) : (Rc.relabel s c h).heap = s.heap := rfl
@[simp] theorem relabel_borrows (s : State) (c : Credit) (h : Holder) : (Rc.relabel s c h).borrows = s.borrows := rfl
@[simp] theorem relabel_locals (s : State) (c : Credit) (h : Holder) : (Rc.relabel s c h).locals = s.locals := rfl
@[simp] theorem relabel_tokens (s : State) (c : Credit) (h : Holder) : (Rc.relabel s c h).tokens = s.tokens := rfl
@[simp] theorem relabel_work (s : State) (c : Credit) (h : Holder) : (Rc.relabel s c h).work = s.work := rfl
@[simp] theorem relabel_regions (s : State) (c : Credit) (h : Holder) : (Rc.relabel s c h).regions = s.regions := rfl
@[simp] theorem relabel_events (s : State) (c : Credit) (h : Holder) : (Rc.relabel s c h).events = s.events := rfl
@[simp] theorem relabel_next (s : State) (c : Credit) (h : Holder) : (Rc.relabel s c h).next = s.next := rfl
@[simp] theorem relabel_clock (s : State) (c : Credit) (h : Holder) : (Rc.relabel s c h).clock = s.clock := rfl
@[simp] theorem publish_credits (s : State) (a : Nat) : (Rc.publish s a).credits = s.credits := rfl
@[simp] theorem publish_borrows (s : State) (a : Nat) : (Rc.publish s a).borrows = s.borrows := rfl
@[simp] theorem publish_locals (s : State) (a : Nat) : (Rc.publish s a).locals = s.locals := rfl
@[simp] theorem publish_tokens (s : State) (a : Nat) : (Rc.publish s a).tokens = s.tokens := rfl
@[simp] theorem publish_work (s : State) (a : Nat) : (Rc.publish s a).work = s.work := rfl
@[simp] theorem publish_regions (s : State) (a : Nat) : (Rc.publish s a).regions = s.regions := rfl
@[simp] theorem publish_events (s : State) (a : Nat) : (Rc.publish s a).events = s.events := rfl
@[simp] theorem publish_next (s : State) (a : Nat) : (Rc.publish s a).next = s.next := rfl
@[simp] theorem publish_clock (s : State) (a : Nat) : (Rc.publish s a).clock = s.clock := rfl
@[simp] theorem addBorrow_heap (s : State) (b : Borrow) : (addBorrow s b).heap = s.heap := rfl
@[simp] theorem addBorrow_credits (s : State) (b : Borrow) : (addBorrow s b).credits = s.credits := rfl
@[simp] theorem addBorrow_borrows (s : State) (b : Borrow) : (addBorrow s b).borrows = b :: s.borrows := rfl
@[simp] theorem addBorrow_locals (s : State) (b : Borrow) : (addBorrow s b).locals = s.locals := rfl

/-- A heap change that every invariant clause ignores (e.g. publication) preserves `Inv`. -/
theorem Inv.of_agree (hi : Inv cfg s) {h : Nat → Block} (ag : Agree s.heap h)
    (hcnt : ∀ x, (h x).cnt = (s.heap x).cnt) (hfr : ∀ x, s.next ≤ x → h x = {}) :
    Inv cfg { s with heap := h } := by
  have hicount : ∀ a, icount { s with heap := h } a = icount s a := fun _ => rfl
  exact {
    frontier := hfr
    nodupH := hi.nodupH
    tgtOk := by intro d hd; simpa [ag.st, ag.gen, ag.kind] using hi.tgtOk d hd
    srcOk := by intro d hd src k hh; simpa [ag.st, ag.arity, ag.born] using hi.srcOk d hd src k hh
    counts := by intro a; simpa [ag.st, ag.kind, hcnt, hicount] using hi.counts a
    staticImm := by intro a; simpa [ag.st, ag.kind, hcnt] using hi.staticImm a
    finNotImm := by intro a; simpa [ag.st, ag.fin, hcnt] using hi.finNotImm a
    nodupB := hi.nodupB
    bGuard := hi.bGuard
    bParent := fun b hb => (ParentOk_congr rfl rfl b).mpr (hi.bParent b hb)
    nodupL := hi.nodupL
    lTgt := by intro l hl; simpa [ag.st, ag.gen, ag.kind] using hi.lTgt l hl
    nodupW := hi.nodupW
    workIff := by intro a; simpa [ag.st] using hi.workIff a
    dyingHeap := by intro a; simpa [ag.st, ag.kind] using hi.dyingHeap a
    resetDying := by intro a; simpa [ag.st, ag.reset] using hi.resetDying a
    nodupT := hi.nodupT
    tokOk := fun t ht => (TokOk_agree (s' := { s with heap := h }) ag t).mpr (hi.tokOk t ht)
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

theorem Inv.publish (hi : Inv cfg s) {a : Nat} (hl : (s.heap a).st = .live) :
    Inv cfg (Rc.publish s a) := by
  have hlt := hi.live_lt_next (s := s) (a := a) (by rw [hl]; decide)
  apply hi.of_agree
  · constructor <;> intro x <;> by_cases hx : x = a <;> simp [hx]
  · intro x; by_cases hx : x = a <;> simp [hx]
  · intro x hx
    have : x ≠ a := by omega
    simp [this, hi.frontier x hx]

/-- **Transfer**: moving a credit to a fresh holder keeps every ideal count. -/
theorem Inv.relabel (hi : Inv cfg s) {c : Credit} (hc : c ∈ s.credits)
    (hg : ∀ b ∈ s.borrows, b.guard ≠ c.h) {h : Holder} (hfresh : ∀ d ∈ s.credits, d.h ≠ h)
    (hsrc : ∀ src k, h = .field src k →
      ((s.heap src).st = .live ∨ (s.heap src).st = .dying) ∧ k < (s.heap src).arity ∧
        (s.heap c.tgt).born < (s.heap src).born) :
    Inv cfg (Rc.relabel s c h) := by
  have hn := hi.credits_nodup
  have hmem : ∀ d, d ∈ (Rc.relabel s c h).credits → d = { c with h := h } ∨ d ∈ s.credits := by
    intro d hd
    simp only [Rc.relabel, List.mem_cons] at hd
    rcases hd with hd | hd
    · exact Or.inl hd
    · exact Or.inr (List.mem_of_mem_erase hd)
  have hcount : ∀ a, icount (Rc.relabel s c h) a = icount s a := by
    intro a
    have e := icount_erase hc a
    simp only [icount, Rc.relabel, List.countP_cons] at e ⊢
    by_cases hca : c.tgt = a <;> simp [hca] at e ⊢ <;> omega
  exact {
    frontier := hi.frontier
    nodupH := by
      simp only [Rc.relabel, List.map_cons, List.nodup_cons]
      refine ⟨?_, nodup_map_erase _ c hi.nodupH⟩
      intro hm
      obtain ⟨d, hd, hdh⟩ := List.mem_map.mp hm
      exact hfresh d (List.mem_of_mem_erase hd) hdh
    tgtOk := by
      intro d hd
      rcases hmem d hd with rfl | hd
      · exact hi.tgtOk c hc
      · exact hi.tgtOk d hd
    srcOk := by
      intro d hd src k hh
      rcases hmem d hd with rfl | hd
      · exact hsrc src k hh
      · exact hi.srcOk d hd src k hh
    counts := by intro a; rw [hcount]; exact hi.counts a
    staticImm := hi.staticImm
    finNotImm := hi.finNotImm
    nodupB := hi.nodupB
    bGuard := by
      intro b hb
      obtain ⟨d, hd, h1, h2, h3⟩ := hi.bGuard b hb
      refine ⟨d, List.mem_cons_of_mem _ ((mem_erase_iff' hn).mpr ⟨?_, hd⟩), h1, h2, h3⟩
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

/-- Restamping a live block that no field credit points into keeps the DAG witness. -/
theorem Inv.restamp (hi : Inv cfg s) {a : Nat} (hl : (s.heap a).st = .live)
    (hno : ∀ d ∈ s.credits, d.tgt = a → ∀ src k, d.h ≠ .field src k) :
    Inv cfg (Rc.restamp s a) := by
  have hlt := hi.live_lt_next (s := s) (a := a) (by rw [hl]; decide)
  have hba := hi.bornLt a (by rw [hl]; decide)
  have hst : ∀ x, ((Rc.restamp s a).heap x).st = (s.heap x).st := by
    intro x; by_cases hx : x = a <;> simp [Rc.restamp, hx]
  have hgen : ∀ x, ((Rc.restamp s a).heap x).gen = (s.heap x).gen := by
    intro x; by_cases hx : x = a <;> simp [Rc.restamp, hx]
  have hkind : ∀ x, ((Rc.restamp s a).heap x).kind = (s.heap x).kind := by
    intro x; by_cases hx : x = a <;> simp [Rc.restamp, hx]
  have hari : ∀ x, ((Rc.restamp s a).heap x).arity = (s.heap x).arity := by
    intro x; by_cases hx : x = a <;> simp [Rc.restamp, hx]
  have hfin : ∀ x, ((Rc.restamp s a).heap x).fin = (s.heap x).fin := by
    intro x; by_cases hx : x = a <;> simp [Rc.restamp, hx]
  have hfd : ∀ x, ((Rc.restamp s a).heap x).finDone = (s.heap x).finDone := by
    intro x; by_cases hx : x = a <;> simp [Rc.restamp, hx]
  have hrs : ∀ x, ((Rc.restamp s a).heap x).reset = (s.heap x).reset := by
    intro x; by_cases hx : x = a <;> simp [Rc.restamp, hx]
  have hcnt : ∀ x, ((Rc.restamp s a).heap x).cnt = (s.heap x).cnt := by
    intro x; by_cases hx : x = a <;> simp [Rc.restamp, hx]
  have hborn : ∀ x, ((Rc.restamp s a).heap x).born = if x = a then s.clock else (s.heap x).born := by
    intro x; by_cases hx : x = a <;> simp [Rc.restamp, hx]
  have hicount : ∀ x, icount (Rc.restamp s a) x = icount s x := fun _ => rfl
  have hclock : (Rc.restamp s a).clock = s.clock + 1 := rfl
  exact {
    frontier := by
      intro x hx
      have hx' : s.next ≤ x := hx
      have : x ≠ a := by omega
      simp [Rc.restamp, this, hi.frontier x hx']
    nodupH := hi.nodupH
    tgtOk := by intro d hd; simpa [hst, hgen, hkind] using hi.tgtOk d hd
    srcOk := by
      intro d hd src k hh
      have h := hi.srcOk d hd src k hh
      simp only [hst, hari, hborn]
      refine ⟨h.1, h.2.1, ?_⟩
      have hdt : d.tgt ≠ a := fun he => hno d hd he src k hh
      by_cases hs : src = a
      · subst hs; simp [hdt]; exact Nat.lt_trans h.2.2 hba
      · simp [hs, hdt]; exact h.2.2
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
    resetDying := by intro x; simpa [hst, hrs] using hi.resetDying x
    nodupT := hi.nodupT
    tokOk := by
      intro t ht; have := hi.tokOk t ht
      unfold TokOk at this ⊢; split <;> simp_all
    resetTok := by intro x t; simpa [hst, hrs] using hi.resetTok x t
    rawTok := by intro x; simpa [hst, hgen, hari] using hi.rawTok x
    tokAddr := hi.tokAddr
    nodupR := hi.nodupR
    regBlocks := by intro R hR x hx; simpa [hst, hkind] using hi.regBlocks R hR x hx
    regLive := by intro x g; simpa [hst, hkind] using hi.regLive x g
    bornLt := by
      intro x hx
      rw [hst] at hx
      rw [hborn, hclock]
      by_cases hxa : x = a
      · simp [hxa]
      · simp [hxa]; exact Nat.lt_succ_of_lt (hi.bornLt x hx)
    finDoneLive := by intro x; simpa [hst, hfd] using hi.finDoneLive x
    finDoneEv := by intro x; simpa [hst, hfd, hgen] using hi.finDoneEv x
    evOk := by intro x g ok; simpa [hst, hfd, hgen] using hi.evOk x g ok
    nodupE := hi.nodupE }

theorem ParentOk_mono_borrows {s s' : State} (hb : ∀ b ∈ s.borrows, b ∈ s'.borrows)
    (hl : s'.locals = s.locals) (b : Borrow) (h : ParentOk s b) : ParentOk s' b := by
  unfold ParentOk at h ⊢
  split at h
  · exact h
  · obtain ⟨pb, hpb, h1, h2⟩ := h; exact ⟨pb, hb pb hpb, h1, h2⟩
  · rw [hl]; exact h

/-- **Share** (dup, owned part read): one more credit, header incremented or saturated. -/
theorem Inv.share (hi : Inv cfg s) {c : Credit} (hc : c ∈ s.credits) {r : Nat}
    (hfresh : ∀ d ∈ s.credits, d.h ≠ .root r) (hnf : (s.heap c.tgt).fin = false) :
    Inv cfg (Rc.share cfg s c.tgt c.gen r) := by
  have ⟨hl, hgen, hk⟩ := hi.tgtOk c hc
  have hlt := hi.live_lt_next (s := s) (a := c.tgt) (by rw [hl]; decide)
  have ag := agree_cnt s.heap c.tgt ((s.heap c.tgt).cnt.incr cfg)
  have hcount : ∀ a, icount (Rc.share cfg s c.tgt c.gen r) a =
      icount s a + if c.tgt = a then 1 else 0 := by
    intro a; simp [icount, Rc.share, List.countP_cons]
  have hmem : ∀ d ∈ (Rc.share cfg s c.tgt c.gen r).credits, d = ⟨.root r, c.tgt, c.gen⟩ ∨ d ∈ s.credits := by
    intro d hd; simpa [Rc.share] using hd
  exact {
    frontier := by
      intro a ha
      have ha' : s.next ≤ a := ha
      have : a ≠ c.tgt := by omega
      simp [Rc.share, this, hi.frontier a ha']
    nodupH := by
      simp only [Rc.share, List.map_cons, List.nodup_cons]
      refine ⟨?_, hi.nodupH⟩
      intro hm; obtain ⟨d, hd, hdh⟩ := List.mem_map.mp hm; exact hfresh d hd hdh
    tgtOk := by
      intro d hd
      rcases hmem d hd with rfl | hd
      · simpa [Rc.share, ag.st, ag.gen, ag.kind] using hi.tgtOk c hc
      · simpa [Rc.share, ag.st, ag.gen, ag.kind] using hi.tgtOk d hd
    srcOk := by
      intro d hd src k hh
      rcases hmem d hd with rfl | hd
      · cases hh
      · simpa [Rc.share, ag.st, ag.arity, ag.born] using hi.srcOk d hd src k hh
    counts := by
      intro a hla hka
      have hla : (s.heap a).st = .live := (ag.st a).symm.trans hla
      have hka : (s.heap a).kind.counted = true := by rw [← ag.kind a]; exact hka
      rw [hcount]
      by_cases hca : c.tgt = a
      · subst hca
        simp only [Rc.share, upd_same, ite_true]
        rcases hi.counts c.tgt hla hka with h | ⟨m, hm, h1, h3, h4⟩
        · left; rw [h]; rfl
        · rw [hm]; simp only [Count.incr]
          split
          · right; exact ⟨m + 1, rfl, by omega, by omega, by omega⟩
          · left; rfl
      · simp only [Rc.share, upd_ne (Ne.symm hca), hca, ite_false, Nat.add_zero]
        exact hi.counts a hla hka
    staticImm := by
      intro a hla hka
      by_cases hca : a = c.tgt
      · subst hca; simp [Rc.share] at hla hka ⊢
        rw [hi.staticImm _ hla hka]; rfl
      · simp [Rc.share, hca] at hla hka ⊢; exact hi.staticImm a hla hka
    finNotImm := by
      intro a hla hfa
      by_cases hca : a = c.tgt
      · subst hca; simp [Rc.share, hnf] at hfa
      · simp [Rc.share, hca] at hla hfa ⊢; exact hi.finNotImm a hla hfa
    nodupB := hi.nodupB
    bGuard := by
      intro b hb
      obtain ⟨d, hd, h1, h2, h3⟩ := hi.bGuard b hb
      exact ⟨d, List.mem_cons_of_mem _ hd, h1, h2, h3⟩
    bParent := fun b hb => (ParentOk_congr rfl rfl b).mpr (hi.bParent b hb)
    nodupL := hi.nodupL
    lTgt := by intro l hl; simpa [Rc.share, ag.st, ag.gen, ag.kind] using hi.lTgt l hl
    nodupW := hi.nodupW
    workIff := by intro a; simpa [Rc.share, ag.st] using hi.workIff a
    dyingHeap := by intro a; simpa [Rc.share, ag.st, ag.kind] using hi.dyingHeap a
    resetDying := by intro a; simpa [Rc.share, ag.st, ag.reset] using hi.resetDying a
    nodupT := hi.nodupT
    tokOk := fun t ht => (TokOk_agree (s' := Rc.share cfg s c.tgt c.gen r) ag t).mpr (hi.tokOk t ht)
    resetTok := by intro a t; simpa [Rc.share, ag.st, ag.reset] using hi.resetTok a t
    rawTok := by intro a; simpa [Rc.share, ag.st, ag.gen, ag.arity] using hi.rawTok a
    tokAddr := hi.tokAddr
    nodupR := hi.nodupR
    regBlocks := by intro R hR a ha; simpa [Rc.share, ag.st, ag.kind] using hi.regBlocks R hR a ha
    regLive := by intro a g; simpa [Rc.share, ag.st, ag.kind] using hi.regLive a g
    bornLt := by intro a; simpa [Rc.share, ag.st, ag.born] using hi.bornLt a
    finDoneLive := by intro a; simpa [Rc.share, ag.st, ag.finDone] using hi.finDoneLive a
    finDoneEv := by intro a; simpa [Rc.share, ag.st, ag.finDone, ag.gen] using hi.finDoneEv a
    evOk := by intro a g ok; simpa [Rc.share, ag.st, ag.finDone, ag.gen] using hi.evOk a g ok
    nodupE := hi.nodupE }

/-- Starting a borrow guarded by a held credit. -/
theorem Inv.addBorrow (hi : Inv cfg s) {b : Borrow} (hid : ∀ x ∈ s.borrows, x.id ≠ b.id)
    (hg : ∃ c ∈ s.credits, c.h = b.guard ∧ c.tgt = b.tgt ∧ c.gen = b.gen)
    (hp : ParentOk s b) : Inv cfg (Rc.addBorrow s b) := by
  have hmono : ∀ x, ParentOk s x → ParentOk (Rc.addBorrow s b) x :=
    ParentOk_mono_borrows (fun x hx => List.mem_cons_of_mem _ hx) rfl
  exact {
    frontier := hi.frontier
    nodupH := hi.nodupH
    tgtOk := hi.tgtOk
    srcOk := hi.srcOk
    counts := hi.counts
    staticImm := hi.staticImm
    finNotImm := hi.finNotImm
    nodupB := by
      simp only [addBorrow_borrows, List.map_cons, List.nodup_cons]
      refine ⟨?_, hi.nodupB⟩
      intro hm; obtain ⟨x, hx, hxid⟩ := List.mem_map.mp hm; exact hid x hx hxid
    bGuard := by
      intro x hx
      rcases List.mem_cons.mp hx with rfl | hx
      · exact hg
      · exact hi.bGuard x hx
    bParent := by
      intro x hx
      rcases List.mem_cons.mp hx with rfl | hx
      · exact hmono _ hp
      · exact hmono _ (hi.bParent x hx)
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

/-- Ending a borrow with no live sub-borrow. -/
theorem Inv.endBorrow (hi : Inv cfg s) {bb : Borrow} (hbb : bb ∈ s.borrows)
    (hchild : ∀ x ∈ s.borrows, x.parent ≠ .borrow bb.id) :
    Inv cfg { s with borrows := s.borrows.erase bb } := by
  have hbn : s.borrows.Nodup := nodup_of_map hi.nodupB
  exact {
    frontier := hi.frontier
    nodupH := hi.nodupH
    tgtOk := hi.tgtOk
    srcOk := hi.srcOk
    counts := hi.counts
    staticImm := hi.staticImm
    finNotImm := hi.finNotImm
    nodupB := hi.nodupB.sublist (List.erase_sublist.map _)
    bGuard := fun x hx => hi.bGuard x (List.mem_of_mem_erase hx)
    bParent := by
      intro x hx
      have hx' := List.mem_of_mem_erase hx
      have hp := hi.bParent x hx'
      unfold ParentOk at hp ⊢
      split at hp
      · exact hp
      · rename_i p h
        obtain ⟨pb, hpb, h1, h2⟩ := hp
        refine ⟨pb, (hbn.mem_erase_iff).mpr ⟨?_, hpb⟩, h1, h2⟩
        rintro rfl
        exact hchild x hx' (by rw [h, h1])
      · exact hp
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

end

end Rc
