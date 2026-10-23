# References and deviations

Ported robfig/cron at bc59245fe10efaed9d51b56900192527ed733435 (MIT):
parser.go:236–363 field/range/step structure and star-bit semantics;
spec.go:58–188 month/day/hour/minute carry and five-year horizon;
spec_test.go:9–118 and parser_test.go:12–90 independent numeric vectors.
The copyright/permission notice is retained in LICENSES/robfig-cron.txt.

Original whole-file SHA256:
- parser.go: 1da6876acb8410f3e4e098f3e503aee2a4b95f2418707463cb12bf6492c32723
- spec.go: d238b062c0ab311d30d62b6ab54d505f0e613cd54826e225e14f708849f3bf98
- spec_test.go: daca39ba173c7612030c8d9b372312acbf3d5149e1db7d60a862e82ab23497e1
- parser_test.go: 3a3f0801d58fed2eb64b5c4809eb6bff48c24fa85bfc28003118f9f84bfa9864

Deviations are deliberate application scope: numeric UTC five fields; minute
resolution, Sunday0..6, no names, descriptors, `?`, seconds, zone annotations or
DST. ASCII-whitespace separates fields. Empty list elements reject, rather than
upstream FieldsFunc's empty-element omission. Calendar and UTC conversion use
shipped std date/time rather than porting Go time or a timezone library. Checked
bounded calendar integers replace ambient Go time/exception flow. Month/day/hour
jumps restart field validation after carry. The horizon matches upstream's
calendar-year+5 rule. Steps accept positive u64 values (Go uses a platform-sized signed int); overflowing
decimal input is an ordinary invalid cron. Literal process argv, capabilities, typed errors and scoped
joins replace ambient goroutines and shell execution.

The file-sync/rename/sync publication follows the shipped journalkv compact
recipe and the specified core file.sync directory-chain contract. Exclusive
mkdir follows the core filesystem contract; put_new's equal-bytes behavior is
not an interprocess lock. No new std API or runtime adapter is included.

An external Go program executes the pinned ParseStandard implementation in UTC
against fixed expected calendar vectors. A separate association-list state model
checks admit/restart/complete/no-retry transitions. Neither calls Hirsel helpers.
Raw oracle/tool hashes and all campaign evidence remain outside the source repo.
