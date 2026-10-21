# Streaming UTF-8

`std@1/utf8` can decode a sequence of byte chunks without replacing valid scalars
that cross chunk boundaries. The whole-buffer `decode_lossy` API is unchanged.

```text
type Policy = Strict | Lossy
type StreamError = InvalidSequence(u64) | IncompleteSequence(u64)
                 | OffsetOverflow | AlreadyFinished | DecoderFailed
type StreamStatus = Active | Finished | Failed
opaque type Decoder

@noalloc fn decoder(policy: Policy) -> Decoder uses {}
fn feed(mut d: Decoder, chunk: Bytes, eof: bool) -> Result[String, StreamError] uses {}
@noalloc fn offset(d: Decoder) -> u64 uses {}
@noalloc fn pending_bytes(d: Decoder) -> u64 uses {}
@noalloc fn status(d: Decoder) -> StreamStatus uses {}
```

Each successful feed returns its own valid `String`, possibly empty. It includes
all completed scalars from that call and any prefix completed from earlier calls.
The result remains valid after the decoder and input are dropped. The decoder
retains no input view or output chunk. Its state has three inline prefix bytes,
a prefix length, a checked absolute byte offset, policy and status.
Output storage is proportional to the current chunk's decoded output; callers
choose whether to retain or concatenate the returned strings.

Before EOF, an otherwise valid incomplete scalar is held as a prefix of one to
three bytes. Empty nonfinal chunks are allowed and leave that prefix unchanged.
The lead-byte ranges are C2–DF, E0–EF and F0–F4. The second byte excludes overlongs
(E0 A0–BF, F0 90–BF), surrogates (ED 80–9F), and values above U+10FFFF
(F4 80–8F). Other continuation bytes are 80–BF. NUL and ASCII are ordinary data.

In `Strict`, the first malformed sequence returns `InvalidSequence(start)`, with
its zero-based absolute start offset, even when the sequence began in an earlier
chunk. An incomplete prefix at EOF returns `IncompleteSequence(start)`. The failing
call publishes no String, including any valid text preceding that error in the
same call. Earlier returned chunks remain valid. Either error clears the pending
prefix and changes status to `Failed`; further feeds return `DecoderFailed`.

In `Lossy`, each invalid sequence span becomes U+FFFD. A rejected continuation
byte is reconsidered as a new lead byte, following Rust's `Utf8Error::error_len`.
At EOF an otherwise valid incomplete prefix becomes one replacement. For every
partition, concatenating all successful output chunks equals both the existing
`utf8.decode_lossy` and Rust's `String::from_utf8_lossy` on all input bytes.

`offset` counts consumed input bytes, including the pending prefix. A strict
validation error leaves it at the end of Rust's invalid span, excluding a rejected
continuation byte (or at the input end for incomplete EOF); it is diagnostic state
after poisoning. Each feed checks
its entire input length against the remaining `u64` offset range before decoding.
`OffsetOverflow` consumes none of that call, clears the prefix and poisons the
decoder. Allocation/resource failures use ordinary language failure and cleanup
semantics; they are not `StreamError` values or a resumability guarantee.

A successful `eof: true`, including an empty EOF, changes status to `Finished` and
clears the prefix. Every later feed, including another empty EOF, returns
`AlreadyFinished` without changing state. `pending_bytes` is always in 0–3 and is
zero in either terminal state.

```hir
pub fn main() -> Result[(), ()] {
  let mut d = std@1/utf8.decoder(Strict())
  let Ok(first) = std@1/utf8.feed(mut d, b"price: \xe2", false) else {
    return Error(())
  }
  let Ok(last) = std@1/utf8.feed(mut d, b"\x82\xac10", true) else {
    return Error(())
  }
  let mut out: List[u8] = []
  std@1/text.append(mut out, first)
  std@1/text.append(mut out, last)
  failure.check(std@1/text.finish_valid(take out) == "price: €10", TextInvariant())
  Ok(())
}
```

The executable example is
`conformance/SEM/SEM-3-utf8-stream-5-run.hir`. Lane-specific driver fixtures add
all partitions of scalar edges and malformed prefixes, every split of Rust's
lossy vectors, deterministic random Rust-oracle cases, terminal misuse, strict
call atomicity, ownership, fault cleanup and long one-byte feeds.

The validation rules and vectors are ported from rust-lang/rust commit
`a7c37f9f0ee5e93396106e9de20f4a86b4154e63`:
`library/core/src/str/validations.rs:146–215`,
`library/core/src/str/error.rs:82–105`, and
`library/alloctests/tests/string.rs:63–114` (MIT; see third-party notices).
Adaptations are inline resumable prefixes instead of a borrowed whole buffer,
owned per-call output instead of borrowed valid spans, explicit policy and
terminal states, strict per-call atomicity, checked `u64` offsets, and a virtual
prefix-plus-chunk view instead of retaining the whole source slice. Go's classifier
and accept ranges become explicit branches; its width-1 malformed replacement
rule becomes Rust's maximal invalid prefix, as required for whole-buffer parity.
The decoder copies validated UTF-8 bytes rather than assembling Go rune values. Validation
and replacement-span semantics are unchanged.

The carry classifier also ports rust-lang/rust
`f6c1107f8be13e48465e12c0fd26e6a39fe18e4f`,
`library/core/src/str/lossy.rs:201–286` (`Utf8Chunks::next`) and its vectors in
`library/coretests/tests/str_lossy.rs:1–72`; and Go
`ebe2feefc400f813aefdc8a036e66d2619da6649`,
`src/unicode/utf8/utf8.go:110–127` (`FullRune`), `:171–208` (`DecodeRune`),
and `utf8_test.go:30–67,79–106,248–302`. Go's 71 scalar, surrogate and malformed
rows run with every EOF prefix and every byte split. The reference implementations
are read-only; no upstream files are modified.
