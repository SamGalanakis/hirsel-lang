import Rc.Worklist

/-! # Regions and locals preserve the invariant -/

namespace Rc

theorem mem_foldl_erase {α : Type} [DecidableEq α] :
    ∀ (l cs : List α), cs.Nodup → ∀ d, d ∈ l.foldl List.erase cs ↔ d ∈ cs ∧ d ∉ l
  | [], cs, _, d => by simp
  | c :: l, cs, hn, d => by
    simp only [List.foldl_cons]
    rw [mem_foldl_erase l (cs.erase c) (hn.erase c) d, hn.mem_erase_iff]
    simp only [List.mem_cons, not_or]
    constructor
    · rintro ⟨⟨h1, h2⟩, h3⟩; exact ⟨h2, h1, h3⟩
    · rintro ⟨h2, h1, h3⟩; exact ⟨⟨h1, h2⟩, h3⟩

theorem foldl_release_credits :
    ∀ (l : List Credit) (s : State), (l.foldl release s).credits = l.foldl List.erase s.credits
  | [], _ => rfl
  | c :: l, s => by simp only [List.foldl_cons]; rw [foldl_release_credits l]; simp

theorem foldl_release_frame :
    ∀ (l : List Credit) (s : State),
      (l.foldl release s).borrows = s.borrows ∧ (l.foldl release s).locals = s.locals ∧
      (l.foldl release s).tokens = s.tokens ∧ (l.foldl release s).regions = s.regions ∧
      (l.foldl release s).events = s.events ∧ (l.foldl release s).next = s.next ∧
      (l.foldl release s).clock = s.clock
  | [], _ => by simp
  | c :: l, s => by
    simp only [List.foldl_cons]
    obtain ⟨h1, h2, h3, h4, h5, h6, h7⟩ := foldl_release_frame l (release s c)
    simp [h1, h2, h3, h4, h5, h6, h7]

theorem foldl_release_heap :
    ∀ (l : List Credit) (s : State) (x : Nat), (∀ c ∈ l, c.tgt ≠ x) →
      (l.foldl release s).heap x = s.heap x
  | [], _, _, _ => rfl
  | c :: l, s, x, h => by
    simp only [List.foldl_cons]
    rw [foldl_release_heap l (release s c) x (fun d hd => h d (List.mem_cons_of_mem _ hd))]
    exact release_heap_ne s c (Ne.symm (h c List.mem_cons_self))

section
variable {cfg : Config}

theorem Inv.releaseAll :
    ∀ (l : List Credit) {s : State}, Inv cfg s → l.Nodup → (∀ c ∈ l, c ∈ s.credits) →
      (∀ c ∈ l, ∀ b ∈ s.borrows, b.guard ≠ c.h) → Inv cfg (l.foldl Rc.release s)
  | [], _, hi, _, _, _ => hi
  | c :: l, s, hi, hn, hm, hg => by
    simp only [List.foldl_cons]
    rw [List.nodup_cons] at hn
    apply Inv.releaseAll l (hi.release (hm c List.mem_cons_self) (hg c List.mem_cons_self)) hn.2
    · intro d hd
      rw [release_credits]
      exact mem_erase_of_mem_ne (hm d (List.mem_cons_of_mem _ hd)) (fun he => hn.1 (he ▸ hd))
    · intro d hd b hb
      rw [release_borrows] at hb
      exact hg d (List.mem_cons_of_mem _ hd) b hb

variable {s : State}

theorem Inv.regionOpen (hi : Inv cfg s) {g : Nat} (hg : ∀ R ∈ s.regions, R.id ≠ g) :
    Inv cfg { s with regions := ⟨g, []⟩ :: s.regions } := by
  exact {
    hi with
    nodupR := by
      simp only [List.map_cons, List.nodup_cons]
      refine ⟨?_, hi.nodupR⟩
      intro hm; obtain ⟨R, hR, hid⟩ := List.mem_map.mp hm; exact hg R hR hid
    regBlocks := by
      intro R hR x hx
      rcases List.mem_cons.mp hR with rfl | hR
      · simp at hx
      · exact hi.regBlocks R hR x hx
    regLive := by
      intro x g' hk
      obtain ⟨hl, R, hR, h1, h2⟩ := hi.regLive x g' hk
      exact ⟨hl, R, List.mem_cons_of_mem _ hR, h1, h2⟩ }

theorem Inv.addLocal (hi : Inv cfg s) {l : Local} (hid : ∀ x ∈ s.locals, x.id ≠ l.id)
    (hl : (s.heap l.tgt).st = .live ∧ (s.heap l.tgt).gen = l.gen ∧
      (s.heap l.tgt).kind.counted = false) :
    Inv cfg { s with locals := l :: s.locals } := by
  exact {
    hi with
    bParent := fun b hb => ParentOk_mono_locals (s := s) (s' := { s with locals := l :: s.locals })
      rfl (fun x hx => List.mem_cons_of_mem _ hx) b (hi.bParent b hb)
    nodupL := by
      simp only [List.map_cons, List.nodup_cons]
      refine ⟨?_, hi.nodupL⟩
      intro hm; obtain ⟨x, hx, hxid⟩ := List.mem_map.mp hm; exact hid x hx hxid
    lTgt := by
      intro x hx
      rcases List.mem_cons.mp hx with rfl | hx
      · exact hl
      · exact hi.lTgt x hx }

theorem Inv.localDrop (hi : Inv cfg s) {lc : Local} (hlc : lc ∈ s.locals)
    (hchild : ∀ b ∈ s.borrows, b.parent ≠ .loc lc.id) :
    Inv cfg { s with locals := s.locals.erase lc } := by
  have hln : s.locals.Nodup := nodup_of_map hi.nodupL
  exact {
    hi with
    bParent := by
      intro b hb
      have hp := hi.bParent b hb
      unfold ParentOk at hp ⊢
      split at hp
      · exact hp
      · exact hp
      · rename_i l h
        obtain ⟨x, hx, h1, h2⟩ := hp
        refine ⟨x, (hln.mem_erase_iff).mpr ⟨?_, hx⟩, h1, h2⟩
        rintro rfl
        exact hchild b hb (by rw [h, h1])
    nodupL := hi.nodupL.sublist (List.erase_sublist.map _)
    lTgt := fun x hx => hi.lTgt x (List.mem_of_mem_erase hx) }

theorem Inv.localCopy (hi : Inv cfg s) {lc : Local} (hlc : lc ∈ s.locals) {l' : Nat}
    (hid : ∀ x ∈ s.locals, x.id ≠ l') :
    Inv cfg { Rc.publish s lc.tgt with locals := ⟨l', lc.tgt, lc.gen⟩ :: s.locals } := by
  have hl := hi.lTgt lc hlc
  have hp := hi.publish hl.1
  have := hp.addLocal (l := ⟨l', lc.tgt, lc.gen⟩) (by simpa using hid) (by
    have hb : (Rc.publish s lc.tgt).heap lc.tgt = { s.heap lc.tgt with published := true } := by
      simp [Rc.publish]
    rw [hb]; exact hl)
  simpa using this

/-- The blocks a region closes, and their outgoing heap credits. -/
theorem mem_outgoing {cs : List Credit} {blocks : List Nat} {c : Credit} :
    c ∈ outgoing cs blocks ↔ c ∈ cs ∧ ∃ src k, c.h = .field src k ∧ src ∈ blocks := by
  simp only [outgoing, List.mem_filter]
  constructor
  · rintro ⟨h1, h2⟩
    refine ⟨h1, ?_⟩
    split at h2
    · rename_i src k hh; exact ⟨src, k, hh, by simpa using h2⟩
    · cases h2
  · rintro ⟨h1, src, k, hh, hs⟩
    refine ⟨h1, ?_⟩
    rw [hh]; simpa using hs

/-- **Region close** preserves the invariant: outgoing credits are released, then the
region's blocks become free storage. -/
theorem Inv.regionClose (hi : Inv cfg s) {R : Region} {rest : List Region}
    (hrs : s.regions = R :: rest) (hloc : ∀ l ∈ s.locals, l.tgt ∉ R.blocks) :
    Inv cfg (freeBlocks ((outgoing s.credits R.blocks).foldl Rc.release s) R.blocks rest) := by
  generalize hs1 : (outgoing s.credits R.blocks).foldl Rc.release s = s1
  have hR : R ∈ s.regions := by rw [hrs]; exact List.mem_cons_self
  have hblk := hi.regBlocks R hR
  have hcn := hi.credits_nodup
  -- the outgoing credits are held, distinct and unguarded
  have hout_mem : ∀ c ∈ outgoing s.credits R.blocks, c ∈ s.credits :=
    fun c hc => (mem_outgoing.mp hc).1
  have hout_nodup : (outgoing s.credits R.blocks).Nodup := hcn.filter _
  have hout_guard : ∀ c ∈ outgoing s.credits R.blocks, ∀ b ∈ s.borrows, b.guard ≠ c.h := by
    intro c hc b hb
    obtain ⟨_, src, k, hh, hs⟩ := mem_outgoing.mp hc
    rw [hh]
    have hk := (hblk src hs).2
    refine hi.no_guard_field (Or.inr (by rw [hk]; rfl)) ?_ b hb
    intro l hl he; exact hloc l hl (he ▸ hs)
  have hi1 : Inv cfg s1 := hs1 ▸ hi.releaseAll _ hout_nodup hout_mem hout_guard
  obtain ⟨hb1, hl1, ht1, hr1, he1, hn1, hc1⟩ := foldl_release_frame (outgoing s.credits R.blocks) s
  rw [hs1] at hb1 hl1 ht1 hr1 he1 hn1 hc1
  -- region blocks are untouched by the releases (they are never credit targets)
  have hregion_heap : ∀ x ∈ R.blocks, s1.heap x = s.heap x := by
    intro x hx
    rw [← hs1]
    apply foldl_release_heap
    intro c hc he
    have := (hi.tgtOk c (hout_mem c hc)).2.2
    rw [he, (hblk x hx).2] at this; cases this
  have hcred1 : ∀ d, d ∈ s1.credits ↔ d ∈ s.credits ∧ d ∉ outgoing s.credits R.blocks := by
    intro d
    rw [← hs1, foldl_release_credits]; exact mem_foldl_erase _ _ hcn d
  have hblk1 : ∀ x ∈ R.blocks, (s1.heap x).st = .live ∧ (s1.heap x).kind = .region R.id := by
    intro x hx; rw [hregion_heap x hx]; exact hblk x hx
  -- after the releases nothing refers to the region's blocks
  have hnt : ∀ c ∈ s1.credits, c.tgt ∉ R.blocks := by
    intro c hc hm
    have := (hi1.tgtOk c hc).2.2
    rw [(hblk1 _ hm).2] at this; cases this
  have hns : ∀ c ∈ s1.credits, ∀ src k, c.h = .field src k → src ∉ R.blocks := by
    intro c hc src k hh hs
    have := (hcred1 c).mp hc
    exact this.2 (mem_outgoing.mpr ⟨this.1, src, k, hh, hs⟩)
  have hntok : ∀ u ∈ s1.tokens, ∀ x ∈ R.blocks, u.val.addr ≠ some x := by
    intro u hu x hx he
    have h0 := hi1.tokOk u hu
    have hlx := (hblk1 x hx).1
    unfold TokOk at h0
    split at h0
    · rename_i hv; rw [hv] at he; cases he
    · rename_i a hv; rw [hv] at he; cases he; rw [hlx] at h0; cases h0.1
    · rename_i a _ _ hv; rw [hv] at he; cases he; rw [hlx] at h0; cases h0.1
  have hheap : ∀ x, x ∉ R.blocks → (freeBlocks s1 R.blocks rest).heap x = s1.heap x := by
    intro x hx; simp [freeBlocks, hx]
  have hnew : ∀ x, x ∈ R.blocks → (freeBlocks s1 R.blocks rest).heap x = { gen := (s1.heap x).gen } := by
    intro x hx; simp [freeBlocks, hx]
  have hnodupR := hi.nodupR
  rw [hrs, List.map_cons, List.nodup_cons] at hnodupR
  exact {
    frontier := by
      intro x hx
      have hx' : s1.next ≤ x := hx
      have hxn : x ∉ R.blocks := by
        intro hm
        have := hi1.live_lt_next (a := x) (by rw [(hblk1 x hm).1]; decide)
        omega
      rw [hheap x hxn]; exact hi1.frontier x hx'
    nodupH := hi1.nodupH
    tgtOk := by intro d hd; rw [hheap _ (hnt d hd)]; exact hi1.tgtOk d hd
    srcOk := by
      intro d hd src k hh
      rw [hheap _ (hns d hd src k hh), hheap _ (hnt d hd)]
      exact hi1.srcOk d hd src k hh
    counts := by
      intro x hlx hk
      by_cases hx : x ∈ R.blocks
      · rw [hnew x hx] at hlx; cases hlx
      · rw [hheap x hx] at hlx hk ⊢; exact hi1.counts x hlx hk
    staticImm := by
      intro x hlx hk
      by_cases hx : x ∈ R.blocks
      · rw [hnew x hx] at hlx; cases hlx
      · rw [hheap x hx] at hlx hk ⊢; exact hi1.staticImm x hlx hk
    finNotImm := by
      intro x hlx hf
      by_cases hx : x ∈ R.blocks
      · rw [hnew x hx] at hlx; cases hlx
      · rw [hheap x hx] at hlx hf ⊢; exact hi1.finNotImm x hlx hf
    nodupB := hi1.nodupB
    bGuard := hi1.bGuard
    bParent := fun b hb => (ParentOk_congr rfl rfl b).mpr (hi1.bParent b hb)
    nodupL := hi1.nodupL
    lTgt := by
      intro l hl
      have hl' : l ∈ s.locals := by rw [← hl1]; exact hl
      rw [hheap _ (hloc l hl')]; exact hi1.lTgt l hl
    nodupW := hi1.nodupW
    workIff := by
      intro x
      by_cases hx : x ∈ R.blocks
      · rw [hnew x hx]
        constructor
        · intro hw; have := (hi1.workIff x).mp hw; rw [(hblk1 x hx).1] at this; cases this
        · intro h; cases h
      · rw [hheap x hx]; exact hi1.workIff x
    dyingHeap := by
      intro x
      by_cases hx : x ∈ R.blocks
      · rw [hnew x hx]; intro h; cases h
      · rw [hheap x hx]; exact hi1.dyingHeap x
    resetDying := by
      intro x
      by_cases hx : x ∈ R.blocks
      · rw [hnew x hx]; intro _; rfl
      · rw [hheap x hx]; exact hi1.resetDying x
    nodupT := hi1.nodupT
    tokOk := by
      intro u hu
      refine (TokOk_congr (s := s1) u ?_).mpr (hi1.tokOk u hu)
      intro x hx
      by_cases hm : x ∈ R.blocks
      · exact absurd hx (hntok u hu x hm)
      · exact hheap x hm
    resetTok := by
      intro x t hdx hrs
      by_cases hx : x ∈ R.blocks
      · rw [hnew x hx] at hdx; cases hdx
      · rw [hheap x hx] at hdx hrs; exact hi1.resetTok x t hdx hrs
    rawTok := by
      intro x hrw
      by_cases hx : x ∈ R.blocks
      · rw [hnew x hx] at hrw; cases hrw
      · rw [hheap x hx] at hrw ⊢; exact hi1.rawTok x hrw
    tokAddr := hi1.tokAddr
    nodupR := hnodupR.2
    regBlocks := by
      intro R' hR' x hx
      have hR's : R' ∈ s1.regions := by rw [hr1, hrs]; exact List.mem_cons_of_mem _ hR'
      have h0 := hi1.regBlocks R' hR's x hx
      have hxR : x ∉ R.blocks := by
        intro hm
        have := (hblk1 x hm).2
        rw [h0.2] at this
        have heq : R'.id = R.id := by injection this
        exact hnodupR.1 (heq ▸ List.mem_map_of_mem (f := Region.id) hR')
      rw [hheap x hxR]; exact h0
    regLive := by
      intro x g hk
      by_cases hx : x ∈ R.blocks
      · rw [hnew x hx] at hk; cases hk
      · rw [hheap x hx] at hk ⊢
        obtain ⟨hlv, R', hR', h1, h2⟩ := hi1.regLive x g hk
        rw [hr1, hrs] at hR'
        rcases List.mem_cons.mp hR' with rfl | hR'
        · exact absurd h2 hx
        · exact ⟨hlv, R', hR', h1, h2⟩
    bornLt := by
      intro x hx
      by_cases hxm : x ∈ R.blocks
      · rw [hnew x hxm] at hx; exact absurd rfl hx
      · rw [hheap x hxm] at hx ⊢; exact hi1.bornLt x hx
    finDoneLive := by
      intro x
      by_cases hx : x ∈ R.blocks
      · rw [hnew x hx]; intro h; cases h
      · rw [hheap x hx]; exact hi1.finDoneLive x
    finDoneEv := by
      intro x hdx
      by_cases hx : x ∈ R.blocks
      · rw [hnew x hx] at hdx; cases hdx
      · rw [hheap x hx] at hdx ⊢; exact hi1.finDoneEv x hdx
    evOk := by
      intro x g ok he
      have := hi1.evOk x g ok he
      by_cases hx : x ∈ R.blocks
      · rw [hnew x hx]
        exact ⟨this.1, fun _ => ⟨fun h => by simp at h, fun h => by simp at h⟩⟩
      · rw [hheap x hx]; exact this
    nodupE := hi1.nodupE }

end

end Rc
