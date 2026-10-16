# External oracles

- **FACT-1:** compare contract order, before snapshots, failures and logical allocation sites in interpreter and native backends. Allocation contracts are source properties even if an optimiser eliminates an allocation. Whole-checker soundness for contract purity and @noalloc requires a checker model beyond the finite corpus.
- **FACT-2:** representation/niche validation must prove every constructed value and every value seen by drop satisfies its fact. A source program cannot require a particular niche layout: the implementation "may use it as a niche". Compare valid/invalid boundary constructors against a distinct-value model and audit niche decoding.
- **FACT-3:** execute all FACT contracts/facts at every optimisation level and topology; compare acceptance and first-failure site/rule, including constant-false conditions. Instrument check removal to verify a removed check is proven unable to fail. Prover strength must never affect which source programs check.
