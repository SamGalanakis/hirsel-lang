import Rc.Basic

/-!
# The concrete machine

`step` is both the transition system the theorems are about and the evaluator behind
`rc-trace`. Every operation carries a stable transition ID (`T01` ...) that runtime primitives
cite in comments and tests. An illegal operation is an
error, never a silent no-op: the protocol it checks is the contract J and H must also enforce.
-/

namespace Rc

abbrev Res := Except String State

/-- Operations of the sequential protocol. Identifiers are chosen by the trace and must be
fresh where an operation introduces them. -/
inductive Op where
  /-- T01: allocate a counted heap block with `arity` pointer fields, held by new root `r`. -/
  | alloc (r arity : Nat) (fin : Bool)
  /-- T02: a static immortal constant block held by new root `r` (static exemption). -/
  | static (r : Nat)
  /-- T03: share the value of root `src` into new root `dst` (count increment). -/
  | dup (src dst : Nat)
  /-- T04: release root `r`'s credit (last-use or scope-end release). -/
  | drop (r : Nat)
  /-- T05: in-place write of root `r`'s credit into field `k` of the unique block held by `b`. -/
  | write (b k r : Nat)
  /-- T06: move field `k` out of the unique block held by `b` into new root `r` (partial move). -/
  | take (b k r : Nat)
  /-- T07: owned read of field `k` through borrow `p` into new root `r` (counted projection). -/
  | load (p k r : Nat)
  /-- T08: borrow `id` viewing root `r`'s value. -/
  | borrow (id r : Nat)
  /-- T09: borrow `id` viewing field `k` of borrow `p`'s value. -/
  | borrowField (id p k : Nat)
  /-- T10: borrow `id` viewing heap field `k` of the region block referenced by local `l`. -/
  | borrowLocal (id l k : Nat)
  /-- T11: end borrow `id` (its last use). -/
  | endBorrow (id : Nat)
  /-- T12: reset root `b`'s block for reuse, producing token `t` (pending or none). -/
  | reset (b t : Nat)
  /-- T13: consume token `t` to install a new value held by new root `r`. -/
  | reuse (t r arity : Nat) (fin : Bool)
  /-- T14: consume token `t`, shallow-freeing raw storage. -/
  | dispose (t : Nat)
  /-- T15: run the due finalizer of the worklist head with outcome `ok`. -/
  | finalize (ok : Bool)
  /-- T16: one dead-worklist step: release the head's last initialized field, or retire it. -/
  | drain
  /-- T17: explicit forget: move root `r`'s credit to the permanent forget ledger. -/
  | forget (r : Nat)
  /-- T18: open compiler-local region `g`. -/
  | regionOpen (g : Nat)
  /-- T19: allocate an uncounted block in open region `g`, referenced by new local `l`. -/
  | regionAlloc (g l arity : Nat)
  /-- T20: store root `r`'s heap credit into field `k` of unpublished region block `l`. -/
  | regionStore (l k r : Nat)
  /-- T21: copy local reference `l` into new local `l'`, publishing the block. -/
  | localCopy (l l' : Nat)
  /-- T22: end local reference `l`. -/
  | localDrop (l : Nat)
  /-- T23: close innermost region `g`, releasing its outgoing heap credits once each. -/
  | regionClose (g : Nat)
  deriving DecidableEq, Repr

/-! ## Lookups -/

def rootOf (s : State) (r : Nat) : Option Credit := s.credits.find? (fun c => c.h == .root r)
def fieldOf (s : State) (a k : Nat) : Option Credit := s.credits.find? (fun c => c.h == .field a k)
def holderUsed (s : State) (h : Holder) : Bool := s.credits.any (fun c => c.h == h)
def guarded (s : State) (h : Holder) : Bool := s.borrows.any (fun b => b.guard == h)
def borrowOf (s : State) (id : Nat) : Option Borrow := s.borrows.find? (fun b => b.id == id)
def localOf (s : State) (id : Nat) : Option Local := s.locals.find? (fun l => l.id == id)
def tokenOf (s : State) (id : Nat) : Option Token := s.tokens.find? (fun t => t.id == id)
def regionUsed (s : State) (g : Nat) : Bool := s.regions.any (fun R => R.id == g)

/-- First free address below the frontier, else the frontier. -/
def freeAddr (s : State) : Nat :=
  ((List.range s.next).find? (fun a => (s.heap a).st == .free)).getD s.next

/-- The initialized pointer fields of block `a`. -/
def fieldsOf (s : State) (a : Nat) : List Credit := s.credits.filter (fun c => c.h.isFieldOf a)

/-- The field credit with the highest field index (reverse declaration order, DROP-2). -/
def pickMax : List Credit → Option Credit
  | [] => none
  | c :: cs =>
    match pickMax cs with
    | none => some c
    | some d => if d.h.idx ≤ c.h.idx then some c else some d

/-! ## Shared transitions -/

/-- Release one credit. The header decides: a finite count of one dies onto the worklist,
a larger finite count decrements, `Immortal` is sticky. -/
def release (s : State) (c : Credit) : State :=
  match (s.heap c.tgt).cnt with
  | .fin n =>
    if n ≤ 1 then
      { s with credits := s.credits.erase c,
               heap := upd s.heap c.tgt { s.heap c.tgt with st := .dying, cnt := .dead },
               work := c.tgt :: s.work }
    else
      { s with credits := s.credits.erase c,
               heap := upd s.heap c.tgt { s.heap c.tgt with cnt := .fin (n - 1) } }
  | _ => { s with credits := s.credits.erase c }

/-- Add a root credit `r` on `tgt` and increment its header (saturating). -/
def share (cfg : Config) (s : State) (tgt gen r : Nat) : State :=
  { s with credits := ⟨.root r, tgt, gen⟩ :: s.credits,
           heap := upd s.heap tgt { s.heap tgt with cnt := (s.heap tgt).cnt.incr cfg } }

/-- Install a counted value block `b` at free address `a`, held by new root `r`. The
generation is bumped, so a stale handle to the previous occupant can never match. -/
def installCounted (s : State) (a r : Nat) (b : Block) : State :=
  let g := (s.heap a).gen + 1
  { s with heap := upd s.heap a { b with gen := g, born := s.clock },
           next := max s.next (a + 1),
           clock := s.clock + 1,
           credits := ⟨.root r, a, g⟩ :: s.credits }

def heapBlock (arity : Nat) (fin : Bool) : Block :=
  { st := .live, cnt := .fin 1, kind := .heap, arity, fin }

def staticBlock : Block := { st := .live, cnt := .immortal, kind := .static }

def allocHeap (s : State) (r arity : Nat) (fin : Bool) : State :=
  installCounted s (freeAddr s) r (heapBlock arity fin)

/-- Install an uncounted block of region `g` at free address `a`, referenced by new local `l`. -/
def installRegion (s : State) (a g l arity : Nat) : State :=
  let gen := (s.heap a).gen + 1
  { s with heap := upd s.heap a { st := .live, gen, kind := .region g, arity, born := s.clock },
           next := max s.next (a + 1),
           clock := s.clock + 1,
           locals := ⟨l, a, gen⟩ :: s.locals,
           regions := s.regions.map (fun R => if R.id = g then { R with blocks := a :: R.blocks } else R) }

/-- Consume token `tok` and shallow-free its raw storage `a` (no scan, no finalizer). -/
def freeRaw (s : State) (tok : Token) (a : Nat) : State :=
  { s with tokens := s.tokens.erase tok, heap := upd s.heap a { gen := (s.heap a).gen } }

/-- Move a credit to a new holder (a transfer: the ideal count is unchanged). -/
def relabel (s : State) (c : Credit) (h : Holder) : State :=
  { s with credits := { c with h := h } :: s.credits.erase c }

/-- Ghost: give block `a` the newest stamp before it gains an edge to a younger block. -/
def restamp (s : State) (a : Nat) : State :=
  { s with heap := upd s.heap a { s.heap a with born := s.clock }, clock := s.clock + 1 }

/-- Mark a region block published: it is visible to another binding and never mutated again. -/
def publish (s : State) (a : Nat) : State :=
  { s with heap := upd s.heap a { s.heap a with published := true } }

/-- A reset whose donor died: the dying block will become raw storage for token `t`. -/
def markReset (s : State) (a t : Nat) : State :=
  { s with heap := upd s.heap a { s.heap a with reset := some t },
           tokens := ⟨t, .pending a⟩ :: s.tokens }

/-- Release the credit field `k` of block `a` currently holds, if any (the displaced child). -/
def displace (s : State) (a k : Nat) : State :=
  match fieldOf s a k with
  | none => s
  | some o => release s o

def addBorrow (s : State) (b : Borrow) : State := { s with borrows := b :: s.borrows }

def err (msg : String) : Res := .error msg

/-! ## Operations -/

def opAlloc (s : State) (r arity : Nat) (fin : Bool) : Res :=
  if holderUsed s (.root r) then err s!"T01 alloc: root r{r} already holds a value"
  else .ok (allocHeap s r arity fin)

def opStatic (s : State) (r : Nat) : Res :=
  if holderUsed s (.root r) then err s!"T02 static: root r{r} already holds a value"
  else
    .ok (installCounted s (freeAddr s) r staticBlock)

def opDup (cfg : Config) (s : State) (src dst : Nat) : Res :=
  match rootOf s src with
  | none => err s!"T03 dup: root r{src} holds no value"
  | some c =>
    if holderUsed s (.root dst) then err s!"T03 dup: root r{dst} already holds a value"
    else if (s.heap c.tgt).fin then err s!"T03 dup: @{c.tgt} is a resource (noncopyable)"
    else .ok (share cfg s c.tgt c.gen dst)

def opDrop (s : State) (r : Nat) : Res :=
  match rootOf s r with
  | none => err s!"T04 release: root r{r} holds no value"
  | some c =>
    if guarded s (.root r) then err s!"T04 release: root r{r} still guards a live borrow"
    else .ok (release s c)

def opWrite (s : State) (b k r : Nat) : Res :=
  match rootOf s b with
  | none => err s!"T05 write: root r{b} holds no value"
  | some cb =>
    if (s.heap cb.tgt).cnt ≠ .fin 1 then err s!"T05 write: @{cb.tgt} is not unique"
    else if (s.heap cb.tgt).arity ≤ k then err s!"T05 write: @{cb.tgt} has no field {k}"
    else if guarded s (.root b) then err s!"T05 write: root r{b} is borrowed"
    else if r = b then err s!"T05 write: a block cannot hold itself"
    else match rootOf s r with
      | none => err s!"T05 write: root r{r} holds no value"
      | some cr =>
        if guarded s (.root r) then err s!"T05 write: root r{r} is borrowed"
        else
          .ok (relabel (restamp (displace s cb.tgt k) cb.tgt) cr (.field cb.tgt k))

def opTake (s : State) (b k r : Nat) : Res :=
  match rootOf s b with
  | none => err s!"T06 take: root r{b} holds no value"
  | some cb =>
    if (s.heap cb.tgt).cnt ≠ .fin 1 then err s!"T06 take: @{cb.tgt} is not unique"
    else if guarded s (.root b) then err s!"T06 take: root r{b} is borrowed"
    else if holderUsed s (.root r) then err s!"T06 take: root r{r} already holds a value"
    else match fieldOf s cb.tgt k with
      | none => err s!"T06 take: field {k} of @{cb.tgt} is not initialized"
      | some o => .ok (relabel s o (.root r))

def opLoad (cfg : Config) (s : State) (p k r : Nat) : Res :=
  match borrowOf s p with
  | none => err s!"T07 load: borrow b{p} is not live"
  | some bp =>
    match fieldOf s bp.tgt k with
    | none => err s!"T07 load: field {k} of @{bp.tgt} is not initialized"
    | some o =>
      if holderUsed s (.root r) then err s!"T07 load: root r{r} already holds a value"
      else if (s.heap o.tgt).fin then err s!"T07 load: @{o.tgt} is a resource (noncopyable)"
      else .ok (share cfg s o.tgt o.gen r)

def opBorrow (s : State) (id r : Nat) : Res :=
  if (borrowOf s id).isSome then err s!"T08 borrow: borrow b{id} is already live"
  else match rootOf s r with
    | none => err s!"T08 borrow: root r{r} holds no value"
    | some c => .ok (addBorrow s ⟨id, .root r, .none, c.tgt, c.gen⟩)

def opBorrowField (s : State) (id p k : Nat) : Res :=
  if (borrowOf s id).isSome then err s!"T09 borrow-field: borrow b{id} is already live"
  else match borrowOf s p with
    | none => err s!"T09 borrow-field: borrow b{p} is not live"
    | some bp =>
      match fieldOf s bp.tgt k with
      | none => err s!"T09 borrow-field: field {k} of @{bp.tgt} is not initialized"
      | some o => .ok (addBorrow s ⟨id, o.h, .borrow p, o.tgt, o.gen⟩)

def opBorrowLocal (s : State) (id l k : Nat) : Res :=
  if (borrowOf s id).isSome then err s!"T10 borrow-local: borrow b{id} is already live"
  else match localOf s l with
    | none => err s!"T10 borrow-local: local l{l} is not live"
    | some lc =>
      match fieldOf s lc.tgt k with
      | none => err s!"T10 borrow-local: field {k} of @{lc.tgt} is not initialized"
      | some o => .ok (addBorrow (publish s lc.tgt) ⟨id, o.h, .loc l, o.tgt, o.gen⟩)

def opEndBorrow (s : State) (id : Nat) : Res :=
  match borrowOf s id with
  | none => err s!"T11 end-borrow: borrow b{id} is not live"
  | some bb =>
    if s.borrows.any (fun b => b.parent == .borrow id) then
      err s!"T11 end-borrow: borrow b{id} still has a live sub-borrow"
    else .ok { s with borrows := s.borrows.erase bb }

def opReset (s : State) (b t : Nat) : Res :=
  match rootOf s b with
  | none => err s!"T12 reset: root r{b} holds no value"
  | some cb =>
    if guarded s (.root b) then err s!"T12 reset: root r{b} still guards a live borrow"
    else if (tokenOf s t).isSome then err s!"T12 reset: token t{t} already exists"
    else
      let s1 := release s cb
      if (s.heap cb.tgt).cnt = .fin 1 then .ok (markReset s1 cb.tgt t)
      else .ok { s1 with tokens := ⟨t, .none⟩ :: s1.tokens }

def opReuse (s : State) (t r arity : Nat) (fin : Bool) : Res :=
  match tokenOf s t with
  | none => err s!"T13 reuse: token t{t} does not exist"
  | some tok =>
    if holderUsed s (.root r) then err s!"T13 reuse: root r{r} already holds a value"
    else match tok.val with
      | .none => .ok (allocHeap { s with tokens := s.tokens.erase tok } r arity fin)
      | .pending a => err s!"T13 reuse: donor @{a} of token t{t} has not finished draining"
      | .raw a _ cap =>
        if cap ≠ arity then err s!"T13 reuse: token t{t} has capacity {cap}, not {arity}"
        else .ok (installCounted (freeRaw s tok a) a r (heapBlock arity fin))

def opDispose (s : State) (t : Nat) : Res :=
  match tokenOf s t with
  | none => err s!"T14 dispose: token t{t} does not exist"
  | some tok =>
    match tok.val with
    | .none => .ok { s with tokens := s.tokens.erase tok }
    | .pending a => err s!"T14 dispose: donor @{a} of token t{t} has not finished draining"
    | .raw a _ _ => .ok (freeRaw s tok a)

/-- Replace the value of token `t`. -/
def setTok (ts : List Token) (t : Nat) (v : TokVal) : List Token :=
  ts.map (fun tk => if tk.id = t then { tk with val := v } else tk)

/-- Consume the finalizer obligation of block `a` with outcome `ok`. A failure cancels a
pending reset: the storage is disposed and the token becomes `none`. -/
def runFinalizer (s : State) (a : Nat) (ok : Bool) : State :=
  { s with heap := upd s.heap a { s.heap a with finDone := true,
                                                reset := if ok then (s.heap a).reset else none },
           events := .finalized a (s.heap a).gen ok :: s.events,
           tokens := if ok then s.tokens else
             match (s.heap a).reset with
             | none => s.tokens
             | some t => setTok s.tokens t .none }

/-- Retire a fully drained dead block: free its storage, or hand it to its pending token. -/
def retireFree (s : State) (a : Nat) (rest : List Nat) : State :=
  { s with work := rest, heap := upd s.heap a { gen := (s.heap a).gen } }

def retireRaw (s : State) (a t : Nat) (rest : List Nat) : State :=
  { s with work := rest,
           heap := upd s.heap a { st := .raw, gen := (s.heap a).gen, arity := (s.heap a).arity },
           tokens := setTok s.tokens t (.raw a (s.heap a).gen (s.heap a).arity) }

def opFinalize (s : State) (ok : Bool) : Res :=
  match s.work with
  | [] => err "T15 finalize: the worklist is empty"
  | a :: _ =>
    if !((s.heap a).fin && !(s.heap a).finDone) then err s!"T15 finalize: @{a} has no finalizer due"
    else .ok (runFinalizer s a ok)

def opDrain (s : State) : Res :=
  match s.work with
  | [] => err "T16 drain: the worklist is empty"
  | a :: rest =>
    if (s.heap a).fin && !(s.heap a).finDone then err s!"T16 drain: @{a} has a finalizer due"
    else match pickMax (fieldsOf s a) with
      | some c => .ok (release s c)
      | none =>
        match (s.heap a).reset with
        | none => .ok (retireFree s a rest)
        | some t => .ok (retireRaw s a t rest)

def opForget (s : State) (r : Nat) : Res :=
  match rootOf s r with
  | none => err s!"T17 forget: root r{r} holds no value"
  | some c =>
    if guarded s (.root r) then err s!"T17 forget: root r{r} still guards a live borrow"
    else if holderUsed s (.forgot r) then err s!"T17 forget: forget ledger entry {r} is in use"
    else .ok (relabel s c (.forgot r))

def opRegionOpen (s : State) (g : Nat) : Res :=
  if regionUsed s g then err s!"T18 region-open: region g{g} is already open"
  else .ok { s with regions := ⟨g, []⟩ :: s.regions }

def opRegionAlloc (s : State) (g l arity : Nat) : Res :=
  if !regionUsed s g then err s!"T19 region-alloc: region g{g} is not open"
  else if (localOf s l).isSome then err s!"T19 region-alloc: local l{l} is already live"
  else
    .ok (installRegion s (freeAddr s) g l arity)

def opRegionStore (s : State) (l k r : Nat) : Res :=
  match localOf s l with
  | none => err s!"T20 region-store: local l{l} is not live"
  | some lc =>
    if (s.heap lc.tgt).published then err s!"T20 region-store: @{lc.tgt} is published"
    else if (s.heap lc.tgt).arity ≤ k then err s!"T20 region-store: @{lc.tgt} has no field {k}"
    else if (fieldOf s lc.tgt k).isSome then err s!"T20 region-store: field {k} of @{lc.tgt} is already initialized"
    else match rootOf s r with
      | none => err s!"T20 region-store: root r{r} holds no value"
      | some cr =>
        if guarded s (.root r) then err s!"T20 region-store: root r{r} is borrowed"
        else .ok (relabel (restamp s lc.tgt) cr (.field lc.tgt k))

def opLocalCopy (s : State) (l l' : Nat) : Res :=
  match localOf s l with
  | none => err s!"T21 local-copy: local l{l} is not live"
  | some lc =>
    if (localOf s l').isSome then err s!"T21 local-copy: local l{l'} is already live"
    else .ok { publish s lc.tgt with locals := ⟨l', lc.tgt, lc.gen⟩ :: s.locals }

def opLocalDrop (s : State) (l : Nat) : Res :=
  match localOf s l with
  | none => err s!"T22 local-drop: local l{l} is not live"
  | some lc =>
    if s.borrows.any (fun b => b.parent == .loc l) then
      err s!"T22 local-drop: local l{l} still has a live borrow"
    else .ok { s with locals := s.locals.erase lc }

/-- The outgoing heap credits of a set of region blocks. -/
def outgoing (cs : List Credit) (blocks : List Nat) : List Credit :=
  cs.filter (fun c => match c.h with
    | .field src _ => decide (src ∈ blocks)
    | _ => false)

/-- Reset a closed region's storage and pop it. -/
def freeBlocks (s : State) (blocks : List Nat) (rest : List Region) : State :=
  { s with regions := rest,
           heap := fun x => if x ∈ blocks then { gen := (s.heap x).gen } else s.heap x }

def opRegionClose (s : State) (g : Nat) : Res :=
  match s.regions with
  | [] => err s!"T23 region-close: no region is open"
  | R :: rest =>
    if R.id ≠ g then err s!"T23 region-close: region g{g} is not the innermost open region"
    else if s.locals.any (fun l => decide (l.tgt ∈ R.blocks)) then
      err s!"T23 region-close: a local reference into region g{g} is still live"
    else
      .ok (freeBlocks ((outgoing s.credits R.blocks).foldl release s) R.blocks rest)

def step (cfg : Config) (s : State) : Op → Res
  | .alloc r arity fin => opAlloc s r arity fin
  | .static r => opStatic s r
  | .dup src dst => opDup cfg s src dst
  | .drop r => opDrop s r
  | .write b k r => opWrite s b k r
  | .take b k r => opTake s b k r
  | .load p k r => opLoad cfg s p k r
  | .borrow id r => opBorrow s id r
  | .borrowField id p k => opBorrowField s id p k
  | .borrowLocal id l k => opBorrowLocal s id l k
  | .endBorrow id => opEndBorrow s id
  | .reset b t => opReset s b t
  | .reuse t r arity fin => opReuse s t r arity fin
  | .dispose t => opDispose s t
  | .finalize ok => opFinalize s ok
  | .drain => opDrain s
  | .forget r => opForget s r
  | .regionOpen g => opRegionOpen s g
  | .regionAlloc g l arity => opRegionAlloc s g l arity
  | .regionStore l k r => opRegionStore s l k r
  | .localCopy l l' => opLocalCopy s l l'
  | .localDrop l => opLocalDrop s l
  | .regionClose g => opRegionClose s g

/-- Run a trace; on failure report the index of the failing operation and the state before it. -/
def run (cfg : Config) : State → List Op → Nat → Except (Nat × String × State) State
  | s, [], _ => .ok s
  | s, op :: ops, i =>
    match step cfg s op with
    | .error e => .error (i, e, s)
    | .ok s' => run cfg s' ops (i + 1)

end Rc
