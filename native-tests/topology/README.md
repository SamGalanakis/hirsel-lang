# native-tests/topology

Transient link topologies for PROG-5: one program's
Ready units are emitted as one object, as one object per unit, and under twenty
seeded random partitions, and every executable must observe what the reference
interpreter does. Before each link the manifest is checked: every export is
defined once and every import resolves.

`cases.txt` lists the corpus cases run this way by `tools/check.sh --full`; each has
several units (generic instances, closures, finalizers). The driver test
`singleton_whole_and_twenty_random_partitions_observe_the_same` runs its own
programs through the same 22 partitions, and
`partitions_preserve_the_gen5_verdict_and_sites` checks that the GEN-5 verdict
and sites do not depend on how the instance edges are grouped.

A case here must have no row in `../regressions/exclusions.tsv`.
