# First-contact friction

Minutes are approximate elapsed investigation time; overlapping waits are not a benchmark.
Copied diagnostics normalize checkout paths and strip trailing display whitespace.

| Tried | Exact diagnostic | Fix | Minutes lost | Class |
| --- | --- | --- | ---: | --- |
| Named the test `deadline`, already used by a local. | [Exact output](repros/incident-1.txt) | Renamed the test `request_deadline`; LANG-4. [Minimal program](repros/incident-1.hir). | 0.5 | language friction |
| Sent a Status containing String through a channel, then consumed the receive directly as List[Status]. | [Exact output](repros/incident-2.txt) | Use `Sender[unique Status]`, `Receiver[unique Status]`, `take url: unique String`, a unique result, and an explicit `let row: unique Status = receive(..)?` before weakening into the output list. CONC-6/MODE-2/MODE-4. [Initial diagnostics](repros/channel-initial.txt). [Minimal program](repros/incident-2.hir). | 1.5 | language friction |
| Translated `resolves_to: List[IpAddr]` to JSON enum objects. | [Exact output](repros/incident-3.txt) | Use IP strings (`["127.0.0.1"]`), not `[{"v4":[127,0,0,1]}]`; added a complete script recipe to building.md. | 0.5 | doc gap |
