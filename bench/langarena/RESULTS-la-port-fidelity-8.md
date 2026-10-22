# LangArena port fidelity, 2026-10-22

Baseline: current fetched main `35d4a54a`, upstream LangArena `3c9f84ddaeec834c0631747cb789e8cfd602f993`. Compiler, LLVM 21.1.8, std and runtime remain fixed between candidates. Native LLVM release builds; unchanged `run.js` inputs and checksums. No language, TIR/LIR, runtime ABI or std changes.

## Instruction and allocation evidence

Three rounds of seeded order (`20261022`), CPU 7, `nice -n 10`, using `compare.Session.sample` and its capped task runner. All 27 samples pass their checksums, and every counter has 100% running coverage. These are whole-process counts including startup, preparation and warmup. Allocation totals are from separate measurement-runtime builds; allocations equal frees on every task. No timing lock was acquired, and no wall-clock speedup is claimed.

| Task | Rust instructions | Hirsel before | Hirsel after | H/R before | H/R after | Allocations before → after |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Binarytrees::Arena | 5,097,339,289 | 13,078,013,076 | 10,476,894,868 | 2.566 | 2.055 | 1,064 → 1,064 |
| Compress::LZWDecode | 9,225,990,763 | 48,311,821,614 | 11,689,723,952 | 5.236 | 1.267 | 10,872,419 → 10,618,979 |
| Compress::HuffDecode | 13,184,166,041 | 91,276,763,025 | 79,670,138,887 | 6.923 | 6.043 | 5,400 → 5,400 |

Kernel instruction counts vary with paging and host load. User-space counters isolate that effect:

| Task | User H/R before | User H/R after |
| --- | ---: | ---: |
| Binarytrees::Arena | 1.433 | 1.500 |
| Compress::LZWDecode | 4.585 | 0.609 |
| Compress::HuffDecode | 6.828 | 5.947 |

Arena trades a small increase in user instructions for the reference-width payload and fewer paging instructions. LZW removes scalar copy/append loops. Huffman removes excess enum dispatch and reads each encoded byte once; its owned cursor still incurs 1,298,190,145 count increments and 1,298,195,094 decrements (before: 1,298,190,137 / 1,298,195,086). This remaining cost is a language-lane finding.

Profiles were collected before editing, on all three full-size native tasks with `perf record -e cycles:u -g`, through the same task executable/config/filter protocol. Arena traversal accounts for 23.84% of samples; LZW `copy_bytes` 37.50%; Huffman decode 56.55%. Unresolved/skid buckets prevent a complete attribution, especially for Arena.

Frozen compiler, runtime archives, link inputs, binaries, source/config/grants identities, raw perf records, PMU coverage, logs and counter JSON are retained outside the worktree at `/workspace/code/hirsel-briefs/langarena-2026-10-21/la-port-fidelity-8/`. `accepted-counts.json` is the final campaign; `before-identities.json` identifies baseline inputs. Preliminary `counts.json`, the regressing record trial `final-counts.json`, and the corrected move candidate remain separate. The record trial borrowed an owned child Option and added an avoidable copy; the accepted port consumes it and moves the child. Failed setup/argument attempts and the canceled superseded Buck test remain in their original logs.

## Reference fidelity

- **Arena:** `rust/src/benchmarks/binarytrees.rs:65-76,104-160`. Three i32 fields, -1 children, independent left/right tests, wrapping u32 subtree sums, and a state-owned vector replaced each iteration now follow Rust. No capacity reuse or precomputed sum was introduced.
- **LZW:** `rust/src/benchmarks/compress.rs:962-1002`. Clone the old dictionary entry, append one byte, extend output from the selected dictionary entry, then insert the new entry. `unique data` performs the language deep copy; `bytes.from_list(copy xs)` shares the existing list payload as a byte view, and `bytes.append` provides bulk extension. The initial dictionary, capacity request, two-byte code order, special next-code case, empty input and invalid-code behavior match Rust. Odd code counts now fail explicitly before dictionary allocation.
- **Huffman:** `rust/src/benchmarks/compress.rs:255-473,512-519`. The node now has i32 frequency, byte, leaf flag and optional boxed children; lengths/codes/bit count are i32. Empty trees return None. Decode loads each byte once and traverses its bits MSB first with both bit-count and input-length guards. It preserves upstream bit-count-sized zeroed scratch, rebuilding the tree each iteration, dummy zero leaves and zero-byte filtering. No flattened tree, lookup decoder or smaller scratch was substituted.

All remaining source/representation deviations from these three reference paths:

1. Hirsel collections and boxes use the existing counted allocator and List metadata rather than Rust Vec/Box. `unique List` cloning copies physical capacity, while Rust Vec::clone copies initialized length (`crates/hirsel-rt/src/copy.rs:19,62-110`); allocation and growth policies therefore differ. The bulk byte view does not clone the output source or change dictionary storage.
2. A rebindable Rust borrowed tree cursor cannot be expressed as `let mut node = root; node = child`: Hirsel treats the binding as a read-only alias (LANG-15). The owned counted cursor remains, with explicit move from its temporary Option. Adding a borrowed-cursor feature is outside this port lane.
3. No bulk initialized List constructor or `list.truncate` exists on the frozen std/core surface. Scratch initialization remains reserve + pushes, and tail removal remains pops. These faithfully preserve initialized length/content, but add per-element work relative to Rust vec![0; n] and u8 Vec::truncate. Follow-ups belong to std, with no port-specific capacity or unsafe workaround.
4. Huffman leaf sorting uses stable insertion rather than Rust stable sort; insertion into the sorted node list uses push + swaps rather than Vec::insert. Stable sort/list.insert are absent from the surface. Frequency order and Rust binary-search equal-frequency placement are preserved; the alphabet is at most 256. This affects tree construction rather than the per-bit loop.
5. Harness sizes, collection indices and Arena depth remain u64 where Rust uses usize/i32 or casts its i64 config. Hirsel uses checked narrowing rather than unchecked `as`; bounds and arithmetic failures use language checks rather than Rust panics. Incomplete LZW codes fail via `failure.check`, and an absent Huffman child fails through the ordinary language failure path rather than Option::unwrap. All configured sizes/indices/shifts fit. Wrapping checksums and signed node-to-u32 conversion are explicit.
6. LZW preparation still uses the existing Hirsel Map[Bytes,u64] encoder with owned byte keys, rather than Rust HashMap<Vec<u8>,...>; this pre-existing preparation representation is outside the decoder change and is included in the whole-process counts. The shared byte-clone helper now uses bulk copying there too. Full-size LZWEncode remains exact.

## Minimal remaining gaps

```hirsel
// LANG-15: a read-only alias cannot be rebound, even with let mut.
type Node = Leaf | Branch(child: box Node)
fn walk(root: box Node) {
  let mut node = root
  if let Branch(child: child) = node { node = child }
}

// LANG-4: these two operations are absent on the frozen surface.
fn scratch() {
  let mut xs: List[u8] = list.filled(0u8, 1000u64)
  list.truncate(mut xs, 1u64)
}
```

The full cursor reproducer and separate missing-API probes, with compiler diagnostics, are in the evidence directory. A borrowed cursor must keep its root alive while allowing rebinding without retaining each traversed box; filled/truncate must preserve initialized length, uniqueness and destruction semantics. Wall-clock timing under the required interleaved timing lock is also an open follow-up.

## Validation

- Frozen baseline interpreter: 1 codec test passed. Final interpreter: 2 tests passed (new arena vectors plus expanded codec vectors). Final LLVM and Cranelift: the same 2 tests passed; after the child-move refinement the affected codec test passed again on both backends. No assertion was removed or weakened.
- Arena vectors cover leaf sentinels, seven-node signed layout, and wrapping negative subtree sums. Codec vectors cover exact upstream bytes, round trips, empty/single-symbol inputs, zero-leaf filtering, a truncated Huffman payload, the LZW next-code case, and invalid-code return behavior.
- Scoped Buck2 `root//:gate-langarena`: 2 protocol tests passed, with explicit test-count output. Ten affected quick task checksums pass. Full-size Arena/LZWDecode/HuffDecode pass in every before/after/Rust sample; full-size HuffEncode and LZWEncode also pass after their shared helpers changed.
- Hirsel formatting checks and `git diff --check` pass. Landing runs the repository build, Clippy and formatting checks.
