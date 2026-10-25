# External oracles

- **CT-1:** audit compiler capability isolation. Replay closed data evaluation while changing ambient environment and ungranted files; neither result nor acceptance may change. A finite expression cannot prove the evaluator has no ambient channel.
- **CT-2:** inspect materialized data and its dependency graph. Values must be published before dependents use them, while dependent checking reads declared signatures. A program's output alone cannot demonstrate the compiler phase boundary.
- **CT-3:** reserved for future compile-time features (#5); no v1 reflection or generation oracle is required.
- **CT-4, budget invariance:** acceptance must not depend on the optimisation level, worker count or cache state. One program's output cannot show this; driver tests compare exact-boundary programs across workers, stores, V and lowering policy, and the interpreter tests compare specialized and general dispatch.
- **CT-5, staging and caching:** that a generator's text is cached under the implementations it executed and the declarations its shapes read, and regenerated when one changes, is an incremental-compilation property; one program's output cannot show it. Engine tests edit a shaped type and a generator and compare the generated text.
