# First-contact friction

Minutes are approximate elapsed investigation time; overlapping waits are not a benchmark.
Copied diagnostics normalize checkout paths and strip trailing display whitespace.

| Tried | Exact diagnostic | Fix | Minutes lost | Class |
| --- | --- | --- | ---: | --- |
| Used closure parameter `n` under the Net local `n`. | [Exact output](repros/incident-1.txt) | Renamed the closure parameter `served`; LANG-4 forbids lexical shadowing even in nested functions. [Minimal program](repros/incident-1.hir). | 0.5 | language friction |
| Expected `io.dir` errors to fit the Grant arm. | [Exact output](repros/incident-2.txt) | Added `Files(FileError)`; `io.dir` returns FileError rather than GrantError. LANG-8; the diagnostic correctly lists injectable arms. [Minimal program](repros/incident-2.hir). | 0.5 | language friction |
| Returned AppError (with store.Corrupt(String)) from an asynchronous task. | [Exact output](repros/incident-3.txt) | Explicitly return `unique Result[(), AppError]` at the task boundary and use the same TaskSet type; nested counted errors prevent sendability. MODE-4. [Minimal program](repros/incident-3.hir). Wanted: Name `AppError.Store.Corrupt` and its String payload as the reason; suggest `unique Result[(), AppError]`. | 1 | diagnostic quality |
| Translated NetClient.from directly into a grants JSON field. | [Exact output](repros/incident-4.txt) | JSON clients use separate `address` and `port`; added the JSON schema example to building.md. | 0.5 | doc gap |
| Provided listeners without an empty peers array. | [Exact output](repros/incident-5.txt) | Add `"peers":[]` for `net.script@2`; documented that it is mandatory. | 0.5 | doc gap |
| Closed the scripted client before the signal could be delivered at idle. | [Exact output](repros/incident-6.txt) | End the client script with Stall rather than Close; the final recording serves four commands then SIGTERM shuts down successfully. Documented exhaustion and idle delivery. | 0.5 | doc gap |
| Replayed after canonical source formatting. | [Exact output](repros/format-replay.txt): `hirsel: replay refused: {"kind":"binding","field":"program"}` | Re-record only after final formatting. [Before](repros/format-before.txt) / [after](repros/format-after.hir), [reproducer script](repros/format-replay.sh). DET-1 says source positions are excluded from semantic identity; source behavior is unchanged. Routed to @agentlang-24. | 1 | bug |
