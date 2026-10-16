import Rc.Preserve

/-! # Allocation, tokens and locals preserve the invariant -/

namespace Rc

section
variable {cfg : Config} {s : State}

theorem freeAddr_spec (hi : Inv cfg s) :
    (s.heap (freeAddr s)).st = .free ∧ freeAddr s ≤ s.next := by
  unfold freeAddr
  cases h : (List.range s.next).find? (fun a => (s.heap a).st == .free) with
  | none => simp [hi.frontier s.next (Nat.le_refl _)]
  | some a =>
    have hp := find?_prop h
    have hm := find?_mem h
    simp at hp hm
    exact ⟨hp, Nat.le_of_lt hm⟩

/-- A free slot is referenced by nothing. -/
theorem Inv.free_unref (hi : Inv cfg s) {a : Nat} (ha : (s.heap a).st = .free) :
    (∀ c ∈ s.credits, c.tgt ≠ a) ∧ (∀ c ∈ s.credits, ∀ k, c.h ≠ .field a k) ∧
      (∀ l ∈ s.locals, l.tgt ≠ a) ∧ a ∉ s.work ∧
      (∀ t ∈ s.tokens, t.val.addr ≠ some a) ∧ (∀ R ∈ s.regions, a ∉ R.blocks) ∧
      (s.heap a).kind.counted = true ∧ (s.heap a).reset = none := by
  refine ⟨?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_⟩
  · intro c hc he; have := (hi.tgtOk c hc).1; rw [he, ha] at this; cases this
  · intro c hc k he
    have := (hi.srcOk c hc a k he).1; rw [ha] at this; rcases this with h | h <;> cases h
  · intro l hl he; have := (hi.lTgt l hl).1; rw [he, ha] at this; cases this
  · intro hw; have := (hi.workIff a).mp hw; rw [ha] at this; cases this
  · intro t ht he
    have := hi.tokOk t ht
    unfold TokOk at this
    split at this
    · rename_i hv; rw [hv] at he; cases he
    · rename_i a' hv; rw [hv] at he; cases he; rw [ha] at this; cases this.1
    · rename_i a' _ _ hv; rw [hv] at he; cases he; rw [ha] at this; cases this.1
  · intro R hR hm; have := (hi.regBlocks R hR a hm).1; rw [ha] at this; cases this
  · cases hk : (s.heap a).kind with
    | heap => rfl
    | static => rfl
    | region g => have := (hi.regLive a g hk).1; rw [ha] at this; cases this
  · exact hi.resetDying a (by rw [ha]; decide)

/-- Conditions on a freshly installed counted block. -/
structure FreshOk (cfg : Config) (b : Block) : Prop where
  st : b.st = .live
  counted : b.kind.counted = true
  cnt : b.cnt = .fin 1 ∨ b.cnt = .immortal
  static : b.kind = .static → b.cnt = .immortal
  fin : b.fin = true → b.cnt ≠ .immortal
  finDone : b.finDone = false
  reset : b.reset = none

theorem heapBlock_ok (arity : Nat) (fin : Bool) : FreshOk cfg (heapBlock arity fin) :=
  ⟨rfl, rfl, Or.inl rfl, (fun h => by cases h), (fun _ h => by cases h), rfl, rfl⟩

theorem staticBlock_ok : FreshOk cfg staticBlock :=
  ⟨rfl, rfl, Or.inr rfl, (fun _ => rfl), (fun h => by cases h), rfl, rfl⟩

theorem Inv.installCounted (hok : cfg.Ok) (hi : Inv cfg s) {a r : Nat} {b : Block}
    (ha : (s.heap a).st = .free) (hle : a ≤ s.next) (hr : ∀ c ∈ s.credits, c.h ≠ .root r)
    (hb : FreshOk cfg b) : Inv cfg (Rc.installCounted s a r b) := by
  obtain ⟨hnt, hns, hnl, hnw, hntok, hnreg, _, _⟩ := hi.free_unref ha
  have hic0 : icount s a = 0 := by
    simp only [icount]; rw [List.countP_eq_zero]; intro c hc; simpa using hnt c hc
  have hcount : ∀ x, icount (Rc.installCounted s a r b) x = icount s x + if a = x then 1 else 0 := by
    intro x; simp [icount, Rc.installCounted, List.countP_cons]
  have hheap : ∀ x, x ≠ a → (Rc.installCounted s a r b).heap x = s.heap x := by
    intro x hx; simp [Rc.installCounted, hx]
  have hnew : (Rc.installCounted s a r b).heap a = { b with gen := (s.heap a).gen + 1, born := s.clock } := by
    simp [Rc.installCounted]
  have hcl : (Rc.installCounted s a r b).clock = s.clock + 1 := rfl
  have hmem : ∀ d ∈ (Rc.installCounted s a r b).credits,
      d = ⟨.root r, a, (s.heap a).gen + 1⟩ ∨ d ∈ s.credits := by
    intro d hd; simpa [Rc.installCounted] using hd
  have hokmax : 1 < cfg.max := hok
  exact {
    frontier := by
      intro x hx
      have hx' : max s.next (a + 1) ≤ x := hx
      rw [hheap x (by omega)]; exact hi.frontier x (by omega)
    nodupH := by
      simp only [Rc.installCounted, List.map_cons, List.nodup_cons]
      refine ⟨?_, hi.nodupH⟩
      intro hm; obtain ⟨d, hd, hdh⟩ := List.mem_map.mp hm; exact hr d hd hdh
    tgtOk := by
      intro d hd
      rcases hmem d hd with rfl | hd
      · rw [hnew]; exact ⟨hb.st, rfl, hb.counted⟩
      · rw [hheap _ (hnt d hd)]; exact hi.tgtOk d hd
    srcOk := by
      intro d hd src k hh
      rcases hmem d hd with rfl | hd
      · cases hh
      · have hsrc : src ≠ a := fun he => hns d hd k (he ▸ hh)
        rw [hheap _ (hnt d hd), hheap _ hsrc]; exact hi.srcOk d hd src k hh
    counts := by
      intro x hl hk
      rw [hcount]
      by_cases hx : x = a
      · subst hx
        rw [hnew] at hl hk ⊢
        rcases hb.cnt with h | h
        · right; refine ⟨1, h, Nat.le_refl _, hokmax, ?_⟩; simp [hic0]
        · left; exact h
      · rw [hheap x hx] at hl hk ⊢
        simp only [Ne.symm hx, ite_false, Nat.add_zero]
        exact hi.counts x hl hk
    staticImm := by
      intro x hl hk
      by_cases hx : x = a
      · subst hx; rw [hnew] at hk ⊢; exact hb.static hk
      · rw [hheap x hx] at hl hk ⊢; exact hi.staticImm x hl hk
    finNotImm := by
      intro x hl hf
      by_cases hx : x = a
      · subst hx; rw [hnew] at hf ⊢; exact hb.fin hf
      · rw [hheap x hx] at hl hf ⊢; exact hi.finNotImm x hl hf
    nodupB := hi.nodupB
    bGuard := by
      intro x hx
      obtain ⟨d, hd, h1, h2, h3⟩ := hi.bGuard x hx
      exact ⟨d, List.mem_cons_of_mem _ hd, h1, h2, h3⟩
    bParent := fun x hx => (ParentOk_congr rfl rfl x).mpr (hi.bParent x hx)
    nodupL := hi.nodupL
    lTgt := by intro l hl; rw [hheap _ (hnl l hl)]; exact hi.lTgt l hl
    nodupW := hi.nodupW
    workIff := by
      intro x
      by_cases hx : x = a
      · subst hx; rw [hnew]; simp [hb.st]; exact hnw
      · rw [hheap x hx]; exact hi.workIff x
    dyingHeap := by
      intro x
      by_cases hx : x = a
      · subst hx; rw [hnew]; simp [hb.st]
      · rw [hheap x hx]; exact hi.dyingHeap x
    resetDying := by
      intro x
      by_cases hx : x = a
      · subst hx; rw [hnew]; simp [hb.reset]
      · rw [hheap x hx]; exact hi.resetDying x
    nodupT := hi.nodupT
    tokOk := by
      intro t ht
      refine (TokOk_congr t ?_).mpr (hi.tokOk t ht)
      intro x hx; apply hheap; intro he; subst he; exact hntok t ht hx
    resetTok := by
      intro x t hd hrs
      by_cases hx : x = a
      · subst hx; rw [hnew] at hd; simp [hb.st] at hd
      · rw [hheap x hx] at hd hrs; exact hi.resetTok x t hd hrs
    rawTok := by
      intro x hrw
      by_cases hx : x = a
      · subst hx; rw [hnew] at hrw; simp [hb.st] at hrw
      · rw [hheap x hx] at hrw ⊢; exact hi.rawTok x hrw
    tokAddr := hi.tokAddr
    nodupR := hi.nodupR
    regBlocks := by
      intro R hR x hx
      have : x ≠ a := by intro he; subst he; exact hnreg R hR hx
      rw [hheap x this]; exact hi.regBlocks R hR x hx
    regLive := by
      intro x g hk
      by_cases hx : x = a
      · subst hx; rw [hnew] at hk; have := hb.counted; rw [hk] at this; cases this
      · rw [hheap x hx] at hk ⊢; exact hi.regLive x g hk
    bornLt := by
      intro x hx
      rw [hcl]
      by_cases hxa : x = a
      · subst hxa; rw [hnew]; simp
      · rw [hheap x hxa] at hx ⊢; exact Nat.lt_succ_of_lt (hi.bornLt x hx)
    finDoneLive := by
      intro x
      by_cases hx : x = a
      · subst hx; rw [hnew]; simp [hb.finDone]
      · rw [hheap x hx]; exact hi.finDoneLive x
    finDoneEv := by
      intro x hd
      by_cases hx : x = a
      · subst hx; rw [hnew] at hd; simp [hb.st] at hd
      · rw [hheap x hx] at hd ⊢; exact hi.finDoneEv x hd
    evOk := by
      intro x g ok he
      have := hi.evOk x g ok he
      by_cases hx : x = a
      · subst hx; rw [hnew]; simp; omega
      · rw [hheap x hx]; exact this
    nodupE := hi.nodupE }

theorem ParentOk_mono_locals {s s' : State} (hb : s'.borrows = s.borrows)
    (hl : ∀ l ∈ s.locals, l ∈ s'.locals) (b : Borrow) (h : ParentOk s b) : ParentOk s' b := by
  unfold ParentOk at h ⊢
  split at h
  · exact h
  · rw [hb]; exact h
  · obtain ⟨lc, hlc, h1, h2⟩ := h; exact ⟨lc, hl lc hlc, h1, h2⟩

theorem Inv.installRegion (hi : Inv cfg s) {a g l arity : Nat}
    (ha : (s.heap a).st = .free) (hle : a ≤ s.next) (hl : ∀ x ∈ s.locals, x.id ≠ l)
    (hg : ∃ R ∈ s.regions, R.id = g) : Inv cfg (Rc.installRegion s a g l arity) := by
  obtain ⟨hnt, hns, hnl, hnw, hntok, hnreg, _, _⟩ := hi.free_unref ha
  have hheap : ∀ x, x ≠ a → (Rc.installRegion s a g l arity).heap x = s.heap x := by
    intro x hx; simp [Rc.installRegion, hx]
  have hnew : (Rc.installRegion s a g l arity).heap a =
      { st := .live, gen := (s.heap a).gen + 1, kind := .region g, arity, born := s.clock } := by
    simp [Rc.installRegion]
  have hcl : (Rc.installRegion s a g l arity).clock = s.clock + 1 := rfl
  have hregs : (Rc.installRegion s a g l arity).regions =
      s.regions.map (fun R => if R.id = g then { R with blocks := a :: R.blocks } else R) := rfl
  have hids : (Rc.installRegion s a g l arity).regions.map Region.id = s.regions.map Region.id := by
    rw [hregs, List.map_map]; congr 1; funext R; by_cases h : R.id = g <;> simp [h]
  exact {
    frontier := by
      intro x hx
      have hx' : max s.next (a + 1) ≤ x := hx
      rw [hheap x (by omega)]; exact hi.frontier x (by omega)
    nodupH := hi.nodupH
    tgtOk := by intro d hd; rw [hheap _ (hnt d hd)]; exact hi.tgtOk d hd
    srcOk := by
      intro d hd src k hh
      have hsrc : src ≠ a := fun he => hns d hd k (he ▸ hh)
      rw [hheap _ (hnt d hd), hheap _ hsrc]; exact hi.srcOk d hd src k hh
    counts := by
      intro x hlx hk
      by_cases hx : x = a
      · subst hx; rw [hnew] at hk; cases hk
      · rw [hheap x hx] at hlx hk ⊢; exact hi.counts x hlx hk
    staticImm := by
      intro x hlx hk
      by_cases hx : x = a
      · subst hx; rw [hnew] at hk; cases hk
      · rw [hheap x hx] at hlx hk ⊢; exact hi.staticImm x hlx hk
    finNotImm := by
      intro x hlx hf
      by_cases hx : x = a
      · subst hx; rw [hnew] at hf; cases hf
      · rw [hheap x hx] at hlx hf ⊢; exact hi.finNotImm x hlx hf
    nodupB := hi.nodupB
    bGuard := hi.bGuard
    bParent := fun x hx => ParentOk_mono_locals (s := s) (s' := Rc.installRegion s a g l arity) rfl
      (fun l hl => List.mem_cons_of_mem _ hl) x (hi.bParent x hx)
    nodupL := by
      simp only [Rc.installRegion, List.map_cons, List.nodup_cons]
      refine ⟨?_, hi.nodupL⟩
      intro hm; obtain ⟨x, hx, hxid⟩ := List.mem_map.mp hm; exact hl x hx hxid
    lTgt := by
      intro x hx
      simp only [Rc.installRegion, List.mem_cons] at hx
      rcases hx with rfl | hx
      · rw [hnew]; exact ⟨rfl, rfl, rfl⟩
      · rw [hheap _ (hnl x hx)]; exact hi.lTgt x hx
    nodupW := hi.nodupW
    workIff := by
      intro x
      by_cases hx : x = a
      · subst hx; rw [hnew]; simp; exact hnw
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
    nodupT := hi.nodupT
    tokOk := by
      intro t ht
      refine (TokOk_congr t ?_).mpr (hi.tokOk t ht)
      intro x hx; apply hheap; intro he; subst he; exact hntok t ht hx
    resetTok := by
      intro x t hd hrs
      by_cases hx : x = a
      · subst hx; rw [hnew] at hd; simp at hd
      · rw [hheap x hx] at hd hrs; exact hi.resetTok x t hd hrs
    rawTok := by
      intro x hrw
      by_cases hx : x = a
      · subst hx; rw [hnew] at hrw; simp at hrw
      · rw [hheap x hx] at hrw ⊢; exact hi.rawTok x hrw
    tokAddr := hi.tokAddr
    nodupR := by rw [hids]; exact hi.nodupR
    regBlocks := by
      intro R' hR' x hx
      rw [hregs] at hR'
      obtain ⟨R, hR, rfl⟩ := List.mem_map.mp hR'
      by_cases hRg : R.id = g
      · have hx' : x = a ∨ x ∈ R.blocks := by simpa [hRg] using hx
        rcases hx' with rfl | hx'
        · rw [hnew]; simp [hRg]
        · have hxa : x ≠ a := fun he => hnreg R hR (he ▸ hx')
          have := hi.regBlocks R hR x hx'
          rw [hheap x hxa]; simpa [hRg] using this
      · have hx' : x ∈ R.blocks := by simpa [hRg] using hx
        have hxa : x ≠ a := fun he => hnreg R hR (he ▸ hx')
        have := hi.regBlocks R hR x hx'
        rw [hheap x hxa]; simpa [hRg] using this
    regLive := by
      intro x g' hk
      rw [hregs]
      by_cases hx : x = a
      · subst hx
        rw [hnew] at hk ⊢; cases hk
        obtain ⟨R, hR, hRg⟩ := hg
        refine ⟨rfl, { R with blocks := x :: R.blocks }, ?_, hRg, by simp⟩
        exact List.mem_map.mpr ⟨R, hR, by simp [hRg]⟩
      · rw [hheap x hx] at hk ⊢
        obtain ⟨hlv, R, hR, hRid, hxR⟩ := hi.regLive x g' hk
        refine ⟨hlv, if R.id = g then { R with blocks := a :: R.blocks } else R,
          List.mem_map.mpr ⟨R, hR, rfl⟩, ?_, ?_⟩
        · split <;> exact hRid
        · split <;> simp [hxR]
    bornLt := by
      intro x hx
      rw [hcl]
      by_cases hxa : x = a
      · subst hxa; rw [hnew]; simp
      · rw [hheap x hxa] at hx ⊢; exact Nat.lt_succ_of_lt (hi.bornLt x hx)
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
      · subst hx; rw [hnew]; simp; omega
      · rw [hheap x hx]; exact this
    nodupE := hi.nodupE }

end

end Rc
