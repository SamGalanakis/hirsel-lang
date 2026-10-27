RustFFT (MIT OR Apache-2.0), copyright 2014–2021 Aaron Hill and contributors.
Retained licenses: LICENSE-RustFFT-MIT, LICENSE-RustFFT-APACHE.
Pin: 3bfe1afaecf5b3e5cc3d70f861a8aa5234aa50f1.
algorithm/dft.rs SHA256 eee7479bb8dc30adfeefc8bde721ef47753adde8de634e1329b65b732c35e668.
twiddles.rs SHA256 78a0e7cce049e64f94c38c18c10415b36023e6a023083ba319ba7f5dadb4d8cc.

transform.hir ports the direct DFT loop and twiddle indexing at dft.rs:27–72;
reuses conjugated forward twiddles for inverse; inverse normalizes once unlike
RustFFT's unnormalized API. Concrete f64 and unique lists replace generic Rust
slice aliasing and mutable output slices. The test generator ports exact len2/4
vectors from dft.rs:284–299,327–359 and uses a separate Taylor trigonometric
oracle based on dft.rs:94–106. Power-of-two lengths also port the transpose and layered structure of
simd/simd_radixn.rs:215-286 and the scalar radix-2 sum/difference butterfly
of algorithm/scalar_column_butterflies.rs:16-22 from the same pin. Hirsel
uses scalar fields, bit-reversed list copies and the existing full twiddle
table rather than packed SIMD columns. Other lengths retain the direct DFT.
No FFT planner or SIMD code is included.
