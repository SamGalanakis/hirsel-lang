# Remaining failure coverage

Failure, SourceSite and FailureKind have concrete fields/variants in section 22;
metadata comparisons can use those definitions. Exact Depth and bounded OutOfSpace
cases still need #16's deterministic constants/accounting. LANG-9 test.check
selection and entry/capability-denial checks need #4 fixture/launch support.

FAIL-4 covers successful re-acquisition and an uncaught first body failure.
Safe observation after killing a non-sendable mutex remains the actual #22 design
question. Do not manufacture a forbidden capture to test it. Failure/cancellation
cleanup traces require M2; `conformance/pending.txt` lists any that cannot pass yet.
