import Rc.Lemmas

/-!
# The safety invariant

`Inv cfg s` is the conjunction every legal step preserves. Its main clauses:

* **holder safety** (`tgtOk`): every credit targets a live counted allocation of the
  credit's own generation;
* **borrower safety** (`bGuard`, `bParent`): every borrow is guarded by a held credit on the
  value it views, and by a live parent;
* **tracking** (`counts`): a finite header equals the ideal count (credits on the address)
  and lies in `1 .. max-1`; `Immortal` is a conservative live representation;
* **token linearity** (`tokOk`, `resetTok`, `rawTok`, `tokAddr`): raw storage and pending
  resets correspond one-to-one to tokens;
* **regions** (`regBlocks`, `regLive`, `lTgt`): region blocks are uncounted, live exactly
  while their region is open, and referenced only by locals;
* **acyclicity** (`srcOk`'s `born` clause): every edge points from a younger to an older
  block, so the value heap is a DAG.
-/

namespace Rc

def Kind.counted : Kind → Bool
  | .heap => true
  | .static => true
  | .region _ => false

def TokVal.addr : TokVal → Option Nat
  | .none => Option.none
  | .pending a => some a
  | .raw a _ _ => some a

def Event.key : Event → Nat × Nat
  | .finalized a g _ => (a, g)

def TokOk (s : State) (t : Token) : Prop :=
  match t.val with
  | .none => True
  | .pending a => (s.heap a).st = .dying ∧ (s.heap a).reset = some t.id
  | .raw a g cap => (s.heap a).st = .raw ∧ (s.heap a).gen = g ∧ (s.heap a).arity = cap

def ParentOk (s : State) (b : Borrow) : Prop :=
  match b.parent with
  | .none => ∃ r, b.guard = .root r
  | .borrow p => ∃ pb ∈ s.borrows, pb.id = p ∧ ∃ k, b.guard = .field pb.tgt k
  | .loc l => ∃ lc ∈ s.locals, lc.id = l ∧ ∃ k, b.guard = .field lc.tgt k

structure Inv (cfg : Config) (s : State) : Prop where
  frontier : ∀ a, s.next ≤ a → s.heap a = {}
  nodupH : (s.credits.map Credit.h).Nodup
  tgtOk : ∀ c ∈ s.credits,
    (s.heap c.tgt).st = .live ∧ (s.heap c.tgt).gen = c.gen ∧ (s.heap c.tgt).kind.counted = true
  srcOk : ∀ c ∈ s.credits, ∀ src k, c.h = .field src k →
    ((s.heap src).st = .live ∨ (s.heap src).st = .dying) ∧ k < (s.heap src).arity ∧
      (s.heap c.tgt).born < (s.heap src).born
  counts : ∀ a, (s.heap a).st = .live → (s.heap a).kind.counted = true →
    (s.heap a).cnt = .immortal ∨
      ∃ n, (s.heap a).cnt = .fin n ∧ 1 ≤ n ∧ n < cfg.max ∧ icount s a = n
  staticImm : ∀ a, (s.heap a).st = .live → (s.heap a).kind = .static → (s.heap a).cnt = .immortal
  finNotImm : ∀ a, (s.heap a).st = .live → (s.heap a).fin = true → (s.heap a).cnt ≠ .immortal
  nodupB : (s.borrows.map Borrow.id).Nodup
  bGuard : ∀ b ∈ s.borrows, ∃ c ∈ s.credits, c.h = b.guard ∧ c.tgt = b.tgt ∧ c.gen = b.gen
  bParent : ∀ b ∈ s.borrows, ParentOk s b
  nodupL : (s.locals.map Local.id).Nodup
  lTgt : ∀ l ∈ s.locals,
    (s.heap l.tgt).st = .live ∧ (s.heap l.tgt).gen = l.gen ∧ (s.heap l.tgt).kind.counted = false
  nodupW : s.work.Nodup
  workIff : ∀ a, a ∈ s.work ↔ (s.heap a).st = .dying
  dyingHeap : ∀ a, (s.heap a).st = .dying → (s.heap a).kind = .heap
  resetDying : ∀ a, (s.heap a).st ≠ .dying → (s.heap a).reset = none
  nodupT : (s.tokens.map Token.id).Nodup
  tokOk : ∀ t ∈ s.tokens, TokOk s t
  resetTok : ∀ a t, (s.heap a).st = .dying → (s.heap a).reset = some t →
    ∃ tk ∈ s.tokens, tk.id = t ∧ tk.val = .pending a
  rawTok : ∀ a, (s.heap a).st = .raw →
    ∃ tk ∈ s.tokens, tk.val = .raw a (s.heap a).gen (s.heap a).arity
  tokAddr : ∀ t ∈ s.tokens, ∀ u ∈ s.tokens, ∀ a, t.val.addr = some a → u.val.addr = some a → t = u
  nodupR : (s.regions.map Region.id).Nodup
  regBlocks : ∀ R ∈ s.regions, ∀ a ∈ R.blocks,
    (s.heap a).st = .live ∧ (s.heap a).kind = .region R.id
  regLive : ∀ a g, (s.heap a).kind = .region g →
    (s.heap a).st = .live ∧ ∃ R ∈ s.regions, R.id = g ∧ a ∈ R.blocks
  bornLt : ∀ a, (s.heap a).st ≠ .free → (s.heap a).born < s.clock
  finDoneLive : ∀ a, (s.heap a).st = .live → (s.heap a).finDone = false
  finDoneEv : ∀ a, (s.heap a).st = .dying → (s.heap a).finDone = true →
    ∃ ok, Event.finalized a (s.heap a).gen ok ∈ s.events
  evOk : ∀ a g ok, Event.finalized a g ok ∈ s.events → g ≤ (s.heap a).gen ∧
    (g = (s.heap a).gen → (s.heap a).st ≠ .live ∧
      ((s.heap a).st = .dying → (s.heap a).finDone = true))
  nodupE : (s.events.map Event.key).Nodup

/-! ## Elementary consequences -/

section
variable {cfg : Config} {s : State}

theorem Inv.rootOf_mem (_ : Inv cfg s) {r : Nat} {c : Credit} (h : rootOf s r = some c) :
    c ∈ s.credits ∧ c.h = .root r := by
  refine ⟨find?_mem h, ?_⟩
  have := find?_prop h
  simpa using this

theorem fieldOf_mem {s : State} {a k : Nat} {c : Credit} (h : fieldOf s a k = some c) :
    c ∈ s.credits ∧ c.h = .field a k := by
  refine ⟨find?_mem h, ?_⟩
  have := find?_prop h
  simpa using this

theorem borrowOf_mem {s : State} {id : Nat} {b : Borrow} (h : borrowOf s id = some b) :
    b ∈ s.borrows ∧ b.id = id := by
  refine ⟨find?_mem h, ?_⟩
  have := find?_prop h
  simpa using this

theorem localOf_mem {s : State} {id : Nat} {l : Local} (h : localOf s id = some l) :
    l ∈ s.locals ∧ l.id = id := by
  refine ⟨find?_mem h, ?_⟩
  have := find?_prop h
  simpa using this

theorem tokenOf_mem {s : State} {id : Nat} {t : Token} (h : tokenOf s id = some t) :
    t ∈ s.tokens ∧ t.id = id := by
  refine ⟨find?_mem h, ?_⟩
  have := find?_prop h
  simpa using this

theorem holderUsed_false {s : State} {h : Holder} (hu : holderUsed s h = false) :
    ∀ c ∈ s.credits, c.h ≠ h := by
  intro c hc he
  simp [holderUsed] at hu
  exact hu c hc he

theorem guarded_false {s : State} {h : Holder} (hg : guarded s h = false) :
    ∀ b ∈ s.borrows, b.guard ≠ h := by
  intro b hb he
  simp [guarded] at hg
  exact hg b hb he

theorem borrowOf_none {s : State} {id : Nat} (h : (borrowOf s id).isSome = false) :
    ∀ b ∈ s.borrows, b.id ≠ id := by
  intro b hb he
  simp [borrowOf] at h
  exact h b hb he

theorem localOf_none {s : State} {id : Nat} (h : (localOf s id).isSome = false) :
    ∀ l ∈ s.locals, l.id ≠ id := by
  intro l hl he
  simp [localOf] at h
  exact h l hl he

theorem tokenOf_none {s : State} {id : Nat} (h : (tokenOf s id).isSome = false) :
    ∀ t ∈ s.tokens, t.id ≠ id := by
  intro t ht he
  simp [tokenOf] at h
  exact h t ht he

/-- The ideal count after erasing one held credit. -/
theorem icount_erase {cs : List Credit} {c : Credit} (hc : c ∈ cs) (a : Nat) :
    (cs.erase c).countP (fun d => d.tgt == a) + (if c.tgt = a then 1 else 0) =
      cs.countP (fun d => d.tgt == a) := by
  have := countP_erase_add (fun d : Credit => d.tgt == a) cs c hc
  simpa using this

/-- A live counted block with finite count one has exactly one credit, the given one. -/
theorem Inv.only_credit (hi : Inv cfg s) {c d : Credit} (hc : c ∈ s.credits) (hd : d ∈ s.credits)
    (h1 : (s.heap c.tgt).cnt = .fin 1) (ht : d.tgt = c.tgt) : d = c := by
  have ⟨hl, _, hk⟩ := hi.tgtOk c hc
  rcases hi.counts c.tgt hl hk with h | ⟨n, hn, _, _, hic⟩
  · rw [h1] at h; cases h
  · rw [h1] at hn; cases hn
    by_cases hne : d = c
    · exact hne
    exfalso
    have e1 := icount_erase hc c.tgt
    have hd' : d ∈ s.credits.erase c := mem_erase_of_mem_ne hd hne
    have : 0 < (s.credits.erase c).countP (fun x => x.tgt == c.tgt) :=
      List.countP_pos_iff.mpr ⟨d, hd', by simp [ht]⟩
    simp [icount] at hic
    simp at e1
    omega

theorem Inv.credits_nodup (hi : Inv cfg s) : s.credits.Nodup := nodup_of_map hi.nodupH

theorem Inv.credit_eq_of_holder (hi : Inv cfg s) {c d : Credit} (hc : c ∈ s.credits)
    (hd : d ∈ s.credits) (h : c.h = d.h) : c = d :=
  eq_of_map_nodup hi.nodupH hc hd h

/-- No borrow is guarded by a field of a block that is not live, or of a region block that no
local references. -/
theorem Inv.no_guard_field (hi : Inv cfg s) {src k : Nat}
    (hsrc : (s.heap src).st ≠ .live ∨ (s.heap src).kind.counted = false)
    (hloc : ∀ l ∈ s.locals, l.tgt ≠ src) :
    ∀ b ∈ s.borrows, b.guard ≠ .field src k := by
  intro b hb hg
  have hp := hi.bParent b hb
  unfold ParentOk at hp
  split at hp
  · obtain ⟨r, hr⟩ := hp; rw [hr] at hg; cases hg
  · obtain ⟨pb, hpb, _, k', hk'⟩ := hp
    rw [hk'] at hg; cases hg
    obtain ⟨c, hc, _, hct, _⟩ := hi.bGuard pb hpb
    have ⟨hl, _, hk⟩ := hi.tgtOk c hc
    rw [hct] at hl hk
    rcases hsrc with h | h
    · exact h hl
    · rw [hk] at h; cases h
  · obtain ⟨lc, hlc, _, k', hk'⟩ := hp
    rw [hk'] at hg; cases hg
    exact hloc lc hlc rfl

end

end Rc
