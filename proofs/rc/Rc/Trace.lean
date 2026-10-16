import Rc.Machine

/-!
# Trace vectors: format `rc-trace v1`

A vector is a text file:

```
rc-trace v1
max 4
# comment
alloc r1 2
```

The first non-comment line names the format version, the second the count bound `max`
(finite counts are `1 .. max-1`). Every other non-blank line is one operation; `#` starts a
comment. Identifiers carry a sort prefix: `r` roots, `b` borrows, `l` locals, `t` tokens,
`g` regions. Field indices, arities and forget ledger entries are plain numbers.

| syntax | op |
|---|---|
| `alloc rN ARITY [fin]` | T01 |
| `static rN` | T02 |
| `dup rA rB` | T03 |
| `drop rN` | T04 |
| `write rB K rN` | T05 |
| `take rB K rN` | T06 |
| `load bP K rN` | T07 |
| `borrow bN rR` | T08 |
| `borrow-field bN bP K` | T09 |
| `borrow-local bN lL K` | T10 |
| `end-borrow bN` | T11 |
| `reset rB tN` | T12 |
| `reuse tN rR ARITY [fin]` | T13 |
| `dispose tN` | T14 |
| `finalize ok` / `finalize fail` | T15 |
| `drain` | T16 |
| `forget rN` | T17 |
| `region-open gN` | T18 |
| `region-alloc gN lL ARITY` | T19 |
| `region-store lL K rN` | T20 |
| `local-copy lA lB` | T21 |
| `local-drop lN` | T22 |
| `region-close gN` | T23 |

The output (`render`) is the exact final model state, with the ghost `born`/`clock`
stamps omitted. Region blocks carry no header and print `uncounted`. Credits, borrows,
locals and tokens are multisets and print sorted; the worklist prints head first, regions
innermost first, events oldest first.
-/

namespace Rc
namespace Trace

def version : String := "rc-trace v1"

def stripComment (line : String) : String :=
  match line.splitOn "#" with
  | [] => ""
  | l :: _ => l

def words (line : String) : List String :=
  ((stripComment line).splitOn " ").filter (fun w => !w.isEmpty)

def ident (pre : Char) (w : String) : Except String Nat :=
  match w.toList with
  | c :: rest =>
    if c == pre then
      match (String.ofList rest).toNat? with
      | some n => .ok n
      | none => .error s!"expected {pre}<number>, got '{w}'"
    else .error s!"expected {pre}<number>, got '{w}'"
  | [] => .error s!"expected {pre}<number>"

def num (w : String) : Except String Nat :=
  match w.toNat? with
  | some n => .ok n
  | none => .error s!"expected a number, got '{w}'"

def finFlag : List String → Except String Bool
  | [] => .ok false
  | ["fin"] => .ok true
  | ws => .error s!"unexpected '{" ".intercalate ws}'"

def parseOp : List String → Except String Op
  | ["alloc", r, n] => return .alloc (← ident 'r' r) (← num n) false
  | "alloc" :: r :: n :: rest => return .alloc (← ident 'r' r) (← num n) (← finFlag rest)
  | ["static", r] => return .static (← ident 'r' r)
  | ["dup", a, b] => return .dup (← ident 'r' a) (← ident 'r' b)
  | ["drop", r] => return .drop (← ident 'r' r)
  | ["write", b, k, r] => return .write (← ident 'r' b) (← num k) (← ident 'r' r)
  | ["take", b, k, r] => return .take (← ident 'r' b) (← num k) (← ident 'r' r)
  | ["load", p, k, r] => return .load (← ident 'b' p) (← num k) (← ident 'r' r)
  | ["borrow", b, r] => return .borrow (← ident 'b' b) (← ident 'r' r)
  | ["borrow-field", b, p, k] => return .borrowField (← ident 'b' b) (← ident 'b' p) (← num k)
  | ["borrow-local", b, l, k] => return .borrowLocal (← ident 'b' b) (← ident 'l' l) (← num k)
  | ["end-borrow", b] => return .endBorrow (← ident 'b' b)
  | ["reset", b, t] => return .reset (← ident 'r' b) (← ident 't' t)
  | "reuse" :: t :: r :: n :: rest =>
    return .reuse (← ident 't' t) (← ident 'r' r) (← num n) (← finFlag rest)
  | ["dispose", t] => return .dispose (← ident 't' t)
  | ["finalize", "ok"] => return .finalize true
  | ["finalize", "fail"] => return .finalize false
  | ["drain"] => return .drain
  | ["forget", r] => return .forget (← ident 'r' r)
  | ["region-open", g] => return .regionOpen (← ident 'g' g)
  | ["region-alloc", g, l, n] => return .regionAlloc (← ident 'g' g) (← ident 'l' l) (← num n)
  | ["region-store", l, k, r] => return .regionStore (← ident 'l' l) (← num k) (← ident 'r' r)
  | ["local-copy", a, b] => return .localCopy (← ident 'l' a) (← ident 'l' b)
  | ["local-drop", l] => return .localDrop (← ident 'l' l)
  | ["region-close", g] => return .regionClose (← ident 'g' g)
  | ws => .error s!"unknown operation '{" ".intercalate ws}'"

/-- A parsed vector: the bound and the operations with their source line numbers. -/
structure Vector where
  max : Nat
  ops : List (Nat × Op)

def parse (text : String) : Except String Vector := do
  let lines := (text.splitOn "\n").zipIdx.map (fun (l, i) => (i + 1, words l))
  let lines := lines.filter (fun (_, ws) => !ws.isEmpty)
  match lines with
  | (_, ["rc-trace", "v1"]) :: (_, ["max", m]) :: rest =>
    let max ← num m
    if max < 2 then throw "max must be at least 2"
    let ops ← rest.mapM (fun (i, ws) =>
      match parseOp ws with
      | .ok op => .ok (i, op)
      | .error e => .error s!"line {i}: {e}")
    return { max, ops }
  | _ => throw s!"a vector starts with '{version}' and 'max N'"

/-! ## Rendering -/

def showCount : Count → String
  | .dead => "dead"
  | .fin n => s!"fin {n}"
  | .immortal => "immortal"

def showKind : Kind → String
  | .heap => "heap"
  | .static => "static"
  | .region g => s!"region g{g}"

def showSt : BState → String
  | .free => "free"
  | .live => "live"
  | .dying => "dying"
  | .raw => "raw"

def showBlock (a : Nat) (b : Block) : String :=
  match b.st with
  | .free => s!"@{a} free gen {b.gen}"
  | .raw => s!"@{a} raw gen {b.gen} capacity {b.arity}"
  | st =>
    let flags := (if b.fin then " fin" else "") ++ (if b.finDone then " finalized" else "")
      ++ (match b.reset with | none => "" | some t => s!" reset t{t}")
      ++ (if b.published then " published" else "")
    let count := match b.kind with
      | .region _ => "uncounted"
      | _ => s!"count {showCount b.cnt}"
    s!"@{a} {showSt st} gen {b.gen} {count} {showKind b.kind} arity {b.arity}{flags}"

def showHolder : Holder → String
  | .root r => s!"r{r}"
  | .field a k => s!"@{a}.{k}"
  | .forgot n => s!"forget {n}"

def showParent : Parent → String
  | .none => "-"
  | .borrow p => s!"b{p}"
  | .loc l => s!"l{l}"

def showTok : TokVal → String
  | .none => "none"
  | .pending a => s!"pending @{a}"
  | .raw a g c => s!"raw @{a}#{g} capacity {c}"

def holderKey : Holder → Nat × Nat × Nat
  | .root r => (0, r, 0)
  | .field a k => (1, a, k)
  | .forgot n => (2, n, 0)

def lexLe (a b : Nat × Nat × Nat) : Bool :=
  a.1 < b.1 || (a.1 == b.1 && (a.2.1 < b.2.1 || (a.2.1 == b.2.1 && a.2.2 ≤ b.2.2)))

def section_ (title : String) (rows : List String) : List String :=
  title :: rows.map (fun r => "  " ++ r)

def render (s : State) : String :=
  let heap := (List.range s.next).map (fun a => showBlock a (s.heap a))
  let credits := (s.credits.mergeSort (fun c d => lexLe (holderKey c.h) (holderKey d.h))).map
    (fun c => s!"{showHolder c.h} -> @{c.tgt}#{c.gen}")
  let borrows := (s.borrows.mergeSort (fun b c => b.id ≤ c.id)).map
    (fun b => s!"b{b.id} guard {showHolder b.guard} parent {showParent b.parent} -> @{b.tgt}#{b.gen}")
  let locals := (s.locals.mergeSort (fun l m => l.id ≤ m.id)).map
    (fun l => s!"l{l.id} -> @{l.tgt}#{l.gen}")
  let tokens := (s.tokens.mergeSort (fun t u => t.id ≤ u.id)).map
    (fun t => s!"t{t.id} {showTok t.val}")
  let work := s.work.map (fun a => s!"@{a}")
  let regions := s.regions.map (fun R =>
    s!"g{R.id} blocks [{", ".intercalate ((R.blocks.mergeSort (· ≤ ·)).map (fun a => s!"@{a}"))}]")
  let events := s.events.reverse.map (fun
    | .finalized a g ok => s!"finalize @{a}#{g} {if ok then "ok" else "fail"}")
  "\n".intercalate (section_ "heap" heap ++ section_ "credits" credits ++ section_ "borrows" borrows
    ++ section_ "locals" locals ++ section_ "tokens" tokens ++ section_ "work" work
    ++ section_ "regions" regions ++ section_ "events" events) ++ "\n"

/-- Evaluate a vector with the proved machine (`Rc.run`) from the empty state and print its
outcome followed by the final (or stuck) model state. -/
def evaluate (v : Vector) : String :=
  let cfg : Config := { max := v.max }
  let header := s!"{version} max {v.max}\n"
  match run cfg {} (v.ops.map (·.2)) 0 with
  | .ok s => header ++ s!"ok after {v.ops.length} steps\n" ++ render s
  | .error (i, e, s) =>
    let line := ((v.ops[i]?).map (·.1)).getD 0
    header ++ s!"error at step {i + 1} (line {line}): {e}\n" ++ render s

end Trace
end Rc
