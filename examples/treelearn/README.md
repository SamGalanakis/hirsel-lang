# treelearn

Bounded, auditable integer CART classification. Run `hirsel run
examples/treelearn/hirsel.json --grants examples/treelearn/grants.json`.
The fixture grants a read-write directory; successful runs atomically replace
`fixtures/model.json` with a report containing reusable models and explanations.

CLI arguments: `train|predict ROOT INPUT OUTPUT JOBS [--interruptible]`.
JOBS is 1–4; results publish in input order after all tasks join. Interruption
requires a granted signal provider. No timers or Free scheduling are needed.
Exit 0 prints `published`; exit 2 prints `treelearn: CODE` on stderr and
preserves the old output. Output grant failures exit 3.

Train input: `{"version":1,"jobs":[{"features":2,"labels":2,"max_depth":4,
"min_leaf":1,"rows":[{"x":[-1,0],"label":0},{"x":[1,0],"label":1}],
"queries":[[0,0]]}]}`. Predict input replaces each job with
`{"model":MODEL,"queries":[[0,0]]}`. Copy MODEL from a report's jobs array.
Unknown fields, duplicate JSON keys, incomplete vectors and noninteger numbers
are rejected. Zero queries is legal.

Bounds: 64 rows, 4 features, 4 labels (IDs starting at 0), 16 queries per job,
4 jobs per manifest, depth 0–4, 31 nodes, minimum leaf 1–8, 65536 total input
bytes, 32768 output bytes. Features and query values are integers in
[-10000,10000]. The input cap covers the entire manifest, including whitespace.

The trainer enumerates sorted distinct observed boundaries and maximizes
strict Gini improvement using checked u64 rational comparisons. Feature index,
then lower threshold resolve ties; leaf count ties choose the smallest label.
Thresholds are inclusive-left lower observed integers, so unseen integers
between observations intentionally differ from midpoint-based sklearn rules.
Pure nodes, zero gain, depth and minimum-leaf constraints stop expansion.
A root with fewer than min_leaf examples is legal and cannot split.

Model v1 has features, labels, max_depth, min_leaf and a nodes array rooted at
index 0. Nodes contain counts plus either leaf (label ID), or feature,
threshold,left,right. Children are allocated in pairs before depth-first
expansion. Loading checks all shapes, bounds, unique reachability, depth,
count conservation and leaf-label policy before traversal. Training roundtrips
through this same loader. Predictions include leaf ID/counts and every tested
node, feature, value, threshold and branch direction. No missing features,
weights, categorical search, probability calibration or floating scores.

Port references: scikit-learn 59106029d483678e79825f753bde67f8ade4332b,
`sklearn/tree/_criterion.pyx:605–688`, `_splitter.pyx:427–485`, and
`tests/test_tree.py:158–161,249–259`. NOTICE retains BSD-3-Clause terms.
Full feature search and exact ratios replace randomized floating-point search;
upstream toy labels -1/+1 become 0/1. The independent Python Fraction oracle
freezes full models, predictions, paths, counts and index partitions.

Qualification commands select `decision_tree_35_simulation` or
`decision_tree_35_native_record_replay` with `--ignored --nocapture` on CLI
shard1. `DECISION_TREE_35_CASE` selects exact comma-separated recovery IDs;
accepted cells must not be repeated. Physical cells additionally select backend,
width and kind using DECISION_TREE_35_BACKEND/WIDTH/KIND. Both native backends passed widths 1/2/4 for success, domain error and real
SIGINT cancellation: 18 physical cells, each with running, effective=requested,
pending0 controls and replay of its own tape. The 18 scripted cells also passed;
full recording comparison across backends/widths removes only admission time
and placement metadata. These controls prove capacity, not simultaneous
execution or speedup.

The independent oracle checks 45 frozen cases. LLVM and Cranelift each pass
45 live workflows; cross-native replay passes 180 observations. Reference and
checked live execution each pass 40 workflows, with five Stops each. Interpreter
tape replay passes 160 observations with 20 Stops; lowered Heap differential
passes 42 with two Stops and one unsupported typed file-fault case. There are
also 24 binding/request/missing-event/extra-event refusal checks.

The original 64-row, four-job, 65536-byte and 31-node workflows have retained
1M-step interpreter Stops; see FRICTION. A default minimum-bound regression is
not acceptance of those upper-cap identities. No all-engine cap qualification
is claimed.

A conservative worst-case compact report is 28855 bytes: 4 models of 31 nodes
(15 branches,16 leaves), each with16 predictions of4 decisions, using maximum
integer spelling lengths independently. Valid class-count constraints only
reduce that size. Thus a32769-byte output has no admitted-schema vector; the
32768-byte guard remains defensive. The31-node/four-job fixture exercises the
full shape, rather than inventing an invalid larger companion.
