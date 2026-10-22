# CIDR prefixes

`std@1/cidr` parses and compares IPv4 and IPv6 prefixes as pure data. Every
operation uses `{}`: no DNS, socket, network grant or policy lookup occurs.
A CIDR match is data processing; it grants no capability and changes no
network allowlist.

`Prefix` has a private address and validated width. `from_addr(ip, width: i64)`
returns `Result[Prefix, PrefixError]`, accepting 0..32 for IPv4 and 0..128 for
IPv6. Negative or excessive widths return `WidthOutOfRange`. Its advertised
`copyable + inert + noalloc_drop` bounds follow from `IpAddr`'s fixed inline
bytes and the scalar width; there are no generic fields or resource handles.
There is no unchecked public constructor or deserialization bypass.

`parse(text)` requires exactly one slash, an address accepted by
`std@1/net.parse_ip`, and an unsigned decimal width with no redundant leading
zeros. Zones, bracketed addresses, ports, signs, whitespace and trailing text
are rejected. Error arms distinguish `InvalidPrefix` (slash structure),
`InvalidAddress`, `InvalidWidth` (width syntax) and `WidthOutOfRange` (numeric
range). Even arbitrarily long decimal widths return a typed error without
integer overflow; all bytes are checked before range validation.

`addr(prefix)` and `bits(prefix)` inspect the supplied representation.
`parse("192.168.1.99/24")` preserves `192.168.1.99`; only `masked(prefix)`
returns `192.168.1.0/24`. Masking preserves the width and is idempotent.
`format(prefix)` uses `net.ip_to_string` followed by slash and decimal width,
so formatting normalizes address spelling while retaining host bits.

`contains(prefix, ip)` compares the leading bits in the same family.
`overlaps(a, b)` compares the common leading width and is symmetric.
Noncanonical host bits do not affect either operation. IPv4-mapped IPv6
addresses remain IPv6: `::ffff:192.0.2.1` cannot match `0.0.0.0/0`, but it
matches `::/0`. A default route matches only its own family. No address-set
enumeration or network-policy enforcement is provided.

## Reference and adaptations

Ported from Go `ebe2feefc400f813aefdc8a036e66d2619da6649`,
`src/net/netip/netip.go:1363-1394` (parse and validation), `1406-1484`
(masking, family checks and overlap), with vectors from
`netip_test.go:1526-1618,1673-1739`. BSD-3-Clause (Go); the license is retained
in `third-party/NOTICES.md`.

- Existing Hirsel address parsing and formatting replace Go's address helpers.
  Hirsel's `IpAddr` has no invalid or zoned variant, so no Go zero-prefix or
  zone-state representation is exposed.
- A checked constructor and private fields replace Go's representable invalid
  prefixes. Typed errors replace Go error strings and panic helpers.
- Fixed octets replace packed machine words: complete prefix bytes use 255,
  suffix bytes use zero, and only a partial byte shifts by 1..7. Family checks
  precede matching, including for width zero. Overlap compares at the minimum
  width directly, without allocating canonical prefixes.
- A saturating decimal accumulator replaces `strconv.Atoi`, while checking
  the full suffix for syntax. It supports long invalid input without overflow.
- Go's reference membership and invalid-input vectors are retained. Added
  vectors cover mapped IPv6, exact IPv6 prefixes, host bits and width extremes.
  Independent Rust per-bit oracles cover every width for zero, all-one and
  patterned addresses (486 prefixes), 1,944 bit-flip membership comparisons,
  and 256 generated prefix pairs for overlap symmetry and containment.

Lane-private fixtures execute through the interpreter, H, LLVM and Cranelift,
with complete observations compared and heap cleanup checked. The bounded
Rust oracle harness emits small source programs under the normal interpreter
budget; no corpus step cap is raised.
