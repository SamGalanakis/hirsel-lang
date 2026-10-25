# Cross-worker heap models

Loom models of cross-worker storage return in `crates/hirsel-rt/src/remote.rs` (design §6
Heaps). `loom/` compiles that module from its own source against loom's atomics
(`loom/src/lib.rs`), so the models check the code the runtime runs. A slab's memory is a loom
`UnsafeCell` that every protocol access reads and its release writes, so loom reports any
touch of a released slab as a race; each model also asserts every freed block or node is
collected exactly once.

- `remote_final_free_against_trim`: another heap frees a slab's last block while its owner
  trims; the slab goes back only after the publisher stopped touching it.
- `concurrent_remote_frees_are_collected_once`: two heaps free blocks of one slab while the
  owner collects; a slab is queued again after a dequeue and nothing is lost.
- `retired_heap_has_one_collector`: a worker retires (orphans) its heap while another adopts it
  and a third frees into it; one collector at a time.
- `return_list_hands_every_node_over_once`: the push-only list large blocks and region segments
  return on.

`loom/` is a workspace crate, so every main run runs these models with the workspace tests.
`check.sh` runs the models and two mutations that must fail: `direct-dequeue` (the owner's
direct collection of its current page clears the queued flag) and `blind-trim` (a trim
releases an empty slab without the busy test). Writing the model found one ordering bug: a
publisher that queues a slab again after a dequeue must acquire the dequeue, or its queue-link
write races the previous publisher's.
