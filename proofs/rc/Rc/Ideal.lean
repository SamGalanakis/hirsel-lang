import Rc.Steps

/-!
# The ideal machine and the simulation theorem

The ideal machine has no header counts and no reuse tokens. A value dies when its *ideal*
count (the number of credits on it) reaches zero, unless it is `pinned`: conservatively
retained, which is how the ideal semantics sees an immortal (saturated or static) block.
Pinning is the ideal machine's only nondeterminism; retaining memory is always safe.

`abs` erases from a concrete state exactly what the ideal machine does not have: header
counts (replaced by `pinned`), reuse tokens, pending-reset marks, and the contents of raw
storage (which the ideal machine sees as free storage of the same generation).
-/

namespace Rc

structure IBlock where
  st : BState := .free
  gen : Nat := 0
  kind : Kind := .heap
  arity : Nat := 0
  fin : Bool := false
  finDone : Bool := false
  pinned : Bool := false
  published : Bool := false
  born : Nat := 0
  deriving DecidableEq, Repr

@[ext] structure IState where
  heap : Nat → IBlock
  next : Nat
  clock : Nat
  credits : List Credit
  borrows : List Borrow
  locals : List Local
  work : List Nat
  regions : List Region
  events : List Event

def iupd (f : Nat → IBlock) (a : Nat) (b : IBlock) : Nat → IBlock :=
  fun x => if x = a then b else f x

def absBlock (b : Block) : IBlock :=
  match b.st with
  | .free => { gen := b.gen }
  | .raw => { gen := b.gen }
  | st => { st, gen := b.gen, kind := b.kind, arity := b.arity, fin := b.fin,
            finDone := b.finDone, pinned := b.cnt == .immortal, published := b.published,
            born := b.born }

def abs (s : State) : IState :=
  { heap := fun a => absBlock (s.heap a), next := s.next, clock := s.clock, credits := s.credits,
    borrows := s.borrows, locals := s.locals, work := s.work, regions := s.regions,
    events := s.events }

/-! ## Ideal transitions -/

/-- Ideal release: the value dies when no credit remains on it, unless it is pinned. -/
def irelease (i : IState) (c : Credit) : IState :=
  if (i.credits.erase c).countP (fun d => d.tgt == c.tgt) = 0 ∧ (i.heap c.tgt).pinned = false then
    { i with credits := i.credits.erase c,
             heap := iupd i.heap c.tgt { i.heap c.tgt with st := .dying },
             work := c.tgt :: i.work }
  else { i with credits := i.credits.erase c }

/-- Ideal allocation at a chosen free address `a`. -/
def ialloc (i : IState) (a r arity : Nat) (fin : Bool) : IState :=
  let g := (i.heap a).gen + 1
  { i with heap := iupd i.heap a { st := .live, gen := g, kind := .heap, arity, fin, born := i.clock },
           next := max i.next (a + 1),
           clock := i.clock + 1,
           credits := ⟨.root r, a, g⟩ :: i.credits }

/-- Ideal share: one more credit; the value may additionally be pinned. -/
def idup (i : IState) (c : Credit) (r : Nat) (pin : Bool) : IState :=
  { i with credits := ⟨.root r, c.tgt, c.gen⟩ :: i.credits,
           heap := iupd i.heap c.tgt { i.heap c.tgt with pinned := (i.heap c.tgt).pinned || pin } }

def ifreeBlocks (i : IState) (blocks : List Nat) (rest : List Region) : IState :=
  { i with regions := rest,
           heap := fun x => if x ∈ blocks then { gen := (i.heap x).gen } else i.heap x }

/-- Ideal region close: release each outgoing credit once, then free the region's blocks. -/
def iregionClose (i : IState) (R : Region) (rest : List Region) : IState :=
  ifreeBlocks ((outgoing i.credits R.blocks).foldl irelease i) R.blocks rest

/-- The ideal steps the storage protocol must simulate. -/
inductive IStep : IState → IState → Prop
  | alloc {i : IState} {a r arity : Nat} {fin : Bool} :
      (i.heap a).st = .free → (∀ c ∈ i.credits, c.h ≠ .root r) →
      IStep i (ialloc i a r arity fin)
  | dup {i : IState} {c : Credit} {r : Nat} (pin : Bool) :
      c ∈ i.credits → (∀ d ∈ i.credits, d.h ≠ .root r) → (i.heap c.tgt).fin = false →
      IStep i (idup i c r pin)
  | release {i : IState} {c : Credit} :
      c ∈ i.credits → (∀ b ∈ i.borrows, b.guard ≠ c.h) → IStep i (irelease i c)
  | regionClose {i : IState} {R : Region} {rest : List Region} :
      i.regions = R :: rest → (∀ l ∈ i.locals, l.tgt ∉ R.blocks) →
      IStep i (iregionClose i R rest)

/-! ## Correspondence lemmas -/

section
variable {cfg : Config} {s : State}

theorem absBlock_live {b : Block} (h : b.st = .live) :
    absBlock b = { st := .live, gen := b.gen, kind := b.kind, arity := b.arity, fin := b.fin,
                   finDone := b.finDone, pinned := b.cnt == .immortal, published := b.published,
                   born := b.born } := by
  simp [absBlock, h]

theorem absBlock_gen (b : Block) : (absBlock b).gen = b.gen := by
  unfold absBlock; split <;> rfl

theorem abs_upd (h : Nat → Block) (a : Nat) (b : Block) :
    (fun x => absBlock (upd h a b x)) = iupd (fun x => absBlock (h x)) a (absBlock b) := by
  funext x; by_cases hx : x = a <;> simp [upd, iupd, hx]

/-- **The decision correspondence**: the header's release decision is the ideal one. -/
theorem Inv.abs_release (hi : Inv cfg s) {c : Credit} (hc : c ∈ s.credits) :
    abs (Rc.release s c) = irelease (abs s) c := by
  have ⟨hl, _, hk⟩ := hi.tgtOk c hc
  have e := icount_erase hc c.tgt
  simp only [ite_true] at e
  rcases hi.counts c.tgt hl hk with himm | ⟨n, hn, h1, _, hic⟩
  · -- immortal: the ideal value is pinned and is retained
    have hrel : Rc.release s c = { s with credits := s.credits.erase c } := by
      simp [Rc.release, himm]
    rw [hrel]
    have hpin : ((abs s).heap c.tgt).pinned = true := by
      simp [abs, absBlock_live hl, himm]
    simp only [irelease, hpin]
    simp [abs]
  · simp only [icount] at hic
    by_cases hn1 : n ≤ 1
    · have hrel : Rc.release s c =
          { s with credits := s.credits.erase c,
                   heap := upd s.heap c.tgt { s.heap c.tgt with st := .dying, cnt := .dead },
                   work := c.tgt :: s.work } := by
        simp [Rc.release, hn, hn1]
      rw [hrel]
      have hpin : ((abs s).heap c.tgt).pinned = false := by
        simp [abs, absBlock_live hl, hn]
      have hz : ((abs s).credits.erase c).countP (fun d => d.tgt == c.tgt) = 0 := by
        simp only [abs]; omega
      simp only [irelease, hz, hpin, and_self, ite_true]
      ext x
      · simp only [abs]
        by_cases hx : x = c.tgt
        · subst hx; simp [upd, iupd, absBlock, hl]
        · simp [upd, iupd, hx]
      all_goals rfl
    · have hrel : Rc.release s c =
          { s with credits := s.credits.erase c,
                   heap := upd s.heap c.tgt { s.heap c.tgt with cnt := .fin (n - 1) } } := by
        simp [Rc.release, hn, hn1]
      rw [hrel]
      have hnz : ¬ ((abs s).credits.erase c).countP (fun d => d.tgt == c.tgt) = 0 := by
        simp only [abs]; omega
      simp only [irelease, hnz, false_and, ite_false]
      ext x
      · simp only [abs]
        by_cases hx : x = c.tgt
        · subst hx; simp [upd, absBlock, hl, hn]; rfl
        · simp [upd, hx]
      all_goals rfl

end

end Rc

namespace Rc

section
variable {cfg : Config} {s s' : State}

theorem abs_installCounted (s : State) (a r arity : Nat) (fin : Bool) :
    abs (installCounted s a r (heapBlock arity fin)) = ialloc (abs s) a r arity fin := by
  ext x
  · simp only [abs, installCounted, ialloc, absBlock_gen]
    by_cases hx : x = a
    · subst hx; simp [upd, iupd, absBlock, heapBlock]
    · simp [upd, iupd, hx]
  all_goals simp [abs, installCounted, ialloc, absBlock_gen]

theorem abs_share (hl : (s.heap c.tgt).st = .live) (r : Nat) :
    abs (share cfg s c.tgt c.gen r) =
      idup (abs s) c r ((s.heap c.tgt).cnt.incr cfg == .immortal) := by
  ext x
  · simp only [abs, share, idup]
    by_cases hx : x = c.tgt
    · rw [hx]
      simp only [upd, iupd, ite_true, absBlock, hl]
      congr 1
      cases hcnt : (s.heap c.tgt).cnt with
      | fin n => simp only [Count.incr]; split <;> rfl
      | dead => rfl
      | immortal => rfl
    · simp [upd, iupd, hx]
  all_goals simp [abs, share, idup]

theorem abs_markReset (s : State) (a t : Nat) : abs (markReset s a t) = abs s := by
  ext x
  · simp only [abs, markReset]
    by_cases hx : x = a
    · subst hx; simp only [upd, ite_true]; unfold absBlock; simp
    · simp [upd, hx]
  all_goals rfl

theorem abs_tokens (s : State) (ts : List Token) : abs { s with tokens := ts } = abs s := rfl

theorem abs_freeRaw (s : State) (tok : Token) {a : Nat} (hraw : (s.heap a).st = .raw) :
    abs (freeRaw s tok a) = abs s := by
  ext x
  · simp only [abs, freeRaw]
    by_cases hx : x = a
    · subst hx; simp [upd, absBlock, hraw]
    · simp [upd, hx]
  all_goals rfl

theorem absBlock_free {b : Block} (h : b.st = .free) : absBlock b = { gen := b.gen } := by
  simp [absBlock, h]

theorem abs_freeBlocks (s : State) (blocks : List Nat) (rest : List Region) :
    abs (freeBlocks s blocks rest) = ifreeBlocks (abs s) blocks rest := by
  ext x
  · simp only [abs, freeBlocks, ifreeBlocks]
    by_cases hx : x ∈ blocks
    · simp only [hx, ite_true]; rw [absBlock_free rfl, absBlock_gen]
    · simp [hx]
  all_goals rfl

theorem Inv.abs_releaseAll :
    ∀ (l : List Credit) {s : State}, Inv cfg s → l.Nodup → (∀ c ∈ l, c ∈ s.credits) →
      (∀ c ∈ l, ∀ b ∈ s.borrows, b.guard ≠ c.h) →
      abs (l.foldl Rc.release s) = l.foldl irelease (abs s)
  | [], _, _, _, _, _ => rfl
  | c :: l, s, hi, hn, hm, hg => by
    simp only [List.foldl_cons]
    rw [List.nodup_cons] at hn
    rw [← hi.abs_release (hm c List.mem_cons_self)]
    apply Inv.abs_releaseAll l (hi.release (hm c List.mem_cons_self) (hg c List.mem_cons_self)) hn.2
    · intro d hd
      rw [release_credits]
      exact mem_erase_of_mem_ne (hm d (List.mem_cons_of_mem _ hd)) (fun he => hn.1 (he ▸ hd))
    · intro d hd b hb
      rw [release_borrows] at hb
      exact hg d (List.mem_cons_of_mem _ hd) b hb

/-- Operations of the storage protocol the simulation theorem covers. -/
def Op.storage : Op → Bool
  | .alloc .. | .dup .. | .drop _ | .reset .. | .reuse .. | .dispose _ | .regionClose _ => true
  | _ => false

/-- **Simulation.** Every legal alloc, dup, drop, reset, reuse, dispose and region-close step
of the concrete machine is an ideal step of the abstracted state, or a silent storage step
that leaves the abstraction unchanged. -/
theorem simulation (hi : Inv cfg s) {op : Op} (hop : op.storage = true)
    (h : step cfg s op = .ok s') : IStep (abs s) (abs s') ∨ abs s' = abs s := by
  cases op with
  | alloc r arity fin =>
    simp only [step, opAlloc] at h
    split at h
    · cases h
    · rename_i hu; cases h
      left
      have hf := (freeAddr_spec hi).1
      rw [allocHeap, abs_installCounted]
      exact IStep.alloc (by simp [abs, absBlock, hf]) (holderUsed_false (by simpa using hu))
  | dup src dst =>
    simp only [step, opDup] at h
    split at h
    · cases h
    · rename_i c hc
      split at h
      · cases h
      · rename_i hu
        split at h
        · cases h
        · rename_i hf; cases h
          left
          obtain ⟨hcm, _⟩ := hi.rootOf_mem hc
          have hl := (hi.tgtOk c hcm).1
          rw [abs_share hl]
          exact IStep.dup _ hcm (holderUsed_false (by simpa using hu))
            (by simp [abs, absBlock, hl]; simpa using hf)
  | drop r =>
    simp only [step, opDrop] at h
    split at h
    · cases h
    · rename_i c hc
      split at h
      · cases h
      · rename_i hg; cases h
        left
        obtain ⟨hcm, hch⟩ := hi.rootOf_mem hc
        rw [hi.abs_release hcm]
        exact IStep.release hcm (by rw [hch]; exact guarded_false (by simpa using hg))
  | reset b t =>
    simp only [step, opReset] at h
    split at h
    · cases h
    · rename_i cb hcb
      obtain ⟨hcbm, hcbh⟩ := hi.rootOf_mem hcb
      split at h
      · cases h
      · rename_i hg
        have hg' : ∀ x ∈ (abs s).borrows, x.guard ≠ cb.h := by
          rw [hcbh]; exact guarded_false (by simpa using hg)
        split at h
        · cases h
        · split at h
          · cases h; left
            rw [abs_markReset, hi.abs_release hcbm]
            exact IStep.release hcbm hg'
          · cases h; left
            show IStep (abs s) (abs (Rc.release s cb))
            rw [hi.abs_release hcbm]
            exact IStep.release hcbm hg'
  | reuse t r arity fin =>
    simp only [step, opReuse] at h
    split at h
    · cases h
    · rename_i tok htok
      obtain ⟨htm, _⟩ := tokenOf_mem htok
      split at h
      · cases h
      · rename_i hu
        have hu := holderUsed_false (by simpa using hu)
        split at h
        · cases h; left
          have hf : (s.heap (freeAddr { s with tokens := s.tokens.erase tok })).st = .free :=
            (freeAddr_spec hi).1
          rw [allocHeap, abs_installCounted, abs_tokens]
          exact IStep.alloc (by simp only [abs]; rw [absBlock_free hf]) hu
        · cases h
        · rename_i a g cap hv
          split at h
          · cases h
          · cases h; left
            have htk := hi.tokOk tok htm
            unfold TokOk at htk; rw [hv] at htk
            rw [abs_installCounted, abs_freeRaw s tok htk.1]
            exact IStep.alloc (by simp [abs, absBlock, htk.1]) hu
  | dispose t =>
    simp only [step, opDispose] at h
    split at h
    · cases h
    · rename_i tok htok
      obtain ⟨htm, _⟩ := tokenOf_mem htok
      split at h
      · cases h; right; rfl
      · cases h
      · rename_i a g cap hv; cases h; right
        have htk := hi.tokOk tok htm
        unfold TokOk at htk; rw [hv] at htk
        exact abs_freeRaw s tok htk.1
  | regionClose g =>
    simp only [step, opRegionClose] at h
    split at h
    · cases h
    · rename_i R rest hrs
      split at h
      · cases h
      · split at h
        · cases h
        · rename_i hloc; cases h; left
          have hR : R ∈ s.regions := by rw [hrs]; exact List.mem_cons_self
          have hloc' : ∀ l ∈ s.locals, l.tgt ∉ R.blocks := by
            intro l hl hm; simp at hloc; exact hloc l hl hm
          have hout_guard : ∀ c ∈ outgoing s.credits R.blocks, ∀ b ∈ s.borrows, b.guard ≠ c.h := by
            intro c hc b hb
            obtain ⟨_, src, k, hh, hs⟩ := mem_outgoing.mp hc
            rw [hh]
            have hk := (hi.regBlocks R hR src hs).2
            refine hi.no_guard_field (Or.inr (by rw [hk]; rfl)) ?_ b hb
            intro l hl he; exact hloc' l hl (he ▸ hs)
          have hnd : (outgoing s.credits R.blocks).Nodup := hi.credits_nodup.filter _
          rw [abs_freeBlocks, hi.abs_releaseAll (outgoing s.credits R.blocks) hnd
            (fun c hc => (mem_outgoing.mp hc).1) hout_guard]
          exact IStep.regionClose hrs hloc'
  | _ => simp [Op.storage] at hop

end

end Rc
