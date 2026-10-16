# Native library adapters

`library.h` declares the fixed-result capability wrappers, the integer order entries and four f32/f64 IEEE bits/from_bits leaves. Arithmetic scalar operations are LIR instructions (schema revision 6); scalar text and reductions are ordinary std source. `hirsel-rt/src/operations.rs` classifies exact reviewed registry declarations; unimplemented paths are explicit unavailable entries, never interpreter fallback. Array/string/collection/callback result layout is owned by compiled physical-value glue, not a Rust universal value.

The sixteen wave-1 algorithms in `stdlib/std@1/{list,string,text}.hir` load
through ordinary package manifests and compile as Source-origin native units.
Four generic list primitives and failure.check use compiled library bodies;
bytes.len uses SeqLen, while fixed byte/position/path bridges use typed runtime
leaves. Existing private cursor, map, sorted-map, iterator and Test bodies
also compile through the library seam. Shared/arena/task/channel/mutex/atomic
operations retain their explicit M2 availability.

Wave-2 string search, ASCII case/trim, concat/join, bytes.replace and
map.same_entries compile from ordinary std source. The byte replacement runtime
export and privileged map equality body are retired along with their core rows.
The text inventory records source/ownership controls and matrix evidence.
