# hirsel-codec

The canonical S-expression snapshot codec shared by every level of the compiler:
one text encoding serves as the normalized dump, the round-trip format and the
digest input (see the crate documentation for the invariants).

A shared data crate. It depends only on `hirsel-syntax` (DefPath and ContentHash encodings live
here under the orphan rule) and may be used by TIR- and
LIR-level code alike. LIR's snapshot domain digest still goes through
`hirsel-lir`; contexts (`Snapshot<C>`) let encoders carry lookup tables without
changing the text format.

Semantic Primitive and BuiltinType snapshot implementations live with their
enums in hirsel-types, which depends on this codec.
