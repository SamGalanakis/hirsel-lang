# First-contact friction

Minutes are approximate elapsed investigation time; overlapping waits are not a benchmark.
Copied diagnostics normalize checkout paths and strip trailing display whitespace.
The table preserves original output; the recipes below describe current rules.

| Tried | Historical diagnostic | Current disposition / repair | Minutes lost | Class |
| --- | --- | --- | ---: | --- |
| Named the test `deadline`, already used by a local. | [Exact output](repros/incident-1.txt) | Renamed the test `request_deadline`; LANG-4. [Minimal program](repros/incident-1.hir). | 0.5 | language friction |
| Declared a channel of ordinary counted Status. | [Historical initial output](repros/channel-initial.txt), CONC-6. | Retain `Sender[unique Status]` / `Receiver[unique Status]` and a unique producer. [Historical declaration repro](repros/channel-counted.hir). | 1 | intentional sendability policy |
| Appended a qualified channel receive directly into List[Status]. | [Historical mixed output](repros/incident-2.txt), MODE-2 at `mut incoming`. | Bind `let row = channel.receive(tasks, mut incoming)?`, then append `take row`. The explicit `: unique Status` is unnecessary. [Isolated direct-append repro](repros/incident-2.hir). | 0.5 | intentional contextual inference; diagnostic fixed |
| Translated `resolves_to: List[IpAddr]` to JSON enum objects. | [Exact output](repros/incident-3.txt) | Use IP strings (`["127.0.0.1"]`), not `[{"v4":[127,0,0,1]}]`; added a complete script recipe to building.md. | 0.5 | doc gap |

## Current executable recipes

### Sendability is a payload contract

[The historical declaration repro](repros/channel-counted.hir) stops at CONC-6:
`Status` contains counted `String`, so `Sender[Status]` cannot send it. It does
not isolate receive inference. Use `Sender[unique Status]` and
`Receiver[unique Status]`. Keep `fetch_one`'s `take url: unique String` and unique
result: moving an ordinary counted String does not prove it unshared. The
caller's `unique urls[i]` creates the independent URL copy for each worker.
Changing the receiver's payload contract while mutably borrowing it is forbidden.

### Receive first, then weaken the owned row

[The direct-append repro](repros/incident-2.hir) is a complete source-checking
negative control with `Receiver[unique Status]`. Under LANG-2/LANG-8, the
`List[Status]` destination asks `?` for `Result[Status, F]`, which asks the generic
receive for `Receiver[Status]`. That conflicts with the actual invariant
`Receiver[unique Status]`, producing MODE-2. This rejection is intentional under
today's rules. [Endpoint guidance shipped in b37d5df3](https://github.com/SamGalanakis/hirsel-lang/commit/b37d5df3ec4aeaa0c42b2014345c53e951916f30):
current MODE-2 names `Receiver[unique Status]` and the context-required
`Receiver[Status]`, identifies `list.push`'s `value` argument, and suggests binding
first, then passing `take row`. The [applied-repair regression](../../crates/hirsel-driver/tests/it/receive_endpoint_diagnostic_19.rs)
checks, independently verifies and executes that repair. The historical mixed
output predates this diagnostic fix; copying the receiver is not a repair.

An inferred local lets the receiver fix its payload contract first. Its owned
Result receipt preserves `unique Status` through `?`; `take row` then transfers
the value into ordinary `List[Status]` storage. No payload copy is needed at
receive. This complete program sends, receives, appends and joins its child
(`hirsel run hirsel.json --grants grants.json` with a tasks grant):

```hirsel
pub type AppError = Grant(GrantError) | Channel(ChannelError)
type Status { url: String }

pub fn main(rt: Io) -> Result[(), AppError] {
  let tasks = io.tasks(rt)?
  task.scope(tasks, fn(s: Scope) -> Result[(), AppError] {
    let (tx, rx): (Sender[unique Status], Receiver[unique Status]) = channel.open(tasks)
    let child = task.async(s, fn() -> () {
      let mut outgoing = take tx
      let _ = channel.send(tasks, mut outgoing, Status(url: "http://check.test/"))
    })
    let mut incoming = take rx
    let mut rows: List[Status] = []
    let row = channel.receive(tasks, mut incoming)?
    list.push(mut rows, take row)
    task.await(s, take child)
    Ok(())
  })
}
```

An explicit unique local also works, but adds no proof. An ordinary annotated
`let row: Status = ...` reintroduces the conflicting context. A matching
`List[unique Status]` destination permits direct append, but changes the stored
contract; linkcheck intentionally returns ordinary rows. Keep the local boundary.
Borrowing or copying a unique-payload Option/Result wrapper cannot recreate an
owned receipt; use `take` when extracting an owned payload. A real live alias
needs `unique` to copy independently, not a stronger annotation.

### Other retained recipes

The no-shadow rule is module-wide (LANG-4). [The rejected `deadline` local](repros/incident-1.hir)
conflicts with a top-level function. Rename the local or test; the app keeps
`request_deadline`. Complete repair:

```hirsel
fn deadline() {}

pub fn main() -> Result[(), ()] {
  let at = 0u64
  Ok(())
}
```

Keep declared `Result` errors at scope/entry boundaries and `Ok(take rows)` at
the end of the scope: `?` unwraps success, it does not wrap a return. Each scope
joins its tasks; `incoming` and `outgoing` are mutable rebindings of moved channel
handles. `unique connections` similarly copies borrowed scripted test input
into an owned `NetPeer`. These are explicit ownership/capability recipes, not
remaining lifetime compiler bugs.

For the historical script encoding failure, a peer's complete address fragment
is `{"address":"check.test","port":80,"resolves_to":["127.0.0.1"],"tls":"plain","connections":[]}`.
`resolves_to` takes strings, not JSON enum objects. The executable full fixture
is [grants.json](grants.json); the [building recipe](../../docs/building.md)
explains scripted network grants.
