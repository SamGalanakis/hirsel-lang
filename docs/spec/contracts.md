# Contracts and facts

Contracts are executable signature code: `requires` and `ensures` clauses on functions
([FACT-1](#fact-1-contracts)), and `where` facts on distinct types
([FACT-2](#fact-2-type-facts)). Their checking policy is [FACT-3](#fact-3-checking).

```hirsel
fn clamp(x: u32, hi: u32) -> u32
  requires hi > 0
  ensures result <= hi
{
  if x > hi { return hi }
  x
}

fn push_one(mut xs: List[u64])
  ensures list.len(xs) == before(list.len(xs)) + 1
{
  list.push(mut xs, 1)
}
```

### FACT-1: Contracts

`requires e` is checked on entry; `ensures e` before return, where `result` names the
result. Each is any pure boolean expression over the parameters (and `result`).
`before(e)` evaluates `e` on entry and requires it to be `plain`; an `ensures` may name a
`take` parameter only inside `before`. A false contract is a failure citing it; a failure
while evaluating one cites its own rule. Contracts are part of the signature and are code:
their allocations count toward `@noalloc`.

Clauses see the parameters and the result as read-only borrows, whatever the function's
passing modes. They cannot consume, mutate or finalize parameter storage; fresh local
mutation that is otherwise permitted is allowed. Postcondition access proofs follow the
continuing returns rather than assuming that entry-time uniqueness remains. A false
`ensures` cites the declared clause, not a `return`.

### FACT-2: Type facts

`where` is allowed only on a `distinct` type over a scalar or other plain base. It is
checked at construction, so no value (and no drop) ever violates it; implementations may
use it as a niche. Invariants over several fields belong to opaque types whose constructors
and operations state them as contracts.

```hirsel
distinct type Percent(v: u8) where v <= 100

fn half(p: Percent) -> Percent {
  let Percent(v) = p
  Percent(v / 2)                 // checked: still <= 100
}
```

### FACT-3: Checking

Every contract and fact is checked at run time at every optimisation level. An
implementation may remove a check it proves cannot fail; that changes cost only, never what
compiles or what a program means ([the proof seam](proof-seam.md)).
