# serde_json micro reference

`//bench/micro-json-reference:json-reference` builds the Rust pair for
`bench/micro/programs/extended/json.hir`. `bench/micro/build json` builds both
participants and writes their argv manifest. This standalone Buck2 target uses
the workspace's pinned serde_json 1.0.151, with default features, at O3.
Construction, ten parse/stringify/drop passes and process startup are included.
Object key order differs, so the checksum is total serialized byte length.
