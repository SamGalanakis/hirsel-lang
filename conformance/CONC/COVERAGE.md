# v20 concurrency coverage

All current call spellings are ratified in section 22. CONC-3 uses CancelError;
child-local checks open a nested task.scope using captured Tasks and inherit
cancellation. ChannelError includes Closed and Canceled; receive checks cancellation
without dequeuing, while unbounded-FIFO send does not block. New run witnesses
cover draining a buffered message and taking a Receiver into a child then rebinding
it mutable. Existing tests cover handle consumption, scope joining, mutex capture,
unique payloads/results and scalar atomic constraints.

Exhaustive cancellation injection, causal failure ordering and fair scheduling
remain model oracles (UNTESTABLE.md). Dead-mutex re-acquisition is still #22.
These dynamic programs require M2 runtime behavior (`conformance/m2-runtime.txt`).
