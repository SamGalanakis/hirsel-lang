# v20 concurrency coverage

All current call spellings are ratified in section 22. CONC-3 uses CancelError;
child-local checks open a nested task.scope using captured Tasks and inherit
cancellation. ChannelError includes Closed and Canceled; receive checks cancellation
without dequeuing, while unbounded-FIFO send never checks cancellation and still
yields. A canceled send succeeds for an open receiver or returns `Closed` with its
owned payload for a closed receiver. The CONC-3 witnesses are
[`canceled-send-open`](CONC-3-canceled-send-open-run.hir),
[`canceled-send-closed`](CONC-3-canceled-send-closed-run.hir),
[`canceled-send-check`](CONC-3-canceled-send-check-run.hir) (an explicit
`check_canceled` before send), and
[`canceled-receive-ready`](CONC-3-canceled-receive-ready-run.hir) (cancellation
still precedes queued messages and `Closed`, and wakes a waiting receive).

Other run witnesses cover draining a buffered message and taking a Receiver into a child then rebinding
it mutable. Existing tests cover handle consumption, scope joining, mutex capture,
unique payloads/results and scalar atomic constraints.

Exhaustive cancellation injection, causal failure ordering and fair scheduling
remain model oracles (UNTESTABLE.md). Dead-mutex re-acquisition is still #22.
`conformance/pending.txt` lists any dynamic program that cannot pass yet.
