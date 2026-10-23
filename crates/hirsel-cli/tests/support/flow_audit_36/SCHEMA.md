# Frozen interface, version 1

`flowaudit audit ROOT INPUT REPORT JOBS [--interruptible]` solves and atomically replaces REPORT.
`flowaudit verify ROOT INPUT REPORT JOBS [--interruptible]` independently checks REPORT against INPUT and never writes. JOBS is 1..4.

Manifest: {version:1, vertices:[name], arcs:[{name,source,target,capacity}], source:name, sink:name, scenarios:[{name,overrides:[{arc:name,capacity}]}]}. Names are nonempty UTF-8 strings of at most 128 bytes, unique within each named collection; arc endpoints use vertex names. Overrides cannot repeat an arc. All collections and byte bounds follow the brief. No implicit default scenario.

Report: {version:1, results:[{name,capacities:[u64],flows:[u64],value:u64,reachable:[bool],cut_arcs:[ordinal],cut_capacity:u64}]}. Arrays follow manifest ordinals; cut_arcs strictly increasing. A feasible flow with matching cut is accepted even when its flow differs from the solver's. Reachable must equal the residual fixed point. Reports may contain JSON whitespace up to 32768 bytes. Domain errors exit 2; stdout/stderr are `flowaudit: N scenarios\n` / `flowaudit: ERROR\n`.
