# X.509 identity parser prior art

The reference is [Go ebe2feefc400f813aefdc8a036e66d2619da6649](https://github.com/golang/go/tree/ebe2feefc400f813aefdc8a036e66d2619da6649),
cloned for this port. All line numbers below refer to that commit. The Go
BSD license is reproduced in [third-party/NOTICES.md](../third-party/NOTICES.md).

`stdlib/std@1/x509.hir` follows these implementation paths:

- `src/vendor/golang.org/x/crypto/cryptobyte/asn1.go:755-835`: single-octet
  tags, definite lengths of at most four octets, no leading zero length
  octets and no long form for lengths below 128. OID arcs follow
  `asn1.go:421-449`, including the architecture-independent 31-bit bound.
- `src/crypto/x509/parser.go:38-154`: PrintableString's character set
  (including Go's wildcard and ampersand compatibility), UTF-8 validation,
  T61 as Latin-1, BMP big-endian UCS-2 with optional final NUL removed and
  surrogate/noncharacter rejection, IA5 ASCII and NumericString digits/spaces.
- `src/crypto/x509/parser.go:209-247`: sequence of RDN sets, each containing
  attribute sequences with an OID and a value; validate string types before
  rendering, including values of unknown OIDs.
- `src/crypto/x509/pkix/pkix.go:28-104`: attribute-name table, reversed RDN
  order, forward multivalued attribute order and character escaping. Unknown
  OIDs with string values render escaped text. SERIALNUMBER and POSTALCODE
  are short names; DC and UID are dotted OIDs. NUL is preserved as in Go.
- `src/crypto/x509/parser.go:465-534`: SAN sequence iteration, IA5 checks
  for DNS, email and URI, URI syntax and domain checks, 4/16-byte IPs and
  rejection of IPv4-mapped IPv6 SAN addresses.
- `src/net/url/url.go:107-165,441-649,1293-1326` and
  `src/net/url/gen_encoding_table.go:152-182`: the URI acceptance path,
  scheme/authority splitting, userinfo/host alphabets, numeric optional
  ports, bracketed IPv6, zone escapes and path/fragment percent validation.
  Opaque URIs and raw queries preserve Go's escape behavior.
- `src/crypto/x509/parser.go:1395-1452`: `domainNameValid`, including the
  compatibility acceptance of empty names and long names/labels. DNS SANs
  receive only IA5 validation, as in Go; this helper is used for URI hosts.

The imported corpus tables are:

| Upstream test | Source | Imported assertions |
| --- | --- | ---: |
| TestPKIXNameString | `src/crypto/x509/x509_test.go:2181-2300` | 14 table rows + 1 preservation assertion |
| TestRDNSequenceString | `src/crypto/x509/x509_test.go:2302-2340` | 1 |
| TestParseASN1String | `src/crypto/x509/parser_test.go:25-149` | 15 rows, with unknown-OID rejection controls |
| TestDomainNameValid | `src/crypto/x509/parser_test.go:317-383` | 25 active rows |

They live in `conformance/SEM/SEM-3-x509-go-{names,strings,domains}-run.hir`.
`SEM-3-x509-{der,san}-run.hir` add accepted/rejected boundary controls.
The x509 corpus grows from one program with 10 `test.check` call sites to
six programs with 140 (130 added). The existing certificate and serialization
corpus cases remain importers.

## Adaptations and deviations

1. This remains an identity extractor, not Go's full certificate parser or
   verifier. The existing `Certificate` stores subject text, interleaved
   DNS/IP names and URI text. It omits email addresses, but now validates
   their IA5 bytes. Signature verification, validity, key parsing and trust
   stay with the TLS provider. This preserves the existing identity API and
   does not imply that parsing arbitrary DER establishes trust.
2. Hirsel has no dynamic `any` or Go reflection formatter. Non-string
   attribute values retain their exact original DER as `#` plus hex, rather
   than passing through Go's `readASN1Any`, re-marshalling or `fmt.Sprint`.
   This matches the imported unknown-integer vector. Known-OID non-string
   values also retain DER; Go would format those dynamic values. No typed
   Go integer/time/bit-string presentation API is introduced by this port.
3. Explicit offset/limit cursors and owned builders replace cryptobyte's
   mutable slice cursors, Go builders and rune slices. Failures use the
   existing `Malformed(offset)` error, rather than Go error strings. Length
   arithmetic uses checked bounds before subtraction; 31-bit OID limits and
   the four-octet length limit match the reference across host architectures.
4. Name tests enter through `x509.parse` because Hirsel has no `pkix.Name`
   construction API. Synthetic DER envelopes encode each row's effective
   RDN order after Go's Names/ExtraNames rules; Go's actual `certBytes`
   chain is retained for its Google subject row and split before parsing its
   first certificate, as Go's `ParseCertificates(der)[0]` does. The 15th PKIX
   assertion checks shared input preservation, replacing Go's mutable
   zero-length slice backing-array check. String tests use the same small
   identity envelope. These envelopes are not claimed to be signed certs.
5. URI validation retains original text rather than allocating Go URL
   objects. Existing `net.parse_ipv6` handles the decoded address, and
   scoped zones are checked separately. There is no environment-controlled
   `GODEBUG` compatibility switch: HTTP(S) colon validation uses Go's default.
6. The domain table contains the 25 active vectors at the pinned commit;
   its nine commented-out stricter vectors are intentionally not activated.
   The public pure `domain_name_valid(name, constraint)` helper exposes the
   same distinction so both name and constraint vectors are directly tested;
   name-constraint extension parsing remains outside this identity API.
