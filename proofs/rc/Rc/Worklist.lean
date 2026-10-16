import Rc.Tokens

/-! # Finalizer events and retiring drained blocks preserve the invariant -/

namespace Rc

theorem pickMax_mem : ∀ {l : List Credit} {c : Credit}, pickMax l = some c → c ∈ l
  | [], _, h => by simp [pickMax] at h
  | d :: l, c, h => by
    simp only [pickMax] at h
    split at h
    · cases h; exact List.mem_cons_self
    · rename_i e he
      split at h
      · cases h; exact List.mem_cons_self
      · cases h; exact List.mem_cons_of_mem _ (pickMax_mem he)

theorem pickMax_none : ∀ {l : List Credit}, pickMax l = none → l = []
  | [], _ => rfl
  | d :: l, h => by
    simp only [pickMax] at h
    split at h
    · cases h
    · split at h <;> cases h

theorem isFieldOf_iff {a : Nat} {h : Holder} : h.isFieldOf a = true ↔ ∃ k, h = .field a k := by
  cases h <;> simp [Holder.isFieldOf]

theorem mem_fieldsOf {s : State} {a : Nat} {c : Credit} :
    c ∈ fieldsOf s a ↔ c ∈ s.credits ∧ ∃ k, c.h = .field a k := by
  simp [fieldsOf, isFieldOf_iff]

@[simp] theorem runFinalizer_credits (s : State) (a : Nat) (ok : Bool) : (Rc.runFinalizer s a ok).credits = s.credits := rfl
@[simp] theorem runFinalizer_borrows (s : State) (a : Nat) (ok : Bool) : (Rc.runFinalizer s a ok).borrows = s.borrows := rfl
@[simp] theorem runFinalizer_locals (s : State) (a : Nat) (ok : Bool) : (Rc.runFinalizer s a ok).locals = s.locals := rfl
@[simp] theorem runFinalizer_work (s : State) (a : Nat) (ok : Bool) : (Rc.runFinalizer s a ok).work = s.work := rfl
@[simp] theorem runFinalizer_regions (s : State) (a : Nat) (ok : Bool) : (Rc.runFinalizer s a ok).regions = s.regions := rfl
@[simp] theorem runFinalizer_next (s : State) (a : Nat) (ok : Bool) : (Rc.runFinalizer s a ok).next = s.next := rfl
@[simp] theorem runFinalizer_clock (s : State) (a : Nat) (ok : Bool) : (Rc.runFinalizer s a ok).clock = s.clock := rfl

section
variable {cfg : Config} {s : State}

/-- The pending token of a dying block, if any, is the one its `reset` names. -/
theorem Inv.pending_id (hi : Inv cfg s) {u : Token} (hu : u ∈ s.tokens) {a : Nat}
    (hv : u.val = .pending a) : (s.heap a).reset = some u.id := by
  have := hi.tokOk u hu; unfold TokOk at this; rw [hv] at this; exact this.2

theorem Inv.runFinalizer (hi : Inv cfg s) {a : Nat} {rest : List Nat} (hw : s.work = a :: rest)
    (hdue : (s.heap a).fin = true ∧ (s.heap a).finDone = false) (ok : Bool) :
    Inv cfg (Rc.runFinalizer s a ok) := by
  have hd : (s.heap a).st = .dying := (hi.workIff a).mp (by rw [hw]; exact List.mem_cons_self)
  have hnl : (s.heap a).st ≠ .live := by rw [hd]; decide
  obtain ⟨hnt, hnl', hnreg, hnk⟩ := hi.unref_of_not_live hnl
  have hlt := hi.live_lt_next (s := s) (a := a) (by rw [hd]; decide)
  -- no earlier event for this generation
  have hnoev : ∀ ok', Event.finalized a (s.heap a).gen ok' ∉ s.events := by
    intro ok' he
    have := (hi.evOk a _ ok' he).2 rfl
    rw [this.2 hd] at hdue; cases hdue.2
  have hheap : ∀ x, x ≠ a → (Rc.runFinalizer s a ok).heap x = s.heap x := by
    intro x hx; simp [Rc.runFinalizer, hx]
  have hnew : (Rc.runFinalizer s a ok).heap a =
      { s.heap a with finDone := true, reset := if ok then (s.heap a).reset else none } := by
    simp [Rc.runFinalizer]
  have hst : ∀ x, ((Rc.runFinalizer s a ok).heap x).st = (s.heap x).st := by
    intro x; by_cases hx : x = a
    · subst hx; rw [hnew]
    · rw [hheap x hx]
  have hgen : ∀ x, ((Rc.runFinalizer s a ok).heap x).gen = (s.heap x).gen := by
    intro x; by_cases hx : x = a
    · subst hx; rw [hnew]
    · rw [hheap x hx]
  have hkind : ∀ x, ((Rc.runFinalizer s a ok).heap x).kind = (s.heap x).kind := by
    intro x; by_cases hx : x = a
    · subst hx; rw [hnew]
    · rw [hheap x hx]
  have hari : ∀ x, ((Rc.runFinalizer s a ok).heap x).arity = (s.heap x).arity := by
    intro x; by_cases hx : x = a
    · subst hx; rw [hnew]
    · rw [hheap x hx]
  have hfin : ∀ x, ((Rc.runFinalizer s a ok).heap x).fin = (s.heap x).fin := by
    intro x; by_cases hx : x = a
    · subst hx; rw [hnew]
    · rw [hheap x hx]
  have hborn : ∀ x, ((Rc.runFinalizer s a ok).heap x).born = (s.heap x).born := by
    intro x; by_cases hx : x = a
    · subst hx; rw [hnew]
    · rw [hheap x hx]
  have hcnt : ∀ x, ((Rc.runFinalizer s a ok).heap x).cnt = (s.heap x).cnt := by
    intro x; by_cases hx : x = a
    · subst hx; rw [hnew]
    · rw [hheap x hx]
  have hicount : ∀ x, icount (Rc.runFinalizer s a ok) x = icount s x := fun _ => rfl
  have hev : (Rc.runFinalizer s a ok).events = .finalized a (s.heap a).gen ok :: s.events := rfl
  -- the token side: either unchanged, or the pending token of `a` becomes `none`
  have htok : (Rc.runFinalizer s a ok).tokens = s.tokens ∧
        ((Rc.runFinalizer s a ok).heap a).reset = (s.heap a).reset ∨
      ∃ t, (s.heap a).reset = some t ∧ ((Rc.runFinalizer s a ok).heap a).reset = none ∧
        (Rc.runFinalizer s a ok).tokens = setTok s.tokens t .none := by
    cases ok with
    | true => left; simp [Rc.runFinalizer]
    | false =>
      cases hr : (s.heap a).reset with
      | none => left; simp [Rc.runFinalizer, hr]
      | some t => right; exact ⟨t, rfl, by simp [Rc.runFinalizer], by simp [Rc.runFinalizer, hr]⟩
  have hresetOther : ∀ x, x ≠ a → ((Rc.runFinalizer s a ok).heap x).reset = (s.heap x).reset := by
    intro x hx; rw [hheap x hx]
  -- tokens whose value is untouched keep their meaning
  have htokOk : ∀ u ∈ (Rc.runFinalizer s a ok).tokens, TokOk (Rc.runFinalizer s a ok) u := by
    intro u hu
    rcases htok with ⟨ht, hr⟩ | ⟨t, hr, hr', ht⟩
    · rw [ht] at hu
      have h0 := hi.tokOk u hu
      unfold TokOk at h0 ⊢
      split at h0
      · trivial
      · rename_i x hv
        by_cases hx : x = a
        · subst hx; rw [hst, hr]; exact h0
        · rw [hheap x hx]; exact h0
      · rename_i x _ _ hv
        by_cases hx : x = a
        · subst hx; rw [hd] at h0; cases h0.1
        · rw [hheap x hx]; exact h0
    · rw [ht] at hu
      obtain ⟨u0, hu0, rfl⟩ := mem_setTok.mp hu
      by_cases hid : u0.id = t
      · simp only [hid, ite_true]; unfold TokOk; trivial
      · simp only [hid, ite_false]
        have h0 := hi.tokOk u0 hu0
        unfold TokOk at h0 ⊢
        split at h0
        · trivial
        · rename_i x hv
          by_cases hx : x = a
          · subst hx; rw [hr] at h0; cases h0.2; exact absurd rfl hid
          · rw [hheap x hx]; exact h0
        · rename_i x _ _ hv
          by_cases hx : x = a
          · subst hx; rw [hd] at h0; cases h0.1
          · rw [hheap x hx]; exact h0
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
      rw [hst] at hx
      have hxa : x ≠ a := by intro he; subst he; exact hx hd
      rw [hheap x hxa]; exact hi.resetDying x hx
    nodupT := by
      rcases htok with ⟨ht, _⟩ | ⟨t, _, _, ht⟩
      · rw [ht]; exact hi.nodupT
      · rw [ht, setTok_ids]; exact hi.nodupT
    tokOk := htokOk
    resetTok := by
      intro x t' hdx hrs
      rw [hst] at hdx
      rcases htok with ⟨ht, hr⟩ | ⟨t, hr, hr', ht⟩
      · rw [ht]
        by_cases hxa : x = a
        · subst hxa; rw [hr] at hrs; exact hi.resetTok x t' hdx hrs
        · rw [hheap x hxa] at hrs; exact hi.resetTok x t' hdx hrs
      · by_cases hxa : x = a
        · subst hxa; rw [hr'] at hrs; cases hrs
        · rw [hheap x hxa] at hrs
          obtain ⟨tk, htk, h1, h2⟩ := hi.resetTok x t' hdx hrs
          rw [ht]
          refine ⟨tk, mem_setTok.mpr ⟨tk, htk, ?_⟩, h1, h2⟩
          have : tk.id ≠ t := by
            intro he
            obtain ⟨ta, hta, h1a, h2a⟩ := hi.resetTok a t hd hr
            have := hi.token_eq_of_id htk hta (by rw [he, h1a])
            subst this; rw [h2] at h2a; cases h2a; exact hxa rfl
          simp [this]
    rawTok := by
      intro x hrw
      rw [hst] at hrw
      obtain ⟨tk, htk, h1⟩ := hi.rawTok x hrw
      rw [hgen, hari]
      rcases htok with ⟨ht, _⟩ | ⟨t, hr, _, ht⟩
      · rw [ht]; exact ⟨tk, htk, h1⟩
      · rw [ht]
        refine ⟨tk, mem_setTok.mpr ⟨tk, htk, ?_⟩, h1⟩
        have : tk.id ≠ t := by
          intro he
          obtain ⟨ta, hta, h1a, h2a⟩ := hi.resetTok a t hd hr
          have := hi.token_eq_of_id htk hta (by rw [he, h1a])
          subst this; rw [h1] at h2a; cases h2a
        simp [this]
    tokAddr := by
      rcases htok with ⟨ht, _⟩ | ⟨t, _, _, ht⟩
      · rw [ht]; exact hi.tokAddr
      · rw [ht]
        intro u hu v hv x hua hva
        obtain ⟨u0, hu0, rfl⟩ := mem_setTok.mp hu
        obtain ⟨v0, hv0, rfl⟩ := mem_setTok.mp hv
        by_cases hu1 : u0.id = t
        · simp [hu1, TokVal.addr] at hua
        by_cases hv1 : v0.id = t
        · simp [hv1, TokVal.addr] at hva
        simp only [hu1, hv1, ite_false] at hua hva ⊢
        exact hi.tokAddr u0 hu0 v0 hv0 x hua hva
    nodupR := hi.nodupR
    regBlocks := by intro R hR x hx; simpa [hst, hkind] using hi.regBlocks R hR x hx
    regLive := by intro x g; simpa [hst, hkind] using hi.regLive x g
    bornLt := by intro x; simpa [hst, hborn] using hi.bornLt x
    finDoneLive := by
      intro x hx
      rw [hst] at hx
      have hxa : x ≠ a := by intro he; subst he; exact hnl hx
      rw [hheap x hxa]; exact hi.finDoneLive x hx
    finDoneEv := by
      intro x hdx hf
      rw [hev, hgen]
      by_cases hxa : x = a
      · subst hxa; exact ⟨ok, List.mem_cons_self⟩
      · rw [hst] at hdx; rw [hheap x hxa] at hf
        obtain ⟨o, ho⟩ := hi.finDoneEv x hdx hf
        exact ⟨o, List.mem_cons_of_mem _ ho⟩
    evOk := by
      intro x g o he
      rw [hev] at he
      rw [hgen, hst]
      rcases List.mem_cons.mp he with he' | he'
      · cases he'
        refine ⟨Nat.le_refl _, fun _ => ⟨hnl, fun _ => ?_⟩⟩
        rw [hnew]
      · have := hi.evOk x g o he'
        by_cases hxa : x = a
        · subst hxa
          refine ⟨this.1, fun hg => ?_⟩
          rw [hg] at he'; exact absurd he' (hnoev o)
        · rw [hheap x hxa]; exact this
    nodupE := by
      rw [hev]
      simp only [List.map_cons, List.nodup_cons]
      refine ⟨?_, hi.nodupE⟩
      intro hm
      obtain ⟨e, he, hk⟩ := List.mem_map.mp hm
      cases e with
      | finalized x g o =>
        simp [Event.key] at hk
        obtain ⟨rfl, rfl⟩ := hk
        exact hnoev o he }

/-- Facts about the head of the worklist once its fields are gone. -/
theorem Inv.drained_head (hi : Inv cfg s) {a : Nat} {rest : List Nat} (hw : s.work = a :: rest) :
    (s.heap a).st = .dying ∧ (s.heap a).kind = .heap ∧ a ∉ rest ∧ rest.Nodup ∧ a < s.next ∧
      (∀ c ∈ s.credits, c.tgt ≠ a) ∧ (∀ l ∈ s.locals, l.tgt ≠ a) ∧
      (∀ R ∈ s.regions, a ∉ R.blocks) := by
  have hd : (s.heap a).st = .dying := (hi.workIff a).mp (by rw [hw]; exact List.mem_cons_self)
  have hnl : (s.heap a).st ≠ .live := by rw [hd]; decide
  obtain ⟨hnt, hnl', hnreg, _⟩ := hi.unref_of_not_live hnl
  have hn := hi.nodupW; rw [hw, List.nodup_cons] at hn
  exact ⟨hd, hi.dyingHeap a hd, hn.1, hn.2, hi.live_lt_next (by rw [hd]; decide), hnt, hnl', hnreg⟩

theorem Inv.retireFree (hi : Inv cfg s) {a : Nat} {rest : List Nat} (hw : s.work = a :: rest)
    (hnf : ∀ c ∈ s.credits, ∀ k, c.h ≠ .field a k) (hr : (s.heap a).reset = none) :
    Inv cfg (Rc.retireFree s a rest) := by
  obtain ⟨hd, _, hna, hrn, hlt, hnt, hnl', hnreg⟩ := hi.drained_head hw
  have hother : ∀ u ∈ s.tokens, u.val.addr ≠ some a := by
    intro u hu he
    have := hi.tokOk u hu
    unfold TokOk at this
    split at this
    · rename_i hv; rw [hv] at he; cases he
    · rename_i a' hv; rw [hv] at he; cases he; rw [hr] at this; cases this.2
    · rename_i a' _ _ hv; rw [hv] at he; cases he; rw [hd] at this; cases this.1
  have hheap : ∀ x, x ≠ a → (Rc.retireFree s a rest).heap x = s.heap x := by
    intro x hx; simp [Rc.retireFree, hx]
  have hnew : (Rc.retireFree s a rest).heap a = { gen := (s.heap a).gen } := by
    simp [Rc.retireFree]
  exact {
    frontier := by
      intro x hx; have hx' : s.next ≤ x := hx
      rw [hheap x (by omega)]; exact hi.frontier x hx'
    nodupH := hi.nodupH
    tgtOk := by intro d hd'; rw [hheap _ (hnt d hd')]; exact hi.tgtOk d hd'
    srcOk := by
      intro d hd' src k hh
      have hsrc : src ≠ a := fun he => hnf d hd' k (he ▸ hh)
      rw [hheap _ (hnt d hd'), hheap _ hsrc]; exact hi.srcOk d hd' src k hh
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
    nodupW := hrn
    workIff := by
      intro x
      show x ∈ rest ↔ _
      by_cases hx : x = a
      · subst hx; rw [hnew]; simp [hna]
      · rw [hheap x hx, ← hi.workIff x, hw]; simp [hx]
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
      intro u hu
      refine (TokOk_congr u ?_).mpr (hi.tokOk u hu)
      intro x hx; apply hheap; intro he; exact hother u hu (he ▸ hx)
    resetTok := by
      intro x t hdx hrs
      by_cases hx : x = a
      · subst hx; rw [hnew] at hdx; simp at hdx
      · rw [hheap x hx] at hdx hrs; exact hi.resetTok x t hdx hrs
    rawTok := by
      intro x hrw
      by_cases hx : x = a
      · subst hx; rw [hnew] at hrw; simp at hrw
      · rw [hheap x hx] at hrw ⊢; exact hi.rawTok x hrw
    tokAddr := hi.tokAddr
    nodupR := hi.nodupR
    regBlocks := by
      intro R hR x hx
      have : x ≠ a := fun he => hnreg R hR (he ▸ hx)
      rw [hheap x this]; exact hi.regBlocks R hR x hx
    regLive := by
      intro x g hk
      by_cases hx : x = a
      · subst hx; rw [hnew] at hk; cases hk
      · rw [hheap x hx] at hk ⊢; exact hi.regLive x g hk
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
      intro x hdx
      by_cases hx : x = a
      · subst hx; rw [hnew] at hdx; simp at hdx
      · rw [hheap x hx] at hdx ⊢; exact hi.finDoneEv x hdx
    evOk := by
      intro x g ok he
      have := hi.evOk x g ok he
      by_cases hx : x = a
      · subst hx; rw [hnew]; simp; exact this.1
      · rw [hheap x hx]; exact this
    nodupE := hi.nodupE }

theorem Inv.retireRaw (hi : Inv cfg s) {a : Nat} {rest : List Nat} (hw : s.work = a :: rest)
    (hnf : ∀ c ∈ s.credits, ∀ k, c.h ≠ .field a k) {t : Nat} (hr : (s.heap a).reset = some t) :
    Inv cfg (Rc.retireRaw s a t rest) := by
  obtain ⟨hd, _, hna, hrn, hlt, hnt, hnl', hnreg⟩ := hi.drained_head hw
  obtain ⟨ta, hta, hida, hva⟩ := hi.resetTok a t hd hr
  -- every other token has a different id and a different address
  have hotherAddr : ∀ u ∈ s.tokens, u ≠ ta → u.val.addr ≠ some a := by
    intro u hu hne he
    exact hne (hi.tokAddr u hu ta hta a he (by rw [hva]; rfl))
  have hotherId : ∀ u ∈ s.tokens, u ≠ ta → u.id ≠ t := by
    intro u hu hne he; exact hne (hi.token_eq_of_id hu hta (by rw [he, hida]))
  have hheap : ∀ x, x ≠ a → (Rc.retireRaw s a t rest).heap x = s.heap x := by
    intro x hx; simp [Rc.retireRaw, hx]
  have hnew : (Rc.retireRaw s a t rest).heap a =
      { st := .raw, gen := (s.heap a).gen, arity := (s.heap a).arity } := by
    simp [Rc.retireRaw]
  have htoks : (Rc.retireRaw s a t rest).tokens =
      setTok s.tokens t (.raw a (s.heap a).gen (s.heap a).arity) := rfl
  -- membership in the updated token list
  have hmemTok : ∀ u ∈ (Rc.retireRaw s a t rest).tokens,
      (u = { ta with val := .raw a (s.heap a).gen (s.heap a).arity }) ∨
        (u ∈ s.tokens ∧ u ≠ ta) := by
    intro u hu
    rw [htoks] at hu
    obtain ⟨u0, hu0, rfl⟩ := mem_setTok.mp hu
    by_cases hid : u0.id = t
    · left
      have : u0 = ta := hi.token_eq_of_id hu0 hta (by rw [hid, hida])
      subst this; simp [hid]
    · right; simp only [hid, ite_false]; exact ⟨hu0, fun he => hid (he ▸ hida)⟩
  have hkeep : ∀ u ∈ s.tokens, u ≠ ta → u ∈ (Rc.retireRaw s a t rest).tokens := by
    intro u hu hne
    rw [htoks]; refine mem_setTok.mpr ⟨u, hu, ?_⟩
    simp [hotherId u hu hne]
  have hta' : { ta with val := .raw a (s.heap a).gen (s.heap a).arity } ∈
      (Rc.retireRaw s a t rest).tokens := by
    rw [htoks]; exact mem_setTok.mpr ⟨ta, hta, by simp [hida]⟩
  exact {
    frontier := by
      intro x hx; have hx' : s.next ≤ x := hx
      rw [hheap x (by omega)]; exact hi.frontier x hx'
    nodupH := hi.nodupH
    tgtOk := by intro d hd'; rw [hheap _ (hnt d hd')]; exact hi.tgtOk d hd'
    srcOk := by
      intro d hd' src k hh
      have hsrc : src ≠ a := fun he => hnf d hd' k (he ▸ hh)
      rw [hheap _ (hnt d hd'), hheap _ hsrc]; exact hi.srcOk d hd' src k hh
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
    nodupW := hrn
    workIff := by
      intro x
      show x ∈ rest ↔ _
      by_cases hx : x = a
      · subst hx; rw [hnew]; simp [hna]
      · rw [hheap x hx, ← hi.workIff x, hw]; simp [hx]
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
    nodupT := by rw [htoks, setTok_ids]; exact hi.nodupT
    tokOk := by
      intro u hu
      rcases hmemTok u hu with rfl | ⟨hu', hne⟩
      · unfold TokOk; simp only; rw [hnew]; exact ⟨rfl, rfl, rfl⟩
      · refine (TokOk_congr u ?_).mpr (hi.tokOk u hu')
        intro x hx; apply hheap; intro he; exact hotherAddr u hu' hne (he ▸ hx)
    resetTok := by
      intro x t' hdx hrs
      by_cases hx : x = a
      · subst hx; rw [hnew] at hdx; simp at hdx
      · rw [hheap x hx] at hdx hrs
        obtain ⟨tk, htk, h1, h2⟩ := hi.resetTok x t' hdx hrs
        refine ⟨tk, hkeep tk htk ?_, h1, h2⟩
        rintro rfl; rw [hva] at h2; cases h2; exact hx rfl
    rawTok := by
      intro x hrw
      by_cases hx : x = a
      · subst hx; rw [hnew]; exact ⟨_, hta', rfl⟩
      · rw [hheap x hx] at hrw ⊢
        obtain ⟨tk, htk, h1⟩ := hi.rawTok x hrw
        refine ⟨tk, hkeep tk htk ?_, h1⟩
        rintro rfl; rw [hva] at h1; cases h1
    tokAddr := by
      intro u hu v hv x hua hva'
      rcases hmemTok u hu with rfl | ⟨hu', hune⟩ <;> rcases hmemTok v hv with rfl | ⟨hv', hvne⟩
      · rfl
      · simp [TokVal.addr] at hua; subst hua; exact absurd hva' (hotherAddr v hv' hvne)
      · simp [TokVal.addr] at hva'; subst hva'; exact absurd hua (hotherAddr u hu' hune)
      · exact hi.tokAddr u hu' v hv' x hua hva'
    nodupR := hi.nodupR
    regBlocks := by
      intro R hR x hx
      have : x ≠ a := fun he => hnreg R hR (he ▸ hx)
      rw [hheap x this]; exact hi.regBlocks R hR x hx
    regLive := by
      intro x g hk
      by_cases hx : x = a
      · subst hx; rw [hnew] at hk; cases hk
      · rw [hheap x hx] at hk ⊢; exact hi.regLive x g hk
    bornLt := by
      intro x hx
      by_cases hxa : x = a
      · subst hxa; rw [hnew]; simp
        exact hi.bornLt x (by rw [hd]; decide) |> fun h => Nat.lt_of_le_of_lt (Nat.zero_le _) h
      · rw [hheap x hxa] at hx ⊢; exact hi.bornLt x hx
    finDoneLive := by
      intro x
      by_cases hx : x = a
      · subst hx; rw [hnew]; simp
      · rw [hheap x hx]; exact hi.finDoneLive x
    finDoneEv := by
      intro x hdx
      by_cases hx : x = a
      · subst hx; rw [hnew] at hdx; simp at hdx
      · rw [hheap x hx] at hdx ⊢; exact hi.finDoneEv x hdx
    evOk := by
      intro x g ok he
      have := hi.evOk x g ok he
      by_cases hx : x = a
      · subst hx; rw [hnew]; simp; exact this.1
      · rw [hheap x hx]; exact this
    nodupE := hi.nodupE }

end

end Rc
