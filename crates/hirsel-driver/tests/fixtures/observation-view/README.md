# Observation viewer vectors

`ordinary.tape` uses the existing ordinary v7 recording format. The corresponding
`durable.journal` has the same two completions, with root/child Prepared anchors,
CRC-32C frames, and a terminal outcome. Their `.view.json` and `.perfetto.json`
goldens use archival capture name `fixture` and hash the exact recording bytes.
Tests truncate these fixtures and damage interior bytes without regenerating them.

The thread instant vector is adapted from Perfetto commit
`c7b436451a48ccd264cd8abe4e968ca6a191c88f`,
`src/trace_processor/export_json_unittest.cc`, `InstantEventOnThread` (lines 920–950).
It retains `ph: I`, `s: t`, explicit pid/tid and a timestamp coordinate. Here that
coordinate is visibly logical order, without a measured duration. Perfetto's
`protos/perfetto/trace/track_event/track_event.proto`, lines 134–156, describes why
begin/end pairs on one track imply nesting; these fixtures use only instants.
