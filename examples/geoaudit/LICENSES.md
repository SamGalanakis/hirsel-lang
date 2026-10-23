# Geometry provenance

Geo 0.33.1, commit `889b8fb6f462a1b0448a4097a9b11f528749bc36`, MIT OR Apache-2.0.
Ported structure: geo/src/algorithm/coordinate_position.rs:281-320,399-455;
polygon vectors :472-542. Segment predicate structure:
geo/src/algorithm/line_intersection.rs:76-161; TomasFa vectors :403-429.
Whole-file SHA256: coordinate_position.rs
`dbc188f206bfbf84fb98ee8b375deeb81e8b54ea278937d32c9e18b47831745a`;
line_intersection.rs
`59ac6d6bc4625596d08aec8b5982d723177c1f7094272f7f351eed1d538e40de`.

Deviations: bounded signed integer coordinates replace robust floating kernels;
checked arithmetic replaces approximate operations; only Boolean contact is
needed, so intersection-point approximation is omitted. Boundary detection is
performed before the winding update with the same half-open edge rule. Rings
must have 3..16 vertices and strict hole topology. The two TomasFa decimal
segment vectors are multiplied by 10 exactly (for example -42.0 -> -420 and
163.2 -> 1632). Square/hole polygon vectors retain their integer coordinates.
Exact scaling changes the TomasFa expectations: both scaled pairs have all four
orientation determinants equal to zero and overlap, so the integer answer is
true. The original robust binary-float vectors expect None; those original
expectations and the failed first helper attempt are retained externally. Geo
on the matching scaled integer-valued inputs agrees with the true answers.
Rational ray parity in support/oracle.py is independent from the winding port.

The MIT license option is reproduced below.

Copyright (c) 2017 The GeoRust Project Developers

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
