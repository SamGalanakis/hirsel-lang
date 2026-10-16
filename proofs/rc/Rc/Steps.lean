import Rc.Region

/-! # Every legal step preserves the invariant -/

namespace Rc

section
variable {cfg : Config} {s s' : State}

/-- The empty initial state satisfies the invariant. -/
theorem inv_init : Inv cfg ({} : State) := by
  refine {
    frontier := fun _ _ => rfl
    nodupH := List.nodup_nil
    tgtOk := fun _ h => by simp at h
    srcOk := fun _ h => by simp at h
    counts := fun _ h => by simp at h
    staticImm := fun _ h => by simp at h
    finNotImm := fun _ h => by simp at h
    nodupB := List.nodup_nil
    bGuard := fun _ h => by simp at h
    bParent := fun _ h => by simp at h
    nodupL := List.nodup_nil
    lTgt := fun _ h => by simp at h
    nodupW := List.nodup_nil
    workIff := fun _ => by simp
    dyingHeap := fun _ h => by simp at h
    resetDying := fun _ _ => rfl
    nodupT := List.nodup_nil
    tokOk := fun _ h => by simp at h
    resetTok := fun _ _ h => by simp at h
    rawTok := fun _ h => by simp at h
    tokAddr := fun _ h => by simp at h
    nodupR := List.nodup_nil
    regBlocks := fun _ h => by simp at h
    regLive := fun _ _ h => by simp at h
    bornLt := fun _ h => by simp at h
    finDoneLive := fun _ h => by simp at h
    finDoneEv := fun _ h => by simp at h
    evOk := fun _ _ _ h => by simp at h
    nodupE := List.nodup_nil }

/-- A unique block's fields guard no borrow unless its own root does. -/
theorem Inv.no_guard_field_unique (hi : Inv cfg s) {cb : Credit} (hcb : cb ∈ s.credits)
    (h1 : (s.heap cb.tgt).cnt = .fin 1) (hng : ∀ b ∈ s.borrows, b.guard ≠ cb.h) :
    ∀ b ∈ s.borrows, ∀ k, b.guard ≠ .field cb.tgt k := by
  intro b hb k hg
  have hp := hi.bParent b hb
  unfold ParentOk at hp
  split at hp
  · obtain ⟨r, hr⟩ := hp; rw [hr] at hg; cases hg
  · obtain ⟨pb, hpb, _, k', hk'⟩ := hp
    rw [hk'] at hg
    injection hg with hpt _
    obtain ⟨d, hd, hdh, hdt, _⟩ := hi.bGuard pb hpb
    have := hi.only_credit hcb hd h1 (hdt.trans hpt)
    subst this
    exact hng pb hpb hdh.symm
  · obtain ⟨lc, hlc, _, k', hk'⟩ := hp
    rw [hk'] at hg
    injection hg with hlt _
    have h1 := (hi.lTgt lc hlc).2.2
    have h2 := (hi.tgtOk cb hcb).2.2
    rw [hlt] at h1; rw [h1] at h2; cases h2

theorem opAlloc_inv (hok : cfg.Ok) (hi : Inv cfg s) {r arity : Nat} {fin : Bool}
    (h : opAlloc s r arity fin = .ok s') : Inv cfg s' := by
  unfold opAlloc at h
  split at h
  · cases h
  · rename_i hu; cases h
    obtain ⟨hf, hle⟩ := freeAddr_spec hi
    exact hi.installCounted hok hf hle (holderUsed_false (by simpa using hu)) (heapBlock_ok arity fin)

theorem opStatic_inv (hok : cfg.Ok) (hi : Inv cfg s) {r : Nat}
    (h : opStatic s r = .ok s') : Inv cfg s' := by
  unfold opStatic at h
  split at h
  · cases h
  · rename_i hu; cases h
    obtain ⟨hf, hle⟩ := freeAddr_spec hi
    exact hi.installCounted hok hf hle (holderUsed_false (by simpa using hu)) staticBlock_ok

theorem opDup_inv (hi : Inv cfg s) {src dst : Nat} (h : opDup cfg s src dst = .ok s') :
    Inv cfg s' := by
  unfold opDup at h
  split at h
  · cases h
  · rename_i c hc
    split at h
    · cases h
    · rename_i hu
      split at h
      · cases h
      · rename_i hf; cases h
        exact hi.share (hi.rootOf_mem hc).1 (holderUsed_false (by simpa using hu)) (by simpa using hf)

theorem opDrop_inv (hi : Inv cfg s) {r : Nat} (h : opDrop s r = .ok s') : Inv cfg s' := by
  unfold opDrop at h
  split at h
  · cases h
  · rename_i c hc
    split at h
    · cases h
    · rename_i hg; cases h
      obtain ⟨hcm, hch⟩ := hi.rootOf_mem hc
      exact hi.release hcm (by rw [hch]; exact guarded_false (by simpa using hg))

theorem opTake_inv (hi : Inv cfg s) {b k r : Nat} (h : opTake s b k r = .ok s') : Inv cfg s' := by
  unfold opTake at h
  split at h
  · cases h
  · rename_i cb hcb
    obtain ⟨hcbm, hcbh⟩ := hi.rootOf_mem hcb
    split at h
    · cases h
    · rename_i h1
      have h1 : (s.heap cb.tgt).cnt = .fin 1 := by simpa using h1
      split at h
      · cases h
      · rename_i hg
        have hg := guarded_false (by simpa using hg)
        split at h
        · cases h
        · rename_i hu
          split at h
          · cases h
          · rename_i o ho; cases h
            obtain ⟨hom, hoh⟩ := fieldOf_mem ho
            refine hi.relabel hom ?_ (holderUsed_false (by simpa using hu)) (fun _ _ he => by cases he)
            rw [hoh]
            intro x hx
            exact hi.no_guard_field_unique hcbm h1 (by rw [hcbh]; exact hg) x hx k

theorem opLoad_inv (hi : Inv cfg s) {p k r : Nat} (h : opLoad cfg s p k r = .ok s') :
    Inv cfg s' := by
  unfold opLoad at h
  split at h
  · cases h
  · rename_i bp hbp
    split at h
    · cases h
    · rename_i o ho
      split at h
      · cases h
      · rename_i hu
        split at h
        · cases h
        · rename_i hf; cases h
          exact hi.share (fieldOf_mem ho).1 (holderUsed_false (by simpa using hu)) (by simpa using hf)

theorem opBorrow_inv (hi : Inv cfg s) {id r : Nat} (h : opBorrow s id r = .ok s') :
    Inv cfg s' := by
  unfold opBorrow at h
  split at h
  · cases h
  · rename_i hid
    split at h
    · cases h
    · rename_i c hc; cases h
      obtain ⟨hcm, hch⟩ := hi.rootOf_mem hc
      refine hi.addBorrow (borrowOf_none (by simpa using hid)) ⟨c, hcm, hch, rfl, rfl⟩ ?_
      unfold ParentOk; exact ⟨r, rfl⟩

theorem opBorrowField_inv (hi : Inv cfg s) {id p k : Nat} (h : opBorrowField s id p k = .ok s') :
    Inv cfg s' := by
  unfold opBorrowField at h
  split at h
  · cases h
  · rename_i hid
    split at h
    · cases h
    · rename_i bp hbp
      split at h
      · cases h
      · rename_i o ho; cases h
        obtain ⟨hom, hoh⟩ := fieldOf_mem ho
        obtain ⟨hbpm, hbpid⟩ := borrowOf_mem hbp
        refine hi.addBorrow (borrowOf_none (by simpa using hid)) ⟨o, hom, rfl, rfl, rfl⟩ ?_
        unfold ParentOk; exact ⟨bp, hbpm, hbpid, k, hoh⟩

theorem opBorrowLocal_inv (hi : Inv cfg s) {id l k : Nat} (h : opBorrowLocal s id l k = .ok s') :
    Inv cfg s' := by
  unfold opBorrowLocal at h
  split at h
  · cases h
  · rename_i hid
    split at h
    · cases h
    · rename_i lc hlc
      split at h
      · cases h
      · rename_i o ho; cases h
        obtain ⟨hom, hoh⟩ := fieldOf_mem ho
        obtain ⟨hlcm, hlcid⟩ := localOf_mem hlc
        have hp := hi.publish (hi.lTgt lc hlcm).1
        refine hp.addBorrow (by simpa using borrowOf_none (by simpa using hid))
          ⟨o, by simpa using hom, rfl, rfl, rfl⟩ ?_
        unfold ParentOk; exact ⟨lc, by simpa using hlcm, hlcid, k, hoh⟩

theorem opEndBorrow_inv (hi : Inv cfg s) {id : Nat} (h : opEndBorrow s id = .ok s') :
    Inv cfg s' := by
  unfold opEndBorrow at h
  split at h
  · cases h
  · rename_i bb hbb
    split at h
    · cases h
    · rename_i hch; cases h
      obtain ⟨hbbm, hbbid⟩ := borrowOf_mem hbb
      refine hi.endBorrow hbbm ?_
      intro x hx he
      simp at hch
      exact hch x hx (by rw [he, hbbid])

theorem opForget_inv (hi : Inv cfg s) {r : Nat} (h : opForget s r = .ok s') : Inv cfg s' := by
  unfold opForget at h
  split at h
  · cases h
  · rename_i c hc
    split at h
    · cases h
    · rename_i hg
      split at h
      · cases h
      · rename_i hu; cases h
        obtain ⟨hcm, hch⟩ := hi.rootOf_mem hc
        exact hi.relabel hcm (by rw [hch]; exact guarded_false (by simpa using hg))
          (holderUsed_false (by simpa using hu)) (fun _ _ he => by cases he)

theorem opRegionOpen_inv (hi : Inv cfg s) {g : Nat} (h : opRegionOpen s g = .ok s') :
    Inv cfg s' := by
  unfold opRegionOpen at h
  split at h
  · cases h
  · rename_i hu; cases h
    refine hi.regionOpen ?_
    intro R hR he; simp [regionUsed] at hu; exact hu R hR he

theorem opRegionAlloc_inv (hi : Inv cfg s) {g l arity : Nat} (h : opRegionAlloc s g l arity = .ok s') :
    Inv cfg s' := by
  unfold opRegionAlloc at h
  split at h
  · cases h
  · rename_i hg
    split at h
    · cases h
    · rename_i hl; cases h
      obtain ⟨hf, hle⟩ := freeAddr_spec hi
      refine hi.installRegion hf hle (localOf_none (by simpa using hl)) ?_
      simp [regionUsed] at hg; exact hg

theorem opLocalCopy_inv (hi : Inv cfg s) {l l' : Nat} (h : opLocalCopy s l l' = .ok s') :
    Inv cfg s' := by
  unfold opLocalCopy at h
  split at h
  · cases h
  · rename_i lc hlc
    split at h
    · cases h
    · rename_i hl; cases h
      exact hi.localCopy (localOf_mem hlc).1 (localOf_none (by simpa using hl))

theorem opLocalDrop_inv (hi : Inv cfg s) {l : Nat} (h : opLocalDrop s l = .ok s') :
    Inv cfg s' := by
  unfold opLocalDrop at h
  split at h
  · cases h
  · rename_i lc hlc
    split at h
    · cases h
    · rename_i hch; cases h
      obtain ⟨hlcm, hlcid⟩ := localOf_mem hlc
      refine hi.localDrop hlcm ?_
      intro b hb he
      simp at hch
      exact hch b hb (by rw [he, hlcid])

theorem opRegionClose_inv (hi : Inv cfg s) {g : Nat} (h : opRegionClose s g = .ok s') :
    Inv cfg s' := by
  unfold opRegionClose at h
  split at h
  · cases h
  · rename_i R rest hrs
    split at h
    · cases h
    · split at h
      · cases h
      · rename_i hloc; cases h
        refine hi.regionClose hrs ?_
        intro l hl hm
        simp at hloc
        exact hloc l hl hm

theorem release_heap_die (s : State) (c : Credit) (h : (s.heap c.tgt).cnt = .fin 1) :
    (Rc.release s c).heap c.tgt = { s.heap c.tgt with st := .dying, cnt := .dead } := by
  simp [Rc.release, h]

theorem opReset_inv (hi : Inv cfg s) {b t : Nat} (h : opReset s b t = .ok s') : Inv cfg s' := by
  unfold opReset at h
  split at h
  · cases h
  · rename_i cb hcb
    obtain ⟨hcbm, hcbh⟩ := hi.rootOf_mem hcb
    split at h
    · cases h
    · rename_i hg
      have hg := guarded_false (by simpa using hg)
      split at h
      · cases h
      · rename_i ht
        have ht := tokenOf_none (by simpa using ht)
        have hi1 := hi.release hcbm (by rw [hcbh]; exact hg)
        split at h
        · rename_i h1; cases h
          have hl := (hi.tgtOk cb hcbm).1
          refine hi1.markReset ?_ ?_ (by simpa using ht)
          · rw [release_heap_die s cb h1]
          · rw [release_heap_die s cb h1]; exact hi.resetDying _ (by rw [hl]; decide)
        · cases h
          exact hi1.addNoneTok (by simpa using ht)

theorem opReuse_inv (hok : cfg.Ok) (hi : Inv cfg s) {t r arity : Nat} {fin : Bool}
    (h : opReuse s t r arity fin = .ok s') : Inv cfg s' := by
  unfold opReuse at h
  split at h
  · cases h
  · rename_i tok htok
    obtain ⟨htm, _⟩ := tokenOf_mem htok
    split at h
    · cases h
    · rename_i hu
      have hu := holderUsed_false (by simpa using hu)
      split at h
      · rename_i hv; cases h
        have hi1 := hi.eraseNoneTok hv
        obtain ⟨hf, hle⟩ := freeAddr_spec hi1
        exact hi1.installCounted hok hf hle hu (heapBlock_ok arity fin)
      · cases h
      · rename_i a g cap hv
        split at h
        · cases h
        · cases h
          have hi1 := hi.freeRaw htm hv
          have htk := hi.tokOk tok htm
          unfold TokOk at htk; rw [hv] at htk
          have hlt := hi.live_lt_next (s := s) (a := a) (by rw [htk.1]; decide)
          refine hi1.installCounted hok (by simp [Rc.freeRaw]) (by simp [Rc.freeRaw]; omega)
            (by simpa [Rc.freeRaw] using hu) (heapBlock_ok arity fin)

theorem opDispose_inv (hi : Inv cfg s) {t : Nat} (h : opDispose s t = .ok s') : Inv cfg s' := by
  unfold opDispose at h
  split at h
  · cases h
  · rename_i tok htok
    obtain ⟨htm, _⟩ := tokenOf_mem htok
    split at h
    · rename_i hv; cases h; exact hi.eraseNoneTok hv
    · cases h
    · rename_i a g cap hv; cases h; exact hi.freeRaw htm hv

theorem opFinalize_inv (hi : Inv cfg s) {ok : Bool} (h : opFinalize s ok = .ok s') : Inv cfg s' := by
  unfold opFinalize at h
  split at h
  · cases h
  · rename_i a rest hw
    split at h
    · cases h
    · rename_i hdue; cases h
      simp at hdue
      exact hi.runFinalizer hw hdue ok

theorem opDrain_inv (hi : Inv cfg s) (h : opDrain s = .ok s') : Inv cfg s' := by
  unfold opDrain at h
  split at h
  · cases h
  · rename_i a rest hw
    obtain ⟨hd, _, _, _, _, _, hnl', _⟩ := hi.drained_head hw
    split at h
    · cases h
    · split at h
      · rename_i c hc; cases h
        obtain ⟨hcm, k, hck⟩ := mem_fieldsOf.mp (pickMax_mem hc)
        refine hi.release hcm ?_
        rw [hck]
        exact hi.no_guard_field (Or.inl (by rw [hd]; decide)) (fun l hl he => hnl' l hl he)
      · rename_i hc
        have hnf : ∀ c ∈ s.credits, ∀ k, c.h ≠ .field a k := by
          intro c hcm k hck
          have := pickMax_none hc
          have hm : c ∈ fieldsOf s a := mem_fieldsOf.mpr ⟨hcm, k, hck⟩
          rw [this] at hm; cases hm
        split at h
        · rename_i hr; cases h; exact hi.retireFree hw hnf hr
        · rename_i t hr; cases h; exact hi.retireRaw hw hnf hr

theorem opWrite_inv (hi : Inv cfg s) {b k r : Nat} (h : opWrite s b k r = .ok s') : Inv cfg s' := by
  unfold opWrite at h
  split at h
  · cases h
  · rename_i cb hcb
    obtain ⟨hcbm, hcbh⟩ := hi.rootOf_mem hcb
    split at h
    · cases h
    · rename_i h1
      have h1 : (s.heap cb.tgt).cnt = .fin 1 := by simpa using h1
      split at h
      · cases h
      · rename_i hk
        have hk : k < (s.heap cb.tgt).arity := by omega
        split at h
        · cases h
        · rename_i hgb
          have hgb := guarded_false (by simpa using hgb)
          split at h
          · cases h
          · rename_i hrb
            split at h
            · cases h
            · rename_i cr hcr
              obtain ⟨hcrm, hcrh⟩ := hi.rootOf_mem hcr
              split at h
              · cases h
              · rename_i hgr
                have hgr := guarded_false (by simpa using hgr)
                cases h
                have ⟨hl, _, _⟩ := hi.tgtOk cb hcbm
                have hnog := hi.no_guard_field_unique hcbm h1 (by rw [hcbh]; exact hgb)
                -- cr is a different credit on a different block
                have hcrcb : cr.tgt ≠ cb.tgt := by
                  intro he
                  have := hi.only_credit hcbm hcrm h1 he
                  rw [this, hcbh] at hcrh; cases hcrh; exact hrb rfl
                -- step 1: release the displaced child, if any
                generalize hs1 : displace s cb.tgt k = s1
                have hfacts : Inv cfg s1 ∧ cb ∈ s1.credits ∧ cr ∈ s1.credits ∧
                    s1.heap cb.tgt = s.heap cb.tgt ∧ (s1.heap cr.tgt).born = (s.heap cr.tgt).born ∧
                    s1.borrows = s.borrows ∧ s1.clock = s.clock ∧
                    (∀ c ∈ s1.credits, c.h ≠ .field cb.tgt k) := by
                  unfold displace at hs1
                  split at hs1
                  · rename_i ho; subst hs1
                    refine ⟨hi, hcbm, hcrm, rfl, rfl, rfl, rfl, ?_⟩
                    intro c hc he
                    simp [fieldOf] at ho
                    exact ho c hc he
                  · rename_i o ho; subst hs1
                    obtain ⟨hom, hoh⟩ := fieldOf_mem ho
                    have hsrc := hi.srcOk o hom cb.tgt k hoh
                    have hot : o.tgt ≠ cb.tgt := by intro he; rw [he] at hsrc; omega
                    have hcn := hi.credits_nodup
                    refine ⟨hi.release hom (by rw [hoh]; exact fun x hx => hnog x hx k), ?_, ?_,
                      release_heap_ne s o (Ne.symm hot), ?_, by simp, by simp, ?_⟩
                    · rw [release_credits]
                      exact mem_erase_of_mem_ne hcbm (by intro he; rw [he, hoh] at hcbh; cases hcbh)
                    · rw [release_credits]
                      exact mem_erase_of_mem_ne hcrm (by intro he; rw [he, hoh] at hcrh; cases hcrh)
                    · by_cases hx : cr.tgt = o.tgt
                      · rw [hx]; exact (release_heap_tgt s o).2.2.2.2.2.2.1
                      · rw [release_heap_ne s o hx]
                    · intro c hc he
                      rw [release_credits] at hc
                      have hc' := (hcn.mem_erase_iff).mp hc
                      exact hc'.1 (hi.credit_eq_of_holder hc'.2 hom (by rw [he, hoh]))
                obtain ⟨hi1, hcb1, hcr1, hh1, hh2, hb1, hc1, hfree⟩ := hfacts
                -- step 2: restamp the unique block
                have h1' : (s1.heap cb.tgt).cnt = .fin 1 := by rw [hh1]; exact h1
                have hi2 := hi1.restamp (a := cb.tgt) (by rw [hh1]; exact hl) (by
                  intro d hd hdt src k' hdh
                  have := hi1.only_credit hcb1 hd h1' hdt
                  rw [this, hcbh] at hdh; cases hdh)
                -- step 3: move cr into the field
                refine hi2.relabel (c := cr) hcr1 ?_ (by simpa using hfree) ?_
                · rw [hcrh]; simp only [restamp_borrows, hb1]; exact hgr
                · intro src k' he
                  cases he
                  simp only [Rc.restamp, upd_same, upd_ne hcrcb]
                  refine ⟨Or.inl (by rw [hh1]; exact hl), by rw [hh1]; exact hk, ?_⟩
                  rw [hh2]
                  exact hc1 ▸ hi.bornLt cr.tgt (by rw [(hi.tgtOk cr hcrm).1]; decide)

theorem opRegionStore_inv (hi : Inv cfg s) {l k r : Nat} (h : opRegionStore s l k r = .ok s') :
    Inv cfg s' := by
  unfold opRegionStore at h
  split at h
  · cases h
  · rename_i lc hlc
    obtain ⟨hlcm, _⟩ := localOf_mem hlc
    have ⟨hl, _, hk⟩ := hi.lTgt lc hlcm
    split at h
    · cases h
    · split at h
      · cases h
      · rename_i hka
        have hka : k < (s.heap lc.tgt).arity := by omega
        split at h
        · cases h
        · rename_i hf
          split at h
          · cases h
          · rename_i cr hcr
            obtain ⟨hcrm, hcrh⟩ := hi.rootOf_mem hcr
            split at h
            · cases h
            · rename_i hgr; cases h
              have hcrt : cr.tgt ≠ lc.tgt := by
                intro he
                have := (hi.tgtOk cr hcrm).2.2; rw [he, hk] at this; cases this
              have hi2 := hi.restamp (a := lc.tgt) hl (by
                intro d hd hdt
                have := (hi.tgtOk d hd).2.2; rw [hdt, hk] at this; cases this)
              refine hi2.relabel (c := cr) hcrm ?_ ?_ ?_
              · rw [hcrh]
                have hg' := guarded_false (s := s) (by simpa using hgr)
                intro x hx; exact hg' x hx
              · intro c hc he
                simp [fieldOf] at hf
                exact hf c hc he
              · intro src k' he
                cases he
                simp only [Rc.restamp, upd_same, upd_ne hcrt]
                exact ⟨Or.inl hl, hka, hi.bornLt cr.tgt (by rw [(hi.tgtOk cr hcrm).1]; decide)⟩

/-- **Every legal step preserves the invariant.** -/
theorem step_inv (hok : cfg.Ok) (hi : Inv cfg s) {op : Op} (h : step cfg s op = .ok s') :
    Inv cfg s' := by
  cases op with
  | alloc r arity fin => exact opAlloc_inv hok hi h
  | static r => exact opStatic_inv hok hi h
  | dup src dst => exact opDup_inv hi h
  | drop r => exact opDrop_inv hi h
  | write b k r => exact opWrite_inv hi h
  | take b k r => exact opTake_inv hi h
  | load p k r => exact opLoad_inv hi h
  | borrow id r => exact opBorrow_inv hi h
  | borrowField id p k => exact opBorrowField_inv hi h
  | borrowLocal id l k => exact opBorrowLocal_inv hi h
  | endBorrow id => exact opEndBorrow_inv hi h
  | reset b t => exact opReset_inv hi h
  | reuse t r arity fin => exact opReuse_inv hok hi h
  | dispose t => exact opDispose_inv hi h
  | finalize ok => exact opFinalize_inv hi h
  | drain => exact opDrain_inv hi h
  | forget r => exact opForget_inv hi h
  | regionOpen g => exact opRegionOpen_inv hi h
  | regionAlloc g l arity => exact opRegionAlloc_inv hi h
  | regionStore l k r => exact opRegionStore_inv hi h
  | localCopy l l' => exact opLocalCopy_inv hi h
  | localDrop l => exact opLocalDrop_inv hi h
  | regionClose g => exact opRegionClose_inv hi h

end

end Rc
