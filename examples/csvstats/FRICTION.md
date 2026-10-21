# First-contact friction

Minutes are approximate elapsed investigation time; overlapping waits are not a benchmark.
Copied diagnostics normalize checkout paths and strip trailing display whitespace.

| Tried | Exact diagnostic | Fix | Minutes lost | Class |
| --- | --- | --- | ---: | --- |
| Looked for record-to-JSON support in the tour. | No compiler diagnostic. | Tour said “There is no automatic conversion of application records to JSON”; spec documents `derive.json_value`. Corrected the tour and demonstrated nested Summary converters. | 0.5 | doc gap |
| Ran native CLI compilation with its default worker count on a shared host. | [Exact output](repros/incident-2.txt) | Set `HIRSEL_JOBS=1`, then native compilation passed. Added the worker-limit recipe to docs/building.md; unrelated thread-creation failure output is also retained in logtail below. | 1.5 | doc gap |
