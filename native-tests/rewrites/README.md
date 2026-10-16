# Optional rewrite ledger

`ledger.tsv` is the proof-domain ledger for the optional Ready rewrites in
`crates/hirsel-rc/src/legalize.rs`. A rewrite is discovered
read-only, proved by looking up its rule and exact scalar domain here, and
committed only when the row says `proved`; the Ready validator then checks the
committed unit again.

Columns: `rule`, `domain` (`int:u32`, `float:f64`, ...), the `failure` and
`layout` assumptions the proof makes, its `evidence` (an exhaustive test, an
argument, or a solver run with its tool and version), and `status`
(`proved`, `unknown`, `timeout`, `refuted`). Solver timeouts and unknowns are
recorded, never treated as proofs. A theorem for one width or signedness says
nothing about another, so every domain has its own row, and i128, SIMD lanes
and float rows need their own evidence.

`cargo test -p hirsel-rc --test it legalize::` parses this file, reruns the
exhaustive and boundary evidence the proved rows cite, and checks that a
rewrite runs only in proved domains.
