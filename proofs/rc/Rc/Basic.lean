/-!
# Basic vocabulary of the sequential RC/region model

Allocation identities are addresses (`Nat`) paired with a generation that every value birth at
an address bumps. Owning holders are explicit *credits*; the ideal count of an address is the
number of credits that target it. The concrete header count is bounded by `Config.max`.
-/

namespace Rc

/-- Model configuration. Finite header counts range over `1 .. max - 1`; a count that would
reach `max` saturates to `Immortal`. Theorems assume `2 ≤ max` (`Config.Ok`). -/
structure Config where
  max : Nat
  deriving DecidableEq, Repr

def Config.Ok (cfg : Config) : Prop := 2 ≤ cfg.max

/-- ABI v2's full u32 sentinel; smaller test bounds instantiate the same protocol. -/
def Config.production : Config := { max := 4294967295 }

theorem Config.production_ok : Config.production.Ok := by
  unfold Config.Ok Config.production
  decide

/-- Concrete header count: `Dead | Finite(1..MAX-1) | Immortal`. -/
inductive Count where
  | dead
  | fin (n : Nat)
  | immortal
  deriving DecidableEq, Repr

/-- Storage owner of an allocation instance (M1b subset). -/
inductive Kind where
  | heap
  | static
  | region (g : Nat)
  deriving DecidableEq, Repr

/-- Lifecycle of one storage slot.
* `free`: dead storage, may be handed out by allocation (with a bumped generation);
* `live`: holds a value;
* `dying`: zero holders, on the dead worklist, children being released;
* `raw`: raw storage owned by exactly one reuse token. -/
inductive BState where
  | free
  | live
  | dying
  | raw
  deriving DecidableEq, Repr

/-- One storage slot. Field contents are not stored here: an initialized pointer field is a
`Holder.field` credit in the ledger, an uninitialized field has none (partial initialization). -/
structure Block where
  st : BState := .free
  gen : Nat := 0
  cnt : Count := .dead
  kind : Kind := .heap
  arity : Nat := 0
  /-- The value is a resource with a user finalizer (noncopyable). -/
  fin : Bool := false
  /-- The finalizer obligation of this generation has been consumed. -/
  finDone : Bool := false
  /-- A pending `reset`: when the block finishes draining it becomes raw storage for this token. -/
  reset : Option Nat := none
  /-- Region blocks only: exposed to another binding, so never mutated again (MEM-10). -/
  published : Bool := false
  /-- Ghost stamp: every edge points from a younger to an older stamp (acyclicity witness). -/
  born : Nat := 0
  deriving DecidableEq, Repr

/-- Owner of one credit. -/
inductive Holder where
  /-- A program holder (a local binding or temporary holding an owned value). -/
  | root (id : Nat)
  /-- Initialized pointer field `k` of block `src` (heap or region block). -/
  | field (src k : Nat)
  /-- The permanent ledger of an explicit `forget` (static exemption of a subgraph). -/
  | forgot (id : Nat)
  deriving DecidableEq, Repr

/-- One owning credit on the allocation `tgt` of generation `gen`. -/
structure Credit where
  h : Holder
  tgt : Nat
  gen : Nat
  deriving DecidableEq, Repr

/-- What keeps a borrow's parent alive: nothing (borrow of a root), another borrow, or a local
reference into a region block. -/
inductive Parent where
  | none
  | borrow (id : Nat)
  | loc (id : Nat)
  deriving DecidableEq, Repr

/-- A scoped non-owning view of `tgt`. It is valid while its `guard` credit is held; the
guard cannot be released while the borrow is live (lifetime guard). -/
structure Borrow where
  id : Nat
  guard : Holder
  parent : Parent
  tgt : Nat
  gen : Nat
  deriving DecidableEq, Repr

/-- An uncounted reference to a region block, bounded by the region (a bound loan). -/
structure Local where
  id : Nat
  tgt : Nat
  gen : Nat
  deriving DecidableEq, Repr

/-- The value of a reuse token. `none` means "allocate fresh"; `pending a` is a reset whose
donor is still draining; `raw a gen cap` owns the raw storage at `a`. -/
inductive TokVal where
  | none
  | pending (a : Nat)
  | raw (a gen cap : Nat)
  deriving DecidableEq, Repr

structure Token where
  id : Nat
  val : TokVal
  deriving DecidableEq, Repr

/-- An open compiler-local region and the blocks allocated in it. -/
structure Region where
  id : Nat
  blocks : List Nat
  deriving DecidableEq, Repr

/-- Ordered abstract observable events: a finalizer call with its outcome. -/
inductive Event where
  | finalized (a gen : Nat) (ok : Bool)
  deriving DecidableEq, Repr

structure State where
  heap : Nat → Block := fun _ => {}
  /-- Every address at or above `next` is untouched default storage. -/
  next : Nat := 0
  /-- Ghost clock for `Block.born`. -/
  clock : Nat := 0
  credits : List Credit := []
  borrows : List Borrow := []
  locals : List Local := []
  tokens : List Token := []
  /-- The dead worklist, head first (MEM-3: linked through dead headers, no recursion). -/
  work : List Nat := []
  /-- Open regions, innermost first. -/
  regions : List Region := []
  /-- Finalizer events, most recent first. -/
  events : List Event := []

/-- Point update of the heap. -/
def upd (f : Nat → Block) (a : Nat) (b : Block) : Nat → Block :=
  fun x => if x = a then b else f x

@[simp] theorem upd_same (f : Nat → Block) (a : Nat) (b : Block) : upd f a b a = b := by
  simp [upd]

theorem upd_apply (f : Nat → Block) (a : Nat) (b : Block) (x : Nat) :
    upd f a b x = if x = a then b else f x := rfl

@[simp] theorem upd_ne {f : Nat → Block} {a x : Nat} {b : Block} (h : x ≠ a) :
    upd f a b x = f x := by simp [upd, h]

/-- Ideal count of an address: the number of credits that target it. -/
def icount (s : State) (a : Nat) : Nat := s.credits.countP (fun c => c.tgt == a)

namespace Holder
/-- Source block of a field holder. -/
def src? : Holder → Option Nat
  | .field src _ => some src
  | _ => none

def isFieldOf (a : Nat) : Holder → Bool
  | .field src _ => src == a
  | _ => false

def idx : Holder → Nat
  | .field _ k => k
  | _ => 0
end Holder

/-- Header increment: saturates to `Immortal` instead of reaching `max` (never wraps). -/
def Count.incr (cfg : Config) : Count → Count
  | .fin n => if n + 1 < cfg.max then .fin (n + 1) else .immortal
  | c => c

end Rc
