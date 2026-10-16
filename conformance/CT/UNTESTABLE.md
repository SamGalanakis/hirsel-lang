# External oracles

- **CT-1:** audit compiler capability isolation. Replay closed data evaluation while changing ambient environment and ungranted files; neither result nor acceptance may change. A finite expression cannot prove the evaluator has no ambient channel.
- **CT-2:** inspect materialized data and its dependency graph. Values must be published before dependents use them, while dependent checking reads declared signatures. A program's output alone cannot demonstrate the compiler phase boundary.
- **CT-3:** reserved for future compile-time features (#5); no v1 reflection or generation oracle is required.
- **CT-4, budget invariance:** compare compile-time acceptance and logical step accounting across compiler optimisation levels and topologies. The fixed budget may not become an implementation flag. Exact boundary programs require the missing constant/charging model in #7 and are recorded as pending in COVERAGE.md.
